//! USB/IP loopback relay, import verification, and in-flight transfer framing validation.

use super::{Opened, Requested, UsbForwarding, Vhci, status, unplug};
use anyhow::{Context, Result, anyhow, bail};
use rubylight_protocol::{
    usb::{UsbReason, UsbState},
    usbip::{self as wire, Header},
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Notify,
};

/// How long the driver may take to import the device, the client's answer included.
const OPEN_TIMEOUT: Duration = Duration::from_secs(20);
/// Transfers in flight on one device; a driver keeps a few per endpoint.
const MAX_IN_FLIGHT: usize = 512;

/// [`unplug`] off the async workers: usbip.exe takes up to 15 s.
async fn unplug_blocking(
    vhci: Arc<dyn Vhci>,
    port: Option<u32>,
    tcp_port: u16,
    busid: String,
) -> Result<()> {
    tokio::task::spawn_blocking(move || unplug(&*vhci, port, tcp_port, &busid))
        .await
        .context("the driver's detach stopped")?
}

impl UsbForwarding {
    /// Relays a claimed tunnel to the driver until either end closes or something detaches it.
    pub async fn serve<T>(&self, opened: Opened, mut tunnel: T)
    where
        T: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let vhci = self.vhci();
        let busid = opened.device.busid.clone();
        let (mut driver, tcp_port, port) = match open(vhci.clone(), &mut tunnel, &opened).await {
            Ok(OpenOutcome::Opened {
                driver,
                tcp_port,
                port,
            }) => (driver, tcp_port, port),
            Ok(OpenOutcome::Stopped { tcp_port, port }) => {
                if let Err(error) = unplug_blocking(vhci, port, tcp_port, busid).await {
                    tracing::debug!(port = ?port, error = format!("{error:#}"), "USB detach after stop while opening");
                }
                return self.finish(&opened, Vec::new());
            }
            Err(failed) => {
                tracing::warn!(client = %opened.client, device = %opened.device.name, error = format!("{:#}", failed.error), reason = ?failed.reason, "USB attach failed");
                let notice = vec![status(opened.request_id, UsbState::Failed, failed.reason)];
                // Whatever the driver took in comes out again.
                if let Some(tcp_port) = failed.tcp_port
                    && let Err(error) = unplug_blocking(vhci, failed.port, tcp_port, busid).await
                {
                    return self.stuck(&opened, failed.port, tcp_port, error, notice);
                }
                return self.finish(&opened, notice);
            }
        };
        self.attached(&opened, port);
        let outcome = pump(&mut driver, tunnel, &opened.stop).await;
        // The driver's connection stays open until it detached: closing it first would have
        // the driver drop the device on its own and the detach find no port.
        let mut held = Some(port);
        let mut detached = unplug_blocking(vhci.clone(), held, tcp_port, busid.clone()).await;
        drop(driver);
        if let Err(error) = &detached
            && matches!(outcome, Ok(Closed::Driver))
        {
            // A driver that closed its end has unplugged the device itself: its port is gone.
            tracing::warn!(
                port,
                error = format!("{error:#}"),
                "USB detach found no port after the driver closed the relay"
            );
            held = None;
            detached = unplug_blocking(vhci, held, tcp_port, busid).await;
        }
        if let Err(error) = detached {
            let natural = vec![status(
                opened.request_id,
                UsbState::Failed,
                UsbReason::Driver,
            )];
            return self.stuck(&opened, held, tcp_port, error, natural);
        }
        let natural = match outcome {
            Ok(Closed::Requested) => Vec::new(),
            Ok(Closed::Device) => {
                tracing::info!(client = %opened.client, device = %opened.device.name, "USB device detached: the client closed its tunnel");
                vec![status(
                    opened.request_id,
                    UsbState::Detached,
                    UsbReason::None,
                )]
            }
            Ok(Closed::Driver) => {
                tracing::warn!(client = %opened.client, device = %opened.device.name, "USB device detached: the driver closed it");
                vec![status(
                    opened.request_id,
                    UsbState::Failed,
                    UsbReason::Driver,
                )]
            }
            Err(error) => {
                tracing::warn!(client = %opened.client, device = %opened.device.name, error = format!("{error:#}"), "USB device detached: the relay failed");
                vec![status(
                    opened.request_id,
                    UsbState::Failed,
                    UsbReason::DeviceError,
                )]
            }
        };
        self.finish(&opened, natural);
    }
}

