//! USB/IP client accept loop, URB dispatch, isochronous cancellation, and interrupt report worker.

use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use super::{
    BUS_ID, CMD_SUBMIT, CMD_UNLINK, Device, ECONNRESET, EPIPE, IMPORT_WAIT, IsoPacket, IsoTransfer,
    Link, MAX_TRANSFER, OP_REQ_DEVLIST, OP_REQ_IMPORT, READ_SLICE, RET_SUBMIT, RET_UNLINK, Setup,
    Shared, Transfer, WRITE_WAIT, be32, op_reply, ret,
};

pub(super) fn accept_clients<D: Device>(shared: &Shared<D>, listener: &TcpListener) {
    while !shared.stop.load(Ordering::Acquire) {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                if error.kind() != io::ErrorKind::WouldBlock {
                    tracing::debug!(%error, "USB/IP accept failed");
                }
                // Only idle attachment waits here; connected input and audio
                // retain their own report deadlines and blocking socket I/O.
                std::thread::park_timeout(READ_SLICE);
                continue;
            }
        };
        if !stream.peer_addr().is_ok_and(|a| a.ip().is_loopback()) {
            continue;
        }
        if let Err(error) = serve(shared, stream) {
            tracing::debug!(%error, "USB/IP client disconnected");
        }
        *shared.writer.lock().unwrap() = None;
        let mut link = shared.link.lock().unwrap();
        link.imported = false;
        link.polled = None;
        link.pending.clear();
        link.held.clear();
        link.iso.clear();
        link.iso_next = [None; 16];
        drop(link);
        shared.device.lock().unwrap().iso_discontinuity();
    }
}

