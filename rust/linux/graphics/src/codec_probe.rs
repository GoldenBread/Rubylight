//! The optional codec probes (PyroWave and packed 4:4:4) and their typed failures. Linux has no hardware encoder to probe yet, so each probe fails at once with an error that is none of the retryable ones.
use crate::user_helper::UserLauncher;
use anyhow::{Result, ensure};
use butterpollo_config::config::Config;
use butterpollo_media_policy::rtsp::Negotiated;

/// The probe could not run in the signed-in user's session yet. Probing again
/// later can succeed.
#[derive(Debug)]
pub struct SessionNotReady(String);
impl std::fmt::Display for SessionNotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SessionNotReady {}

/// The probe did not answer within its time limit, as when other work keeps
/// the CPU busy. Probing again once the host is idle can succeed.
#[derive(Debug)]
pub struct TimedOut;
impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("optional codec probe timed out")
    }
}
impl std::error::Error for TimedOut {}

/// The caller stopped the probe, as when a stream wants the encoder.
#[derive(Debug)]
pub struct Interrupted;
impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("optional codec probe stopped for a stream")
    }
}
impl std::error::Error for Interrupted {}

/// The probe's worker ended before it replied, with the exit code in the
/// message. A few more tries can find the codec.
#[derive(Debug)]
pub struct Crashed(String);
impl std::fmt::Display for Crashed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Crashed {}

impl SessionNotReady {
    /// For a fake probe in tests.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}
impl Crashed {
    /// For a fake probe in tests.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// Probe PyroWave. On Linux no worker is started and `stop` is never asked:
/// the probe fails with a plain error, which the host does not try again.
pub fn pyrowave_with(
    _config: &Config,
    _stop: &dyn Fn() -> bool,
    _launcher: &dyn UserLauncher,
) -> Result<u32> {
    Err(crate::unsupported("PyroWave encoding"))
}

/// Probe one packed 4:4:4 mode. An invalid request is refused as such; a valid
/// one fails with a plain error, so clients are not offered the mode.
pub fn packed_with(
    _config: &Config,
    stream: &Negotiated,
    _stop: &dyn Fn() -> bool,
    _launcher: &dyn UserLauncher,
) -> Result<()> {
    stream.validate()?;
    ensure!(
        rubylight_protocol::packed444::supported(stream.chroma_packing),
        "invalid packed probe request"
    );
    Err(crate::unsupported("Packed 4:4:4 encoding"))
}

/// The probe's worker process. The Linux host never starts one; run by hand it
/// reports that there is nothing to probe.
pub fn worker_with(_name: &str, _parent: u32, _launcher: &dyn UserLauncher) -> Result<()> {
    Err(crate::unsupported("The optional codec probe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_probe_fails_with_an_error_the_host_does_not_retry() {
        struct NoLauncher;
        impl UserLauncher for NoLauncher {
            fn pipe_server(
                &self,
                _: &str,
            ) -> Result<(Box<dyn crate::user_helper::HelperPipe>, String)> {
                anyhow::bail!("a probe must not open a pipe")
            }
            fn pipe_client(
                &self,
                _: &str,
                _: u32,
                _: &str,
            ) -> Result<Box<dyn crate::user_helper::HelperPipe>> {
                anyhow::bail!("a probe must not open a pipe")
            }
            fn spawn_user(
                &self,
                _: &std::path::Path,
                _: &[std::ffi::OsString],
            ) -> Result<Box<dyn crate::user_helper::UserChild>> {
                anyhow::bail!("a probe must not start a child")
            }
            fn spawn_user_piped(
                &self,
                _: &std::path::Path,
                _: &[std::ffi::OsString],
            ) -> Result<crate::user_helper::PipedUserChild> {
                anyhow::bail!("a probe must not start a child")
            }
            fn user_sid(&self) -> Option<String> {
                None
            }
        }
        let Err(error) = pyrowave_with(&Config::default(), &|| true, &NoLauncher) else {
            panic!("PyroWave cannot be found on Linux yet")
        };
        assert!(
            !error.is::<SessionNotReady>()
                && !error.is::<TimedOut>()
                && !error.is::<Crashed>()
                && !error.is::<Interrupted>()
        );
        assert!(error.to_string().contains("not available on a Linux host"));
        assert_eq!(
            SessionNotReady::new("nobody is signed in").to_string(),
            "nobody is signed in"
        );
        assert_eq!(Crashed::new("exit 5").to_string(), "exit 5");
    }
}