/// Why a device did not attach, and what the driver may still hold.
struct OpenFailed {
    reason: UsbReason,
    error: anyhow::Error,
    /// The relay's port once the driver was asked to attach.
    tcp_port: Option<u16>,
    /// The hub port, when the driver reported one.
    port: Option<u32>,
}

/// Imports the device into the driver: the driver's import request goes to the client and its
/// answer back, while the driver attaches. Returns the driver's connection, the loopback port
/// and the hub port.
enum OpenOutcome {
    Opened {
        driver: TcpStream,
        tcp_port: u16,
        port: u32,
    },
    Stopped {
        tcp_port: u16,
        port: Option<u32>,
    },
}

async fn open<T>(
    vhci: Arc<dyn Vhci>,
    tunnel: &mut T,
    opened: &Opened,
) -> Result<OpenOutcome, OpenFailed>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    let unbound = |error: anyhow::Error| OpenFailed {
        reason: UsbReason::Driver,
        error,
        tcp_port: None,
        port: None,
    };
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .context("binding the USB/IP relay")
        .map_err(unbound)?;
    let relay = listener
        .local_addr()
        .context("the USB/IP relay's port")
        .map_err(unbound)?;
    let tcp_port = relay.port();
    let busid = opened.device.busid.clone();
    let attaching = vhci.clone();
    let mut attach = std::pin::pin!(async move {
        tokio::task::spawn_blocking(move || attaching.attach(tcp_port, &busid))
            .await
            .context("the driver's attach stopped")?
    });
    let (imported, attached) = {
        let mut import = std::pin::pin!(tokio::time::timeout(
            OPEN_TIMEOUT,
            import(vhci, listener, relay, tunnel, &opened.device)
        ));
        let (mut imported, mut attached) = (None, None);
        let mut stopped = false;
        while imported.is_none() || attached.is_none() {
            tokio::select! {
                () = opened.stop.notified() => {
                    stopped = true;
                    break;
                }
                result = &mut import, if imported.is_none() => imported = Some(result.unwrap_or_else(|_| {
                    Err((UsbReason::Driver, anyhow!("the driver did not import the device within {OPEN_TIMEOUT:?}")))
                })),
                result = &mut attach, if attached.is_none() => {
                    // A failed attach never imports: stop waiting for it.
                    if result.is_err() && imported.is_none() {
                        imported = Some(Err((UsbReason::Driver, anyhow!("the driver did not import the device"))));
                    }
                    attached = Some(result);
                }
            }
        }
        if stopped {
            let port = attached.and_then(Result::ok);
            return Ok(OpenOutcome::Stopped { tcp_port, port });
        }
        (imported.unwrap(), attached.unwrap())
    };
    // The caller unplugs whatever the driver may hold once the attach started.
    let failed = |reason, error, port| OpenFailed {
        reason,
        error,
        tcp_port: Some(tcp_port),
        port,
    };
    match (imported, attached) {
        (Ok(driver), Ok(port)) => Ok(OpenOutcome::Opened {
            driver,
            tcp_port,
            port,
        }),
        (Err((reason, error)), Ok(port)) => Err(failed(reason, error, Some(port))),
        (Ok(driver), Err(error)) => {
            // Without its connection the driver lets go of a device it plugged in anyway.
            drop(driver);
            Err(failed(UsbReason::Driver, error, None))
        }
        // The client's answer explains a failed attach better than usbip.exe does.
        (Err((UsbReason::DeviceError, error)), Err(_)) => {
            Err(failed(UsbReason::DeviceError, error, None))
        }
        (Err(_), Err(error)) => Err(failed(UsbReason::Driver, error, None)),
    }
}

