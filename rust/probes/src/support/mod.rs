//! Internal capture-worker dispatch for probes that create a Capture.
use anyhow::Result;
use butterpollo_win_graphics::user_helper::{HelperPipe, PipedUserChild, UserChild, UserLauncher};
use butterpollo_win_io::ipc::Pipe;
use butterpollo_win_io::process::{Process, Target};
use std::{
    collections::BTreeMap, ffi::OsString, os::windows::io::OwnedHandle, path::Path, time::Duration,
};

struct ProbePipe(Pipe);
impl HelperPipe for ProbePipe {
    fn connected(&self, expected_pid: u32) -> Result<bool> {
        self.0.connected(expected_pid)
    }
    fn send_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.0.send_bytes(bytes)
    }
    fn receive_bytes(&self) -> Result<Option<Vec<u8>>> {
        self.0.receive_bytes()
    }
}

struct ProbeChild(Process);
impl UserChild for ProbeChild {
    fn pid(&self) -> u32 {
        self.0.pid
    }
    fn exit_code(&self) -> Result<Option<u32>> {
        self.0.exit_code()
    }
    fn wait(&self, timeout: Duration) -> Result<u32> {
        self.0.wait(timeout)
    }
    fn terminate(&self) -> Result<()> {
        self.0.terminate()
    }
    fn duplicate_into(&self, raw_handle: usize) -> Result<u64> {
        self.0.duplicate_into_raw(raw_handle)
    }
    fn duplicate_resource(&self, raw_handle: usize) -> Result<OwnedHandle> {
        self.0.duplicate_resource(raw_handle)
    }
}

struct ProbeLauncher;
impl UserLauncher for ProbeLauncher {
    fn pipe_server(&self, prefix: &str) -> Result<(Box<dyn HelperPipe>, String)> {
        let (pipe, name) = Pipe::server(prefix)?;
        Ok((Box::new(ProbePipe(pipe)), name))
    }
    fn pipe_client(&self, name: &str, parent: u32, prefix: &str) -> Result<Box<dyn HelperPipe>> {
        Ok(Box::new(ProbePipe(Pipe::client(name, parent, prefix)?)))
    }
    fn spawn_user(&self, program: &Path, args: &[OsString]) -> Result<Box<dyn UserChild>> {
        Ok(Box::new(ProbeChild(Process::spawn(
            program,
            args,
            None,
            Target::User { elevated: false },
            &BTreeMap::new(),
            true,
        )?)))
    }
    fn spawn_user_piped(&self, program: &Path, args: &[OsString]) -> Result<PipedUserChild> {
        let child = Process::spawn_piped(program, args, Target::User { elevated: false })?;
        Ok(PipedUserChild {
            process: Box::new(ProbeChild(child.process)),
            stdin: child.stdin,
            stdout: child.stdout,
            stderr: child.stderr,
        })
    }
    fn user_sid(&self) -> Option<String> {
        butterpollo_win_io::process::user_sid()
    }
}

pub fn wgc_worker() -> Option<Result<()>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("--wgc-worker") {
        return None;
    }
    Some((|| {
        anyhow::ensure!(
            args.len() == 4 && args[2] == "--wgc-parent",
            "invalid WGC worker arguments"
        );
        butterpollo_win_graphics::capture::enable_dpi_awareness();
        butterpollo_win_graphics::capture::run_wgc_worker_with(
            &args[1],
            args[3].parse()?,
            &ProbeLauncher,
        )
    })())
}
