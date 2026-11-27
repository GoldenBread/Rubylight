//! Tests of the broker's pieces that need no DRM card: who may connect, the messages, the
//! descriptors that travel with them, and the client against a stand-in broker.
use super::*;
use crate::{client::Client, fdpass, protocol::Request};
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};

fn peer(uid: u32, gid: u32, groups: &[u32]) -> Peer {
    Peer {
        pid: 1,
        uid,
        gid,
        groups: groups.to_vec(),
    }
}

#[test]
fn only_root_and_the_group_may_capture() {
    assert!(peer(0, 0, &[]).may_capture(None));
    assert!(peer(1000, 1000, &[998, 961]).may_capture(Some(961)));
    assert!(peer(1000, 961, &[]).may_capture(Some(961)));
    assert!(!peer(1000, 1000, &[998]).may_capture(Some(961)));
    // Without the group on the system, nobody but root.
    assert!(!peer(1000, 1000, &[961]).may_capture(None));
}

#[test]
fn messages_read_back_as_they_were_written() {
    let request = Request::Frame {
        output: "DP-1".into(),
        fresh: true,
        wait_ms: 250,
    };
    assert_eq!(Request::decode(&request.encode()), Some(request));
    assert_eq!(
        String::from_utf8(Request::Outputs.encode()).unwrap(),
        r#"{"op":"outputs"}"#
    );
    let frame = Reply::Frame(FrameInfo {
        output: OutputInfo {
            name: "DP-1".into(),
            width: 1920,
            height: 1080,
            refresh_millihz: 164_997,
        },
        fb_id: 456,
        width: 1920,
        height: 1080,
        fourcc: 0x3033_5241,
        modifier: 0x0200_0000_2086_7b04,
        planes: vec![
            PlaneInfo {
                pitch: 7680,
                offset: 0,
            };
            3
        ],
        cursor: Some(CursorInfo {
            fb_id: 452,
            x: -3,
            y: 700,
            width: 64,
            height: 64,
            fourcc: 0x3432_5241,
            modifier: 0,
            pitch: 256,
            offset: 0,
        }),
    });
    assert_eq!(frame.fds(), 4);
    assert_eq!(Reply::decode(&frame.encode()), Some(frame));
    assert_eq!(Reply::Timeout.fds(), 0);
    assert_eq!(Request::decode(b"{\"op\":\"rm -rf\"}"), None);
}

/// A pipe whose reading end travels; what is written into the kept end comes out of it.
fn pipe() -> (OwnedFd, std::fs::File) {
    let (reader, writer) = std::io::pipe().unwrap();
    (
        OwnedFd::from(reader),
        std::fs::File::from(OwnedFd::from(writer)),
    )
}

#[test]
fn descriptors_travel_with_their_message() {
    let (a, b) = fdpass::pair().unwrap();
    let (reader, mut writer) = pipe();
    fdpass::send(a.as_fd(), b"one", &[reader.as_fd()]).unwrap();
    drop(reader);
    let (bytes, fds) = fdpass::receive(b.as_fd()).unwrap().unwrap();
    assert_eq!(bytes, b"one");
    assert_eq!(fds.len(), 1);
    writer.write_all(b"through").unwrap();
    drop(writer);
    let mut text = String::new();
    std::fs::File::from(fds.into_iter().next().unwrap())
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "through");
    // Too many descriptors, and the other side closing.
    let many: Vec<_> = (0..6).map(|_| a.as_fd()).collect();
    assert!(fdpass::send(a.as_fd(), b"x", &many).is_err());
    drop(a);
    assert!(fdpass::receive(b.as_fd()).unwrap().is_none());
}

#[test]
fn the_client_takes_frames_and_their_descriptors() {
    let (host, broker) = fdpass::pair().unwrap();
    let stand_in = std::thread::spawn(move || {
        // A frame of two planes and a cursor, a timeout, then a refusal.
        let (bytes, _) = fdpass::receive(broker.as_fd()).unwrap().unwrap();
        assert!(matches!(
            Request::decode(&bytes),
            Some(Request::Frame { fresh: false, .. })
        ));
        let mut info = FrameInfo {
            output: OutputInfo {
                name: "Virtual-1".into(),
                width: 1920,
                height: 1080,
                refresh_millihz: 60_000,
            },
            fb_id: 48,
            width: 1920,
            height: 1080,
            fourcc: 0x3432_5258,
            modifier: 0,
            planes: vec![
                PlaneInfo {
                    pitch: 7680,
                    offset: 0
                };
                2
            ],
            cursor: None,
        };
        info.cursor = Some(CursorInfo {
            fb_id: 49,
            x: 10,
            y: 20,
            width: 64,
            height: 64,
            fourcc: 0x3432_5241,
            modifier: 0,
            pitch: 256,
            offset: 0,
        });
        let pipes: Vec<_> = (0..3).map(|_| pipe()).collect();
        let fds: Vec<_> = pipes.iter().map(|(r, _)| r.as_fd()).collect();
        fdpass::send(broker.as_fd(), &Reply::Frame(info).encode(), &fds).unwrap();
        fdpass::receive(broker.as_fd()).unwrap().unwrap();
        fdpass::send(broker.as_fd(), &Reply::Timeout.encode(), &[]).unwrap();
        fdpass::receive(broker.as_fd()).unwrap().unwrap();
        let message = "only root and the rubylight group may capture the screen".into();
        fdpass::send(broker.as_fd(), &Reply::Error { message }.encode(), &[]).unwrap();
        // A frame that names a cursor but comes without its descriptor.
        fdpass::receive(broker.as_fd()).unwrap().unwrap();
        let (r, _) = pipe();
        fdpass::send(
            broker.as_fd(),
            &Reply::Frame(bare_frame()).encode(),
            &[r.as_fd()],
        )
        .unwrap();
    });
    let client = Client::from_socket(host);
    let frame = client
        .frame("Virtual-1", false, Duration::from_millis(100))
        .unwrap()
        .unwrap();
    assert_eq!(frame.info.fb_id, 48);
    assert_eq!(frame.planes.len(), 2);
    assert!(frame.cursor.is_some());
    assert!(
        client
            .frame("Virtual-1", true, Duration::from_millis(100))
            .unwrap()
            .is_none()
    );
    let refused = client.outputs().unwrap_err();
    assert!(format!("{refused:#}").contains("rubylight group"));
    // It names no descriptor but one came.
    let mismatch = client
        .frame("Virtual-1", false, Duration::ZERO)
        .unwrap_err();
    assert!(format!("{mismatch:#}").contains("descriptors"));
    stand_in.join().unwrap();
}

/// A frame without planes or cursor.
fn bare_frame() -> FrameInfo {
    FrameInfo {
        output: OutputInfo {
            name: "X".into(),
            width: 1,
            height: 1,
            refresh_millihz: 1,
        },
        fb_id: 1,
        width: 1,
        height: 1,
        fourcc: 0,
        modifier: 0,
        planes: vec![],
        cursor: None,
    }
}