/// Fills `buf`, giving up when the export stops or `deadline` passes.
fn read_full<D: Device>(
    shared: &Shared<D>,
    mut stream: &TcpStream,
    buf: &mut [u8],
    deadline: Option<Instant>,
) -> io::Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if shared.stop.load(Ordering::Acquire)
                    || deadline.is_some_and(|d| Instant::now() >= d)
                {
                    return Err(e);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn serve<D: Device>(shared: &Shared<D>, stream: TcpStream) -> io::Result<()> {
    // Accepted sockets may inherit the nonblocking listener's mode.
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(WRITE_WAIT))?;
    stream.set_read_timeout(Some(READ_SLICE))?;
    {
        let mut writer = shared.writer.lock().unwrap();
        if shared.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        *writer = Some(stream.try_clone()?);
    }
    let deadline = Some(Instant::now() + IMPORT_WAIT);
    let read =
        |buf: &mut [u8], deadline: Option<Instant>| read_full(shared, &stream, buf, deadline);
    let mut head = [0u8; 8];
    read(&mut head, deadline)?;
    let code = u16::from_be_bytes([head[2], head[3]]);
    let describe = || shared.device.lock().unwrap().describe();
    match code {
        OP_REQ_DEVLIST => {
            let device = describe();
            let mut b = op_reply(0x0005, 0);
            b.extend_from_slice(&1u32.to_be_bytes());
            b.extend_from_slice(&device.encode());
            for (class, subclass, protocol) in device.interfaces {
                b.extend_from_slice(&[class, subclass, protocol, 0]);
            }
            (&stream).write_all(&b)?;
            return Ok(());
        }
        OP_REQ_IMPORT => {
            let mut busid = [0u8; 32];
            read(&mut busid, deadline)?;
            let len = busid.iter().position(|b| *b == 0).unwrap_or(32);
            if &busid[..len] != BUS_ID.as_bytes() {
                (&stream).write_all(&op_reply(0x0003, 1))?;
                return Ok(());
            }
            let mut b = op_reply(0x0003, 0);
            b.extend_from_slice(&describe().encode());
            (&stream).write_all(&b)?;
        }
        _ => {
            return Err(io::Error::other(format!(
                "unknown USB/IP request {code:#x}"
            )));
        }
    }
    shared.link.lock().unwrap().imported = true;
    tracing::debug!("USB/IP client attached the device");
    loop {
        let mut h = [0u8; 48];
        read(&mut h, None)?;
        let (command, seqnum, direction, endpoint) =
            (be32(&h, 0), be32(&h, 4), be32(&h, 12), be32(&h, 16));
        if direction > 1 || endpoint > 15 {
            return Err(io::Error::other("invalid USB/IP endpoint or direction"));
        }
        match command {
            CMD_SUBMIT => {
                let length = be32(&h, 24) as i32;
                let packets = be32(&h, 32) as i32;
                if !(-1..=1024).contains(&packets) {
                    return Err(io::Error::other("invalid USB/IP packet count"));
                }
                let length = usize::try_from(length)
                    .ok()
                    .filter(|n| *n <= MAX_TRANSFER)
                    .ok_or_else(|| io::Error::other("USB/IP transfer too large"))?;
                let mut data = vec![];
                if direction == 0 {
                    data.resize(length, 0);
                    read(&mut data, None)?;
                }
                if packets > 0 {
                    let mut wire = vec![0; 16 * packets as usize];
                    read(&mut wire, None)?;
                    let mut descriptors = Vec::new();
                    let mut end = 0;
                    for p in wire.as_chunks::<16>().0 {
                        let offset = be32(p, 0) as usize;
                        let size = be32(p, 4) as usize;
                        if offset < end || offset > length || size > length - offset {
                            return Err(io::Error::other("invalid USB/IP isochronous offsets"));
                        }
                        end = offset + size;
                        descriptors.push(IsoPacket {
                            offset,
                            length: size,
                        });
                    }
                    let layout = shared
                        .device
                        .lock()
                        .unwrap()
                        .iso_layout(endpoint as u8, direction == 1);
                    let valid = layout.is_some_and(|(max, align)| {
                        align != 0
                            && descriptors
                                .iter()
                                .all(|p| p.length <= max && p.length.is_multiple_of(align))
                    });
                    let mut link = shared.link.lock().unwrap();
                    let now = Instant::now();
                    let cursor = &mut link.iso_next[endpoint as usize];
                    let start = cursor
                        .filter(|last| {
                            now.saturating_duration_since(*last) <= Duration::from_millis(50)
                        })
                        .unwrap_or(now);
                    let due = start + Duration::from_millis(packets as u64);
                    let transfer = IsoTransfer {
                        seqnum,
                        endpoint: endpoint as u8,
                        input: direction == 1,
                        length,
                        packets: descriptors,
                        data,
                        due,
                        start_frame: due.duration_since(shared.epoch).as_millis() as u32,
                    };
                    if !valid || link.iso.len() >= 64 {
                        drop(link);
                        shared.send(&transfer.reply(if valid { -12 } else { EPIPE }, &[], &[]));
                    } else {
                        link.iso_next[endpoint as usize] = Some(due);
                        link.iso.push_back(transfer);
                        drop(link);
                        shared.wake.notify_all();
                    }
                    continue;
                }
                let reply = if endpoint == 0 {
                    let setup = Setup::parse(h[40..48].try_into().unwrap());
                    let answer = if u32::from(setup.request_type >> 7) != direction
                        || usize::from(setup.length) > length
                    {
                        None
                    } else {
                        shared.device.lock().unwrap().control(&setup, &data)
                    };
                    if answer.is_some() {
                        let reset = (setup.request_type == 0 && setup.request == 9)
                            || (setup.request_type == 0x23
                                && setup.request == 3
                                && setup.value == 4);
                        let stop_audio = setup.request_type == 1
                            && setup.request == 11
                            && setup.value == 0
                            && matches!(setup.index, 1 | 2);
                        if reset || stop_audio {
                            let cancelled = {
                                let mut link = shared.link.lock().unwrap();
                                let mut cancelled = cancel_iso(&mut link, |t| {
                                    reset || u16::from(t.endpoint) == setup.index
                                });
                                if reset {
                                    let mut pending: Vec<_> = link.pending.drain(..).collect();
                                    pending.append(&mut link.held);
                                    for t in pending {
                                        cancelled
                                            .push(ret(RET_SUBMIT, t.seqnum, ECONNRESET, None, 0));
                                    }
                                    link.iso_next = [None; 16];
                                } else {
                                    link.iso_next[setup.index as usize] = None;
                                }
                                cancelled
                            };
                            shared.send_batch(&cancelled);
                        }
                    }
                    match answer {
                        Some(mut answer) if direction == 1 => {
                            answer.truncate(length.min(usize::from(setup.length)));
                            ret(RET_SUBMIT, seqnum, 0, Some(&answer), answer.len())
                        }
                        Some(_) => ret(RET_SUBMIT, seqnum, 0, None, length),
                        None => ret(RET_SUBMIT, seqnum, EPIPE, None, 0),
                    }
                } else if direction == 1 {
                    let mut link = shared.link.lock().unwrap();
                    if link.pending.len() + link.held.len() >= 64 {
                        drop(link);
                        shared.send(&ret(RET_SUBMIT, seqnum, -12, None, 0));
                        continue;
                    }
                    link.pending.push_back(Transfer {
                        seqnum,
                        endpoint: endpoint as u8,
                        length,
                    });
                    link.polled.get_or_insert_with(Instant::now);
                    link.idle = false;
                    drop(link);
                    shared.wake.notify_all();
                    continue;
                } else {
                    let (accepted, retry) = {
                        let mut device = shared.device.lock().unwrap();
                        let accepted = device.output(endpoint as u8, &data);
                        (accepted, accepted && device.retries_held())
                    };
                    let reply = ret(
                        RET_SUBMIT,
                        seqnum,
                        if accepted { 0 } else { EPIPE },
                        None,
                        if accepted { length } else { 0 },
                    );
                    // The OUT transfer completes before the IN transfer that
                    // carries its answer, as on the wire.
                    shared.send(&reply);
                    if retry {
                        let mut link = shared.link.lock().unwrap();
                        link.retry_held();
                        drop(link);
                        shared.wake.notify_all();
                    }
                    continue;
                };
                shared.send(&reply);
            }
            CMD_UNLINK => {
                let victim = be32(&h, 20);
                let reply = {
                    let mut link = shared.link.lock().unwrap();
                    let before = link.pending.len() + link.held.len() + link.iso.len();
                    link.pending.retain(|t| t.seqnum != victim);
                    link.held.retain(|t| t.seqnum != victim);
                    link.iso.retain(|t| t.seqnum != victim);
                    for endpoint in 0..16 {
                        link.iso_next[endpoint] = link
                            .iso
                            .iter()
                            .rev()
                            .find(|t| usize::from(t.endpoint) == endpoint)
                            .map(|t| t.due);
                    }
                    // A transfer already completed is not cancelled: status 0.
                    let found = link.pending.len() + link.held.len() + link.iso.len() != before;
                    let status = if found { ECONNRESET } else { 0 };
                    ret(RET_UNLINK, seqnum, status, None, 0)
                };
                shared.send(&reply);
            }
            _ => {
                return Err(io::Error::other(format!(
                    "unknown USB/IP command {command}"
                )));
            }
        }
    }
}

