//! Test helpers exposed by `butterpollo-webrtc` under `cfg(test)` or `feature = "test-support"`.

pub use crate::input::Recorder;
pub use crate::sdp::testing as sdp;
pub use crate::server::{Console, Limits, Running, start_with};

use serde_json::Value;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

/// A minimal WebSocket client: enough to talk to the server in tests.
pub struct Ws {
    stream: TcpStream,
    /// The response to the upgrade request.
    pub head: String,
    /// The code and reason in the server's close frame, once it sent one with them.
    pub close_code: Option<u16>,
    pub close_reason: Option<String>,
}

impl Ws {
    pub async fn connect(address: SocketAddr) -> Self {
        Self::connect_with(address, "/", "").await
    }

    /// Upgrade `path`, adding `headers` (each line ending in `\r\n`) to the request.
    pub async fn connect_with(address: SocketAddr, path: &str, headers: &str) -> Self {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{headers}\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(stream.read_u8().await.unwrap());
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        Self {
            stream,
            head,
            close_code: None,
            close_reason: None,
        }
    }

    pub async fn send(&mut self, message: Value) {
        self.send_text(message.to_string().into_bytes()).await;
    }

    /// One masked text frame carrying `payload` as it is.
    pub async fn send_text(&mut self, payload: Vec<u8>) {
        let mut frame = vec![0x81u8];
        match payload.len() {
            n if n < 126 => frame.push(0x80 | n as u8),
            n if n <= u16::MAX as usize => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        // The server may close the socket before it has read it all.
        let _ = self.stream.write_all(&frame).await;
    }

    /// The next message; None once the server closed the socket.
    pub async fn next(&mut self) -> Option<Value> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let first = self.stream.read_u8().await.ok()?;
                let second = self.stream.read_u8().await.ok()?;
                let length = match second & 0x7f {
                    126 => self.stream.read_u16().await.ok()? as usize,
                    127 => self.stream.read_u64().await.ok()? as usize,
                    n => n as usize,
                };
                let mut payload = vec![0; length];
                self.stream.read_exact(&mut payload).await.ok()?;
                match first & 0x0f {
                    1 => return serde_json::from_slice(&payload).ok(),
                    8 => {
                        self.close_code =
                            payload.get(..2).map(|c| u16::from_be_bytes([c[0], c[1]]));
                        self.close_reason = payload
                            .get(2..)
                            .map(|r| String::from_utf8_lossy(r).into_owned());
                        return None;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("a message from the server")
    }

    /// The next message, which must have this type.
    pub async fn expect(&mut self, kind: &str) -> Value {
        let message = self
            .next()
            .await
            .unwrap_or_else(|| panic!("closed, wanted {kind}"));
        assert_eq!(message["type"], kind, "{message}");
        assert_eq!(message["version"], 2);
        message
    }

    pub async fn closed(&mut self) {
        assert!(
            self.next().await.is_none(),
            "the socket should have been closed"
        );
    }
}

/// A real MediaHandle on loopback, against a peer that never answers.
pub fn real_handle(
    events: std::sync::Arc<std::sync::Mutex<Vec<crate::media::TransportEvent>>>,
) -> (crate::media::MediaHandle, std::net::SocketAddr) {
    use std::sync::{Arc, atomic::AtomicBool};
    let spec = crate::sdp::testing::spec(crate::sdp::VideoCodec::H264, false);
    let socket = crate::media::bind_socket("127.0.0.1:0".parse().unwrap()).unwrap();
    let local = socket.local_addr().unwrap();
    let (mut rtc, offer) = crate::sdp::testing::host_offer(&spec, local);
    let mut peer = crate::sdp::testing::client(&spec, "192.168.1.20:50000".parse().unwrap());
    let text = crate::sdp::testing::answer(&mut peer, &offer.sdp);
    let (ids, _) = crate::sdp::accept_answer(&mut rtc, offer, &text, &spec).unwrap();
    let session = crate::media::Session {
        rtc,
        ids,
        spec: &spec,
        authenticated: Arc::new(AtomicBool::new(false)),
    };
    let hooks = crate::media::Hooks::new(|_| {}, move |e| events.lock().unwrap().push(e));
    let handle = crate::media::MediaHandle::spawn(
        socket,
        local,
        session,
        crate::media::TransportOptions::default(),
        hooks,
    )
    .unwrap();
    (handle, local)
}
