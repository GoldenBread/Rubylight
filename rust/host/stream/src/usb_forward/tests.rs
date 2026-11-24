//! USB forwarding against a fake driver and a scripted client device.

use super::*;
use rubylight_protocol::usbip::Header;
use std::sync::atomic::Ordering;
use tokio::io::DuplexStream;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

mod relay;

const SESSION: &str = "session-1";
const BUSID: &str = "1-1";
const VID: u16 = 0x0781;
const PID: u16 = 0x5581;

fn request(request_id: u32, busid: &str) -> UsbMessage {
    UsbMessage::AttachRequest {
        request_id,
        vid: VID,
        pid: PID,
        class: 8,
        speed: 3,
        busid: busid.into(),
        name: "Flash drive".into(),
    }
}

pub(crate) use super::test_support::{FakeVhci, Script, client};

fn forwarding(vhci: Arc<dyn Vhci>) -> Arc<UsbForwarding> {
    let forwarding = Arc::new(UsbForwarding::default());
    forwarding.set_vhci(vhci);
    forwarding
}

fn refusal(messages: &[UsbMessage]) -> Option<UsbReason> {
    match messages {
        [
            UsbMessage::Status {
                state: UsbState::Refused,
                reason,
                ..
            },
        ] => Some(*reason),
        _ => None,
    }
}

fn token(messages: &[UsbMessage]) -> [u8; TOKEN_BYTES] {
    match messages {
        [UsbMessage::TunnelReady { token, .. }] => *token,
        other => panic!("expected a TunnelReady, got {other:?}"),
    }
}

/// Asks for the device and returns its token.
fn asked(forwarding: &UsbForwarding, request_id: u32) -> [u8; TOKEN_BYTES] {
    forwarding.on_message(SESSION, &client(true), true, 4, request(request_id, BUSID));
    token(&forwarding.take(SESSION, Instant::now()))
}

#[test]
fn attach_requests_are_refused_by_setting_permission_busid_driver_and_limit() {
    let (ready, _) = FakeVhci::new(true);
    let forwarding = forwarding(ready);
    let now = Instant::now();
    let ask = |client: &Client, enabled: bool, limit: u8, busid: &str| {
        forwarding.on_message(SESSION, client, enabled, limit, request(7, busid));
        refusal(&forwarding.take(SESSION, now))
    };
    assert_eq!(
        ask(&client(true), false, 4, BUSID),
        Some(UsbReason::Disabled)
    );
    assert_eq!(
        ask(&client(false), true, 4, BUSID),
        Some(UsbReason::Permission)
    );
    // Full control never implies USB.
    let mut full_control = client(false);
    full_control.perm = u32::MAX;
    assert_eq!(
        ask(&full_control, true, 4, BUSID),
        Some(UsbReason::Permission)
    );
    let mut disabled = client(true);
    disabled.enabled = false;
    assert_eq!(ask(&disabled, true, 4, BUSID), Some(UsbReason::Permission));
    assert_eq!(
        ask(&client(true), true, 4, "-1 --remote evil"),
        Some(UsbReason::DeviceError)
    );
    assert_eq!(ask(&client(true), true, 0, BUSID), Some(UsbReason::Limit));
    assert!(forwarding.list().is_empty());

    let (missing, _) = FakeVhci::new(false);
    let forwarding = self::forwarding(missing);
    forwarding.on_message(SESSION, &client(true), true, 4, request(7, BUSID));
    assert_eq!(
        refusal(&forwarding.take(SESSION, now)),
        Some(UsbReason::Driver)
    );
}

#[test]
fn the_device_limit_counts_every_client() {
    let (vhci, _) = FakeVhci::new(true);
    let forwarding = forwarding(vhci);
    forwarding.on_message("session-a", &client(true), true, 1, request(1, BUSID));
    token(&forwarding.take("session-a", Instant::now()));
    forwarding.on_message("session-b", &client(true), true, 1, request(1, BUSID));
    assert_eq!(
        refusal(&forwarding.take("session-b", Instant::now())),
        Some(UsbReason::Limit)
    );
    // A repeated request id is ignored, not attached twice.
    forwarding.on_message("session-a", &client(true), true, 4, request(1, BUSID));
    assert!(forwarding.take("session-a", Instant::now()).is_empty());
    assert_eq!(forwarding.list().len(), 1);
}

