//! Tests for private named-pipe framing and peer identity checks.
use super::*;
use serde::{Deserialize, Serialize};
use windows::Win32::System::Threading::GetCurrentProcessId;

const PIPE_PREFIX: &str = r"\\.\pipe\Butterpollo.IpcTest.";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum Request {
    Release { slot: usize },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum Reply {
    Frame {
        slot: usize,
        sequence: u64,
        qpc: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<bool>,
    },
}

#[test]
fn private_pipe_checks_peers_bounds_messages_and_reports_disconnect() -> Result<()> {
    let (server, name) = Pipe::server(PIPE_PREFIX)?;
    // SAFETY: GetCurrentProcessId has no preconditions.
    let pid = unsafe { GetCurrentProcessId() };
    assert!(Pipe::client(&name, pid.wrapping_add(1), PIPE_PREFIX).is_err());
    // The rejected client closes its endpoint; use a fresh single-instance
    // server, just as failed pipe startup does.
    drop(server);
    let (server, name) = Pipe::server(PIPE_PREFIX)?;
    let client = Pipe::client(&name, pid, PIPE_PREFIX)?;
    assert!(server.connected(pid)?);
    assert!(server.connected(pid.wrapping_add(1)).is_err());
    assert!(server.receive::<Request>()?.is_none());
    client.send(&Request::Release { slot: 2 })?;
    assert!(matches!(
        server.receive::<Request>()?,
        Some(Request::Release { slot: 2 })
    ));
    assert!(server.receive::<Request>()?.is_none());
    server.send(&Reply::Frame {
        slot: 1,
        sequence: 15,
        qpc: 1234,
        focus: Some(false),
    })?;
    assert!(matches!(
        client.receive::<Reply>()?,
        Some(Reply::Frame {
            slot: 1,
            sequence: 15,
            qpc: 1234,
            focus: Some(false)
        })
    ));
    let unlabelled = serde_json::to_string(&Reply::Frame {
        slot: 2,
        sequence: 16,
        qpc: 1235,
        focus: None,
    })
    .unwrap();
    assert_eq!(
        unlabelled,
        r#"{"type":"Frame","slot":2,"sequence":16,"qpc":1235}"#
    );
    assert!(matches!(
        serde_json::from_str(&unlabelled).unwrap(),
        Reply::Frame { focus: None, .. }
    ));
    assert!(client.send(&"x".repeat(MESSAGE_LIMIT)).is_err());
    assert!(server.receive::<Request>()?.is_none());
    drop(client);
    assert!(server.receive::<Request>().is_err());
    Ok(())
}