fn cancel_iso(link: &mut Link, matches: impl Fn(&IsoTransfer) -> bool) -> Vec<Vec<u8>> {
    let mut replies = Vec::new();
    let mut i = 0;
    while i < link.iso.len() {
        if matches(&link.iso[i]) {
            let transfer = link.iso.remove(i).unwrap();
            replies.push(transfer.reply(ECONNRESET, &[], &[]));
        } else {
            i += 1;
        }
    }
    replies
}

/// Completes interrupt transfers: one per endpoint each period, and at once
/// when the device changed.
pub(super) fn report<D: Device>(shared: &Shared<D>) {
    let mut next = Instant::now();
    let mut link = shared.link.lock().unwrap();
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        let polling = !link.pending.is_empty() && !link.idle;
        let waiting = link.imported && (polling || !link.iso.is_empty());
        let now = Instant::now();
        if !waiting {
            link = shared.wake.wait(link).unwrap();
            continue;
        }
        let due = link
            .iso
            .iter()
            .map(|t| t.due)
            .chain(polling.then_some(if link.changed { now } else { next }))
            .min()
            .unwrap_or(next);
        if now < due {
            link = shared.wake.wait_timeout(link, due - now).unwrap().0;
            continue;
        }
        let mut replies = Vec::new();
        let mut device = shared.device.lock().unwrap();
        let mut iso_index = 0;
        while iso_index < link.iso.len() {
            if link.iso[iso_index].due > now {
                iso_index += 1;
                continue;
            }
            let transfer = link.iso.remove(iso_index).unwrap();
            // A delayed worker completes old USB requests but must not play
            // their audio at the current time or join it to a fresh PCM block.
            if !transfer.input && now.saturating_duration_since(transfer.due) > shared.late {
                device.iso_discontinuity();
                replies.push(transfer.reply(0, &[], &[]));
                continue;
            }
            let mut data = vec![];
            let mut lengths = vec![];
            let mut status = 0;
            for packet in &transfer.packets {
                let out = if transfer.input {
                    &[][..]
                } else {
                    &transfer.data[packet.offset..packet.offset + packet.length]
                };
                match device.iso(transfer.endpoint, transfer.input, out, packet.length) {
                    Some(bytes) if bytes.len() <= packet.length => {
                        lengths.push(bytes.len());
                        data.extend_from_slice(&bytes);
                    }
                    _ => {
                        status = EPIPE;
                        data.clear();
                        break;
                    }
                }
            }
            replies.push(transfer.reply(status, &data, &lengths));
        }
        let mut served = 0u32;
        let mut sent = false;
        let mut naked = false;
        let mut i = 0;
        while polling && (link.changed || now >= next) && i < link.pending.len() {
            let transfer = link.pending[i];
            let bit = 1u32 << (transfer.endpoint & 31);
            if served & bit != 0 {
                i += 1;
                continue;
            }
            served |= bit;
            if !device.interrupt_ready(transfer.endpoint) {
                // A NAK: the transfer waits for the device's next change.
                naked = true;
                i += 1;
                continue;
            }
            link.pending.remove(i);
            match device.transfer_in(transfer.endpoint, transfer.length) {
                Some(mut data) => {
                    data.truncate(transfer.length);
                    replies.push(ret(RET_SUBMIT, transfer.seqnum, 0, Some(&data), data.len()));
                    sent = true;
                }
                None => link.held.push(transfer),
            }
        }
        // Drain back-to-back queued HID/bulk reports onto subsequent waiting URBs immediately
        // instead of sleeping for `shared.period` between reports. Periodic endpoints return
        // `interrupt_queued() == false` so ping-pong URBs stay paced at `shared.period`.
        let more_ready = sent
            && link
                .pending
                .iter()
                .any(|t| device.interrupt_queued(t.endpoint));
        drop(device);
        if sent {
            link.changed = more_ready;
            next = now + shared.period;
        } else if naked {
            link.changed = false;
            link.idle = true;
        }
        if !replies.is_empty() {
            drop(link);
            shared.send_batch(&replies);
            link = shared.link.lock().unwrap();
        }
    }
}