#[test]
fn a_token_opens_once_within_its_time_and_only_for_its_device() {
    let (vhci, _) = FakeVhci::new(true);
    let forwarding = forwarding(vhci);
    let token = asked(&forwarding, 1);
    let now = Instant::now();
    assert_eq!(
        forwarding
            .claim(&[0; TOKEN_BYTES], "cert-phone", now)
            .unwrap_err(),
        Refused::Unknown
    );
    assert_eq!(
        forwarding.claim(&token, "cert-other", now).unwrap_err(),
        Refused::Unknown
    );
    let late = now + Duration::from_secs(TOKEN_TTL_SECS + 1);
    assert_eq!(
        forwarding.claim(&token, "cert-phone", late).unwrap_err(),
        Refused::Unknown
    );
    let opened = forwarding.claim(&token, "cert-phone", now).unwrap();
    assert_eq!(opened.request_id, 1);
    assert_eq!(forwarding.list()[0].state, "opening");
    assert_eq!(
        forwarding.claim(&token, "cert-phone", now).unwrap_err(),
        Refused::Used
    );
    forwarding.abandon(opened, "upgrade failed");
    assert!(forwarding.list().is_empty());
    assert!(matches!(
        forwarding.take(SESSION, now)[..],
        [UsbMessage::Status {
            request_id: 1,
            state: UsbState::Failed,
            reason: UsbReason::DeviceError
        }]
    ));
}

#[test]
fn an_unopened_tunnel_times_out() {
    let (vhci, _) = FakeVhci::new(true);
    let forwarding = forwarding(vhci);
    let token = asked(&forwarding, 2);
    let late = Instant::now() + Duration::from_secs(TOKEN_TTL_SECS + 1);
    assert!(matches!(
        forwarding.take(SESSION, late)[..],
        [UsbMessage::Status {
            request_id: 2,
            state: UsbState::Failed,
            reason: UsbReason::TunnelTimeout
        }]
    ));
    assert!(forwarding.list().is_empty());
    assert!(
        forwarding
            .claim(&token, "cert-phone", Instant::now())
            .is_err()
    );
}

/// The control loop reads `queued` to skip the registry's lock; an answer
/// queued after its look must still be taken on the next one, and a token
/// still to open keeps the gate set until it is reported.
#[test]
fn a_message_queued_after_the_loops_look_is_taken_on_the_next_one() {
    let (vhci, _) = FakeVhci::new(true);
    let forwarding = forwarding(vhci);
    let now = Instant::now();
    // A quiet pass: the gate the control loop reads says nothing waits.
    assert!(!forwarding.queued());
    assert!(forwarding.take(SESSION, now).is_empty());
    assert!(!forwarding.queued(), "a quiet pass reads the gate clear");
    // The refusal lands after the look above.
    forwarding.on_message(SESSION, &client(true), false, 4, request(7, BUSID));
    assert!(forwarding.queued(), "the next pass sees the answer");
    assert_eq!(
        refusal(&forwarding.take(SESSION, now)),
        Some(UsbReason::Disabled)
    );
    assert!(!forwarding.queued(), "the answer was taken");
    // An attachment whose tunnel has not opened keeps the gate set: its
    // failure has to be reported when its token runs out.
    forwarding.on_message(SESSION, &client(true), true, 4, request(8, BUSID));
    assert!(forwarding.queued(), "the token still waits");
    token(&forwarding.take(SESSION, now));
    assert!(forwarding.queued(), "the attachment is still to open");
    let late = now + Duration::from_secs(TOKEN_TTL_SECS + 1);
    assert!(matches!(
        forwarding.take(SESSION, late)[..],
        [UsbMessage::Status {
            request_id: 8,
            state: UsbState::Failed,
            reason: UsbReason::TunnelTimeout
        }]
    ));
    assert!(!forwarding.queued(), "nothing waits for anyone");
}

