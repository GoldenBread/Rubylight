//! The driver settings helper process: the host exe started with the hidden `--radeon-helper` flag reads one request line, answers it and exits. On Linux the helper speaks the same protocol, but no driver settings API is available yet, so it answers every request with an error and the host never starts it.
use super::{Driver, RadeonBackend, Setting, absent::NoDriver, model::Value};
use serde::{Deserialize, Serialize};
use std::io::BufRead;

/// The hidden command-line flag that makes the host exe act as the helper.
pub const FLAG: &str = "--radeon-helper";
/// Marks the helper's answer; anything else it prints, even earlier on the same line, is
/// ignored.
const MARKER: &str = "RADEON-HELPER-RESPONSE ";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(super) enum Request {
    Read { gpu: i32 },
    Write { gpu: i32, id: String, value: Value },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Response {
    Settings { settings: Vec<Setting> },
    Done,
    Error { message: String },
}

/// Answer one request against a backend. A request that does not parse is an error response, not
/// a panic.
pub(super) fn handle(request: &str, backend: &mut impl RadeonBackend) -> Response {
    let request: Request = match serde_json::from_str(request) {
        Ok(request) => request,
        Err(error) => {
            return Response::Error {
                message: format!("unreadable helper request: {error}"),
            };
        }
    };
    let answer = match request {
        Request::Read { gpu } => backend
            .read(gpu)
            .map(|settings| Response::Settings { settings }),
        Request::Write { gpu, id, value } => {
            backend.write(gpu, &id, &value).map(|()| Response::Done)
        }
    };
    answer.unwrap_or_else(|error| Response::Error { message: error.0 })
}

/// The helper's entry point (`--radeon-helper`): read one request from stdin and print one
/// answer line. On Linux the answer to a readable request is always an error saying that the
/// driver settings API is not available. The exit code is non-zero only when no answer could be
/// produced.
pub fn run() -> i32 {
    run_with(&mut NoDriver(Driver::Radeon))
}

// The helper talks to its parent only through stdin, stdout and stderr.
#[allow(clippy::print_stdout, clippy::print_stderr)]
fn run_with(backend: &mut impl RadeonBackend) -> i32 {
    // Exactly one request line: the helper never waits for stdin to close, because another
    // process that inherited a copy of the host's end can keep it open.
    let mut request = String::new();
    if let Err(error) = std::io::stdin().lock().read_line(&mut request) {
        eprintln!("radeon helper: cannot read its request: {error}");
        return 2;
    }
    let response = handle(&request, backend);
    match serde_json::to_string(&response) {
        Ok(line) => {
            println!("{MARKER}{line}");
            0
        }
        Err(error) => {
            eprintln!("radeon helper: cannot encode its answer: {error}");
            3
        }
    }
}