/// Takes the driver's connection and passes its import request and the client's reply,
/// checking that the reply is the device the client asked to attach. Connections from anyone
/// but the driver are dropped; the caller's deadline bounds the wait.
async fn import<T>(
    vhci: Arc<dyn Vhci>,
    listener: TcpListener,
    relay: SocketAddr,
    tunnel: &mut T,
    device: &Requested,
) -> Result<TcpStream, (UsbReason, anyhow::Error)>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let driver_error = |e: anyhow::Error| (UsbReason::Driver, e);
    let device_error = |e: anyhow::Error| (UsbReason::DeviceError, e);
    let mut driver = loop {
        let (stream, peer) = listener
            .accept()
            .await
            .context("accepting the driver")
            .map_err(driver_error)?;
        let checking = vhci.clone();
        let known = tokio::task::spawn_blocking(move || checking.is_driver(peer, relay))
            .await
            .context("the driver check stopped")
            .and_then(|known| known)
            .context("checking who connected to the USB/IP relay")
            .map_err(driver_error)?;
        if known {
            break stream;
        }
        tracing::error!(%peer, device = %device.name, "dropped a connection to the USB/IP relay that was not the driver's");
    };
    drop(listener);
    let _ = driver.set_nodelay(true);
    let mut request = [0; wire::IMPORT_REQUEST_BYTES];
    driver
        .read_exact(&mut request)
        .await
        .context("reading the driver's import request")
        .map_err(driver_error)?;
    match wire::decode_import_request(&request) {
        Some(busid) if busid == device.busid => {}
        other => {
            return Err(driver_error(anyhow!(
                "the driver asked for {other:?}, not {}",
                device.busid
            )));
        }
    }
    let mut reply = [0; wire::OP_HEADER_BYTES + wire::DEVICE_BYTES];
    let result: Result<usize> = async {
        tunnel.write_all(&request).await?;
        tunnel.flush().await?;
        tunnel
            .read_exact(&mut reply[..wire::OP_HEADER_BYTES])
            .await
            .context("the client closed the tunnel")?;
        let header = wire::OpHeader::decode(reply[..wire::OP_HEADER_BYTES].try_into()?);
        if header.code != wire::OP_REP_IMPORT {
            bail!("the client answered op {:#06x}", header.code);
        }
        if header.status != 0 {
            return Ok(wire::OP_HEADER_BYTES);
        }
        tunnel
            .read_exact(&mut reply[wire::OP_HEADER_BYTES..])
            .await
            .context("the client closed the tunnel")?;
        let described = wire::Device::decode(reply[wire::OP_HEADER_BYTES..].try_into()?)
            .context("the client described the device badly")?;
        if described.busid != device.busid
            || described.vendor != device.vid
            || described.product != device.pid
        {
            bail!(
                "the client served {} {:04x}:{:04x}, not {} {:04x}:{:04x}",
                described.busid,
                described.vendor,
                described.product,
                device.busid,
                device.vid,
                device.pid
            );
        }
        Ok(reply.len())
    }
    .await;
    let length = result.map_err(device_error)?;
    driver
        .write_all(&reply[..length])
        .await
        .context("answering the driver's import request")
        .map_err(driver_error)?;
    if length == wire::OP_HEADER_BYTES {
        return Err(device_error(anyhow!("the client refused the import")));
    }
    Ok(driver)
}

/// Which end closed a relay cleanly.
#[derive(Debug, PartialEq, Eq)]
enum Closed {
    Requested,
    Device,
    Driver,
}

struct Submitted {
    direction: u32,
    length: u32,
    packets: u32,
    /// The isochronous packets the driver asked for; the reply must keep their layout.
    iso: Box<[wire::IsoPacket]>,
}

/// The driver's transfers the client has yet to answer.
#[derive(Default)]
struct InFlight {
    submits: std::collections::HashMap<u32, Submitted>,
    /// Unlink seqnum to the submit it cancels.
    unlinks: std::collections::HashMap<u32, u32>,
}

/// Passes USB/IP both ways until an end closes or `stop` is notified. The driver's connection
/// stays open for the detach.
async fn pump<T>(driver: &mut TcpStream, tunnel: T, stop: &Notify) -> Result<Closed>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let (mut from_driver, mut to_driver) = driver.split();
    let (mut from_client, mut to_client) = tokio::io::split(tunnel);
    let flight = Mutex::new(InFlight::default());
    tokio::select! {
        result = commands(&mut from_driver, &mut to_client, &flight) => result.map(|()| Closed::Driver),
        result = replies(&mut from_client, &mut to_driver, &flight) => result.map(|()| Closed::Device),
        () = stop.notified() => Ok(Closed::Requested),
    }
}

/// Reads one header; false when the peer closed between messages.
async fn header<R: AsyncRead + Unpin>(
    from: &mut R,
    header: &mut [u8; wire::HEADER_BYTES],
) -> Result<bool> {
    let first = from.read(header).await?;
    if first == 0 {
        return Ok(false);
    }
    from.read_exact(&mut header[first..]).await?;
    Ok(true)
}