#[test]
fn waiting_attachments_end_with_the_client_the_console_and_the_session() {
    let (vhci, _) = FakeVhci::new(true);
    let forwarding = forwarding(vhci);
    let now = Instant::now();

    asked(&forwarding, 1);
    forwarding.on_message(
        SESSION,
        &client(true),
        true,
        4,
        UsbMessage::Detach { request_id: 1 },
    );
    assert!(matches!(
        forwarding.take(SESSION, now)[..],
        [UsbMessage::Status {
            request_id: 1,
            state: UsbState::Detached,
            reason: UsbReason::None
        }]
    ));
    assert!(forwarding.list().is_empty());

    let token = asked(&forwarding, 2);
    let id = forwarding.list()[0].id;
    assert_eq!(forwarding.list()[0].state, "waiting");
    assert!(forwarding.detach(id).unwrap());
    assert!(!forwarding.detach(id).unwrap());
    assert!(matches!(
        forwarding.take(SESSION, now)[..],
        [
            UsbMessage::Detach { request_id: 2 },
            UsbMessage::Status {
                request_id: 2,
                state: UsbState::Detached,
                reason: UsbReason::None
            }
        ]
    ));
    assert!(forwarding.claim(&token, "cert-phone", now).is_err());

    asked(&forwarding, 3);
    forwarding.on_message("other", &client(true), true, 4, request(1, BUSID));
    assert!(matches!(
        forwarding.end_session(SESSION)[..],
        [
            UsbMessage::Detach { request_id: 3 },
            UsbMessage::Status {
                request_id: 3,
                state: UsbState::Detached,
                reason: UsbReason::SessionEnd
            }
        ]
    ));
    assert_eq!(forwarding.list().len(), 1, "another session's device stays");
    assert!(forwarding.end_session(SESSION).is_empty());
}

/// A client device: answers the driver's import with the requested device.
async fn serve_import(client: &mut DuplexStream, vendor: u16) {
    let mut request = [0; wire::IMPORT_REQUEST_BYTES];
    client.read_exact(&mut request).await.unwrap();
    assert_eq!(
        wire::decode_import_request(&request).as_deref(),
        Some(BUSID)
    );
    let device = wire::Device {
        path: "/android/usb/1".into(),
        busid: BUSID.into(),
        busnum: 1,
        devnum: 1,
        speed: 3,
        vendor,
        product: PID,
        bcd_device: 0x0100,
        class: 0,
        subclass: 0,
        protocol: 0,
        configuration: 1,
        configurations: 1,
        interfaces: 1,
    };
    client.write_all(&wire::import_reply(0)).await.unwrap();
    client.write_all(&device.encode().unwrap()).await.unwrap();
}

async fn read_header<R: AsyncRead + Unpin>(from: &mut R) -> Header {
    let mut head = [0; wire::HEADER_BYTES];
    from.read_exact(&mut head).await.unwrap();
    Header::decode(&head).unwrap()
}

fn base(seqnum: u32, direction: u32, ep: u32) -> wire::Base {
    wire::Base {
        seqnum,
        devid: 0x10002,
        direction,
        ep,
    }
}

fn submit(seqnum: u32, direction: u32, length: u32) -> Header {
    Header::Submit {
        base: base(seqnum, direction, 1),
        flags: 0,
        length,
        start_frame: 0,
        packets: wire::NOT_ISO,
        interval: 0,
        setup: [0; 8],
    }
}

fn ret_submit(seqnum: u32, length: u32) -> Header {
    Header::RetSubmit {
        base: base(seqnum, 0, 0),
        status: 0,
        length,
        start_frame: 0,
        packets: wire::NOT_ISO,
        errors: 0,
    }
}

fn iso(offset: u32, length: u32, actual_length: u32) -> wire::IsoPacket {
    wire::IsoPacket {
        offset,
        length,
        actual_length,
        status: 0,
    }
}

