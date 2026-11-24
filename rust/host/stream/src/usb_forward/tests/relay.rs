//! Async relay, framing validation, and driver detach tests for USB forwarding.

use super::*;
use rubylight_protocol::usbip::Header;
use std::io::Read;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_carries_transfers_and_unlinks_and_the_console_detaches() {
    let Relay {
        forwarding,
        vhci,
        mut driver,
        mut client,
        served,
    } = attached_with(Script {
        watch_driver: true,
        ..Script::default()
    })
    .await;

    // An IN transfer: the client answers with less than the driver's buffer.
    driver
        .write_all(&submit(1, wire::DIR_IN, 512).encode())
        .await
        .unwrap();
    assert_eq!(read_header(&mut client).await, submit(1, wire::DIR_IN, 512));
    client.write_all(&ret_submit(1, 13).encode()).await.unwrap();
    client.write_all(b"hello, driver").await.unwrap();
    assert_eq!(read_header(&mut driver).await, ret_submit(1, 13));
    let mut data = [0; 13];
    driver.read_exact(&mut data).await.unwrap();
    assert_eq!(&data, b"hello, driver");

    // An OUT transfer: its data goes to the client, the reply carries none.
    driver
        .write_all(&submit(2, wire::DIR_OUT, 4).encode())
        .await
        .unwrap();
    driver.write_all(b"ping").await.unwrap();
    assert_eq!(read_header(&mut client).await, submit(2, wire::DIR_OUT, 4));
    let mut data = [0; 4];
    client.read_exact(&mut data).await.unwrap();
    assert_eq!(&data, b"ping");
    client.write_all(&ret_submit(2, 4).encode()).await.unwrap();
    assert_eq!(read_header(&mut driver).await, ret_submit(2, 4));

    // A cancelled transfer: the unlink answers for it.
    driver
        .write_all(&submit(3, wire::DIR_IN, 64).encode())
        .await
        .unwrap();
    let unlink = Header::Unlink {
        base: base(4, wire::DIR_OUT, 1),
        unlink: 3,
    };
    driver.write_all(&unlink.encode()).await.unwrap();
    assert_eq!(read_header(&mut client).await, submit(3, wire::DIR_IN, 64));
    assert_eq!(read_header(&mut client).await, unlink);
    let reset = Header::RetUnlink {
        base: base(4, 0, 0),
        status: wire::ECONNRESET,
    };
    client.write_all(&reset.encode()).await.unwrap();
    assert_eq!(read_header(&mut driver).await, reset);

    // An unlink answered with any other status ends its transfer too: no RET_SUBMIT follows.
    for (seqnum, status) in [(5, 0), (7, wire::ENOENT), (9, wire::ESHUTDOWN)] {
        let unlink = Header::Unlink {
            base: base(seqnum + 1, wire::DIR_OUT, 1),
            unlink: seqnum,
        };
        driver
            .write_all(&submit(seqnum, wire::DIR_IN, 64).encode())
            .await
            .unwrap();
        driver.write_all(&unlink.encode()).await.unwrap();
        read_header(&mut client).await;
        read_header(&mut client).await;
        let answer = Header::RetUnlink {
            base: base(seqnum + 1, 0, 0),
            status,
        };
        client.write_all(&answer.encode()).await.unwrap();
        assert_eq!(read_header(&mut driver).await, answer);
        // A late reply to the unlinked transfer finds nothing in flight.
        assert!(
            relay_flight_is_empty(&mut driver, &mut client, seqnum).await,
            "status {status} left seqnum {seqnum} in flight"
        );
    }

    // An isochronous IN transfer: packed data and packets that keep the submitted layout.
    let asked = [iso(0, 64, 0), iso(64, 64, 0)];
    driver
        .write_all(&iso_submit(20, wire::DIR_IN, 128, &asked))
        .await
        .unwrap();
    let mut submitted = vec![0; wire::HEADER_BYTES + 2 * wire::ISO_PACKET_BYTES];
    client.read_exact(&mut submitted).await.unwrap();
    let replied = [iso(0, 64, 60), iso(64, 64, 64)];
    let reply = iso_reply(20, &[0x5a; 124], &replied);
    client.write_all(&reply).await.unwrap();
    let mut relayed = vec![0; reply.len()];
    driver.read_exact(&mut relayed).await.unwrap();
    assert_eq!(relayed, reply);

    // The console detaches it: the relay stops, the driver lets go, the client hears once.
    let id = forwarding.list()[0].id;
    assert!(forwarding.detach(id).unwrap());
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vhci.detached(), [(3, BUSID.to_owned())]);
    assert_eq!(
        *vhci.open_at_detach.lock().unwrap(),
        [true],
        "the relay keeps the driver's connection until the driver detached"
    );
    assert!(forwarding.list().is_empty());
    assert!(matches!(
        forwarding.take(SESSION, Instant::now())[..],
        [
            UsbMessage::Detach { request_id: 5 },
            UsbMessage::Status {
                request_id: 5,
                state: UsbState::Detached,
                reason: UsbReason::None
            }
        ]
    ));
    let mut rest = Vec::new();
    client.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty(), "the tunnel closes with the relay");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attached_device_detaches_when_its_session_ends() {
    // Both ends stay open: only the session end detaches.
    let Relay {
        forwarding,
        vhci,
        driver: _driver,
        client: _client,
        served,
    } = attached().await;
    assert!(matches!(
        forwarding.end_session(SESSION)[..],
        [
            UsbMessage::Detach { request_id: 5 },
            UsbMessage::Status {
                request_id: 5,
                state: UsbState::Detached,
                reason: UsbReason::SessionEnd
            }
        ]
    ));
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vhci.detached(), [(3, BUSID.to_owned())]);
    assert!(forwarding.list().is_empty());
    assert!(
        forwarding.take(SESSION, Instant::now()).is_empty(),
        "the session end already told the client"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_usb_forwarding_off_detaches_every_device_now() {
    let Relay {
        forwarding,
        vhci,
        driver: _driver,
        client: _client,
        served,
    } = attached().await;
    // Another client's device still waits for its tunnel.
    forwarding.on_message("other", &client(true), true, 4, request(1, BUSID));
    token(&forwarding.take("other", Instant::now()));
    assert_eq!(forwarding.disable(), 2);
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vhci.detached(), [(3, BUSID.to_owned())]);
    assert!(forwarding.list().is_empty());
    for (session, request_id) in [(SESSION, 5), ("other", 1)] {
        let told = forwarding.take(session, Instant::now());
        assert!(
            matches!(
                told[..],
                [
                    UsbMessage::Detach { request_id: r },
                    UsbMessage::Status {
                        request_id: s,
                        state: UsbState::Detached,
                        reason: UsbReason::Disabled
                    }
                ] if r == request_id && s == request_id
            ),
            "{session}: {told:?}"
        );
    }
    assert_eq!(forwarding.disable(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_detaches_a_client_that_breaks_the_framing() {
    for reply in [
        // A reply to nothing the driver asked.
        ret_submit(9, 0),
        // More IN data than the driver's buffer holds.
        ret_submit(1, 4096),
    ] {
        let Relay {
            forwarding,
            vhci,
            mut driver,
            mut client,
            served,
        } = attached().await;
        driver
            .write_all(&submit(1, wire::DIR_IN, 512).encode())
            .await
            .unwrap();
        read_header(&mut client).await;
        client.write_all(&reply.encode()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), served)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(vhci.detached(), [(3, BUSID.to_owned())]);
        assert!(matches!(
            forwarding.take(SESSION, Instant::now())[..],
            [UsbMessage::Status {
                request_id: 5,
                state: UsbState::Failed,
                reason: UsbReason::DeviceError
            }]
        ));
        let mut rest = Vec::new();
        assert_eq!(
            driver.read_to_end(&mut rest).await.unwrap(),
            0,
            "nothing reaches the driver"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_client_closing_its_tunnel_detaches_the_device() {
    let Relay {
        forwarding,
        vhci,
        driver: _driver,
        client,
        served,
    } = attached().await;
    drop(client);
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vhci.detached(), [(3, BUSID.to_owned())]);
    assert!(matches!(
        forwarding.take(SESSION, Instant::now())[..],
        [UsbMessage::Status {
            request_id: 5,
            state: UsbState::Detached,
            reason: UsbReason::None
        }]
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_driver_letting_go_fails_the_attachment() {
    let Relay {
        forwarding,
        vhci,
        driver,
        client: _client,
        served,
    } = attached().await;
    drop(driver);
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vhci.detached(), [(3, BUSID.to_owned())]);
    assert!(matches!(
        forwarding.take(SESSION, Instant::now())[..],
        [UsbMessage::Status {
            request_id: 5,
            state: UsbState::Failed,
            reason: UsbReason::Driver
        }]
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_serving_another_device_is_refused() {
    let (vhci, _connections) = FakeVhci::new(true);
    let forwarding = forwarding(vhci.clone());
    let token = asked(&forwarding, 6);
    let opened = forwarding
        .claim(&token, "cert-phone", Instant::now())
        .unwrap();
    let (tunnel, mut client) = tokio::io::duplex(1 << 16);
    let served = tokio::spawn({
        let forwarding = forwarding.clone();
        async move { forwarding.serve(opened, tunnel).await }
    });
    serve_import(&mut client, 0x046d).await;
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        forwarding.take(SESSION, Instant::now())[..],
        [UsbMessage::Status {
            request_id: 6,
            state: UsbState::Failed,
            reason: UsbReason::DeviceError
        }]
    ));
    assert!(forwarding.list().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_detaches_a_client_whose_iso_packets_do_not_fit() {
    let asked = [iso(0, 64, 0), iso(64, 64, 0)];
    for (case, data, packets) in [
        (
            "the sum differs from the data",
            124,
            [iso(0, 64, 60), iso(64, 64, 60)],
        ),
        ("a packet moved", 124, [iso(4, 64, 60), iso(64, 64, 64)]),
        ("a packet resized", 124, [iso(0, 128, 60), iso(64, 64, 64)]),
        (
            "longer than its slot",
            129,
            [iso(0, 64, 65), iso(64, 64, 64)],
        ),
    ] {
        let Relay {
            forwarding,
            vhci,
            mut driver,
            mut client,
            served,
        } = attached().await;
        driver
            .write_all(&iso_submit(1, wire::DIR_IN, 192, &asked))
            .await
            .unwrap();
        let mut submitted = vec![0; wire::HEADER_BYTES + 2 * wire::ISO_PACKET_BYTES];
        client.read_exact(&mut submitted).await.unwrap();
        client
            .write_all(&iso_reply(1, &vec![0; data], &packets))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), served)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(vhci.detached(), [(3, BUSID.to_owned())], "{case}");
        assert!(
            matches!(
                forwarding.take(SESSION, Instant::now())[..],
                [UsbMessage::Status {
                    state: UsbState::Failed,
                    reason: UsbReason::DeviceError,
                    ..
                }]
            ),
            "{case}"
        );
        let mut rest = Vec::new();
        assert_eq!(
            driver.read_to_end(&mut rest).await.unwrap(),
            0,
            "{case}: nothing reaches the driver"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_turns_away_a_process_that_is_not_the_driver() {
    let Relay {
        forwarding: _forwarding,
        vhci,
        driver: _driver,
        client: _client,
        served: _served,
    } = attached_with(Script {
        stranger: true,
        ..Script::default()
    })
    .await;
    assert_eq!(vhci.rejected.lock().unwrap().len(), 1);
    let mut stranger = vhci.stranger.lock().unwrap().take().unwrap();
    stranger
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut got = [0; wire::OP_HEADER_BYTES];
    assert!(
        matches!(stranger.read(&mut got), Ok(0) | Err(_)),
        "the stranger is dropped without the import"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_attach_after_the_import_stops_the_driver() {
    let (vhci, connections) = FakeVhci::scripted(
        true,
        Script {
            attach_fails: true,
            ..Script::default()
        },
    );
    let forwarding = forwarding(vhci.clone());
    let token = asked(&forwarding, 6);
    let opened = forwarding
        .claim(&token, "cert-phone", Instant::now())
        .unwrap();
    let (tunnel, mut client) = tokio::io::duplex(1 << 16);
    let served = tokio::spawn({
        let forwarding = forwarding.clone();
        async move { forwarding.serve(opened, tunnel).await }
    });
    serve_import(&mut client, VID).await;
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        forwarding.take(SESSION, Instant::now())[..],
        [UsbMessage::Status {
            request_id: 6,
            state: UsbState::Failed,
            reason: UsbReason::Driver
        }]
    ));
    assert!(vhci.detached().is_empty(), "no hub port to detach");
    assert_eq!(vhci.stopped(), [BUSID.to_owned()]);
    assert!(forwarding.list().is_empty());
    let mut driver = connections.recv_timeout(Duration::from_secs(5)).unwrap();
    driver
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    assert_eq!(
        driver.read(&mut [0; 1]).unwrap(),
        0,
        "the relay dropped the driver's connection"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_the_driver_keeps_stays_listed_until_the_console_detaches_it() {
    let Relay {
        forwarding,
        vhci,
        driver: _driver,
        client: device,
        served,
    } = attached_with(Script {
        failed_detaches: 2,
        ..Script::default()
    })
    .await;
    drop(device);
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    // The client hears that it failed, not that it detached.
    assert!(matches!(
        forwarding.take(SESSION, Instant::now())[..],
        [UsbMessage::Status {
            request_id: 5,
            state: UsbState::Failed,
            reason: UsbReason::Driver
        }]
    ));
    let listed = forwarding.list();
    assert_eq!(
        (listed[0].state, listed[0].port),
        ("detach_failed", Some(3))
    );
    assert!(
        listed[0]
            .error
            .as_deref()
            .unwrap()
            .contains("detach --port 3")
    );
    // It holds a hub port, so it counts against the limit; its token stays spent.
    forwarding.on_message("session-b", &client(true), true, 1, request(1, BUSID));
    assert_eq!(
        refusal(&forwarding.take("session-b", Instant::now())),
        Some(UsbReason::Limit)
    );
    // The session ending leaves it to the console.
    assert!(forwarding.end_session(SESSION).is_empty());
    let id = listed[0].id;
    let again = forwarding.detach(id).unwrap_err();
    assert!(format!("{again:#}").contains("still holds the device"));
    assert_eq!(forwarding.list()[0].state, "detach_failed");
    assert!(forwarding.detach(id).unwrap());
    assert!(forwarding.list().is_empty());
    assert_eq!(vhci.detached().len(), 3);
    assert!(forwarding.take(SESSION, Instant::now()).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_driver_that_let_go_is_only_stopped_when_its_port_is_gone() {
    let Relay {
        forwarding,
        vhci,
        driver,
        client: _client,
        served,
    } = attached_with(Script {
        failed_detaches: 1,
        ..Script::default()
    })
    .await;
    drop(driver);
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vhci.detached(), [(3, BUSID.to_owned())]);
    assert_eq!(vhci.stopped(), [BUSID.to_owned()]);
    assert!(forwarding.list().is_empty());
    assert!(matches!(
        forwarding.take(SESSION, Instant::now())[..],
        [UsbMessage::Status {
            request_id: 5,
            state: UsbState::Failed,
            reason: UsbReason::Driver
        }]
    ));
}