/// The driver's submits and unlinks, to the client.
async fn commands<R, W>(from: &mut R, to: &mut W, flight: &Mutex<InFlight>) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut message = Vec::new();
    loop {
        let mut head = [0; wire::HEADER_BYTES];
        if !header(from, &mut head)
            .await
            .context("reading the driver")?
        {
            return Ok(());
        }
        let header = match Header::decode(&head) {
            Some(header @ (Header::Submit { .. } | Header::Unlink { .. })) => header,
            other => bail!("the driver sent {other:?}"),
        };
        let payload = match header {
            Header::Submit {
                base,
                length,
                packets,
                ..
            } => wire::submit_payload(base.direction, length, packets).with_context(|| {
                format!("the driver submitted {length} bytes, {packets} packets")
            })?,
            _ => 0,
        };
        message.clear();
        message.extend_from_slice(&head);
        message.resize(wire::HEADER_BYTES + payload, 0);
        from.read_exact(&mut message[wire::HEADER_BYTES..])
            .await
            .context("reading the driver")?;
        // In flight before the client can see it, so its reply finds it.
        match header {
            Header::Submit {
                base,
                length,
                packets,
                ..
            } => {
                let mut flight = flight.lock().unwrap();
                if flight.submits.len() >= MAX_IN_FLIGHT {
                    bail!("the driver has more than {MAX_IN_FLIGHT} transfers in flight");
                }
                let submitted = Submitted {
                    direction: base.direction,
                    length,
                    packets,
                    iso: wire::iso_descriptors(&message[wire::HEADER_BYTES..], packets)
                        .context("the driver's packet descriptors")?
                        .collect(),
                };
                if flight.submits.insert(base.seqnum, submitted).is_some() {
                    bail!("the driver reused seqnum {}", base.seqnum);
                }
            }
            Header::Unlink { base, unlink } => {
                flight.lock().unwrap().unlinks.insert(base.seqnum, unlink);
            }
            _ => {}
        }
        to.write_all(&message).await.context("writing the tunnel")?;
        to.flush().await.context("writing the tunnel")?;
    }
}

/// The client's replies, to the driver, each checked against what the driver asked.
async fn replies<R, W>(from: &mut R, to: &mut W, flight: &Mutex<InFlight>) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut message = Vec::new();
    loop {
        let mut head = [0; wire::HEADER_BYTES];
        if !header(from, &mut head)
            .await
            .context("reading the tunnel")?
        {
            return Ok(());
        }
        let mut answered = None;
        let payload = match Header::decode(&head) {
            Some(Header::RetSubmit {
                base,
                length,
                packets,
                ..
            }) => {
                let submitted = flight
                    .lock()
                    .unwrap()
                    .submits
                    .remove(&base.seqnum)
                    .with_context(|| {
                        format!(
                            "the client answered seqnum {}, which is not in flight",
                            base.seqnum
                        )
                    })?;
                let payload = wire::reply_payload(
                    submitted.direction,
                    submitted.length,
                    submitted.packets,
                    length,
                    packets,
                )
                .with_context(|| {
                    format!(
                        "the client answered {} bytes and {packets} packets to {} bytes and {} packets",
                        length, submitted.length, submitted.packets
                    )
                })?;
                answered = Some((base.seqnum, length, submitted));
                payload
            }
            Some(Header::RetUnlink { base, .. }) => {
                let mut flight = flight.lock().unwrap();
                let unlinked = flight.unlinks.remove(&base.seqnum).with_context(|| {
                    format!(
                        "the client answered unlink {}, which is not in flight",
                        base.seqnum
                    )
                })?;
                // Whatever the status (-ECONNRESET when it cancelled the transfer, 0 or -ENOENT
                // when the transfer had finished, -ESHUTDOWN when the device went away), no
                // RET_SUBMIT follows: one that came first already took it out.
                flight.submits.remove(&unlinked);
                0
            }
            other => bail!("the client sent {other:?}"),
        };
        message.clear();
        message.extend_from_slice(&head);
        message.resize(wire::HEADER_BYTES + payload, 0);
        from.read_exact(&mut message[wire::HEADER_BYTES..])
            .await
            .context("reading the tunnel")?;
        // The driver places isochronous data by the descriptors, so they must describe it.
        if let Some((seqnum, length, submitted)) = answered
            && !submitted.iso.is_empty()
        {
            let replied = wire::iso_descriptors(&message[wire::HEADER_BYTES..], submitted.packets)
                .context("the client's packet descriptors")?;
            if !wire::iso_reply_fits(&submitted.iso, replied, length) {
                bail!(
                    "the client's isochronous packets for seqnum {seqnum} do not match the {} bytes it answered or the packets asked for",
                    length
                );
            }
        }
        to.write_all(&message).await.context("writing the driver")?;
    }
}