/// A `CMD_SUBMIT` with its packets, as the driver sends it.
fn iso_submit(seqnum: u32, direction: u32, length: u32, packets: &[wire::IsoPacket]) -> Vec<u8> {
    let mut out = Header::Submit {
        base: base(seqnum, direction, 1),
        flags: 0,
        length,
        start_frame: 0,
        packets: packets.len() as u32,
        interval: 1,
        setup: [0; 8],
    }
    .encode()
    .to_vec();
    out.extend(packets.iter().flat_map(|packet| packet.encode()));
    out
}

/// A `RET_SUBMIT` for an isochronous IN transfer: the packed data, then the packets.
fn iso_reply(seqnum: u32, data: &[u8], packets: &[wire::IsoPacket]) -> Vec<u8> {
    let mut out = Header::RetSubmit {
        base: base(seqnum, 0, 0),
        status: 0,
        length: data.len() as u32,
        start_frame: 0,
        packets: packets.len() as u32,
        errors: 0,
    }
    .encode()
    .to_vec();
    out.extend_from_slice(data);
    out.extend(packets.iter().flat_map(|packet| packet.encode()));
    out
}

/// Whether `seqnum` is out of flight: the driver submits it again (a seqnum still in flight is
/// refused as reused) and the client answers it.
async fn relay_flight_is_empty(
    driver: &mut TcpStream,
    client: &mut DuplexStream,
    seqnum: u32,
) -> bool {
    driver
        .write_all(&submit(seqnum, wire::DIR_IN, 64).encode())
        .await
        .unwrap();
    let mut head = [0; wire::HEADER_BYTES];
    let reached = tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut head)).await;
    if !matches!(reached, Ok(Ok(_))) {
        return false;
    }
    client
        .write_all(&ret_submit(seqnum, 0).encode())
        .await
        .unwrap();
    read_header(driver).await == ret_submit(seqnum, 0)
}

/// Polls the session's outbox until it holds something.
async fn next(forwarding: &UsbForwarding) -> Vec<UsbMessage> {
    for _ in 0..500 {
        let messages = forwarding.take(SESSION, Instant::now());
        if !messages.is_empty() {
            return messages;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no USB message for the client");
}

struct Relay {
    forwarding: Arc<UsbForwarding>,
    vhci: Arc<FakeVhci>,
    driver: TcpStream,
    client: DuplexStream,
    served: tokio::task::JoinHandle<()>,
}

/// A device attached through the relay, with the driver's and the client's ends.
async fn attached() -> Relay {
    attached_with(Script::default()).await
}

async fn attached_with(script: Script) -> Relay {
    let (vhci, connections) = FakeVhci::scripted(true, script);
    let forwarding = forwarding(vhci.clone());
    let token = asked(&forwarding, 5);
    let opened = forwarding
        .claim(&token, "cert-phone", Instant::now())
        .unwrap();
    let (tunnel, mut client) = tokio::io::duplex(1 << 16);
    let served = tokio::spawn({
        let forwarding = forwarding.clone();
        async move { forwarding.serve(opened, tunnel).await }
    });
    serve_import(&mut client, VID).await;
    assert!(matches!(
        next(&forwarding).await[..],
        [UsbMessage::Status {
            request_id: 5,
            state: UsbState::Attached,
            reason: UsbReason::None
        }]
    ));
    let listed = &forwarding.list()[0];
    assert_eq!((listed.state, listed.port), ("attached", Some(3)));
    let driver = connections.recv_timeout(Duration::from_secs(5)).unwrap();
    driver.set_nonblocking(true).unwrap();
    Relay {
        forwarding,
        vhci,
        driver: TcpStream::from_std(driver).unwrap(),
        client,
        served,
    }
}

#[test]
fn the_driver_install_check_is_reused_for_a_while() {
    let (vhci, _) = FakeVhci::new(true);
    let forwarding = forwarding(vhci.clone());
    for request_id in 1..=3 {
        forwarding.on_message(SESSION, &client(true), true, 4, request(request_id, BUSID));
        assert!(forwarding.ready());
    }
    assert_eq!(vhci.checks.load(Ordering::Relaxed), 1);
}
