//! The optional codec probes (PyroWave and packed 4:4:4) and their typed failures. Both run in
//! the host's process: PyroWave on Vulkan compute, packed 4:4:4 on the VA-API encoder with a
//! Vulkan compute pass ([`crate::packed444`]). A probe that cannot run fails with an error that
//! is none of the retryable ones.
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
/// The PyroWave modes this host encodes: one 640x480 frame in each, in this process. The
/// Windows host probes in a child, which keeps an AMD encoder hang out of the host; on Linux
/// PyroWave runs on Vulkan compute, not the video encoder, and its library is loaded once.
pub fn pyrowave_with(
    _config: &Config,
    stop: &dyn Fn() -> bool,
    _launcher: &dyn UserLauncher,
) -> Result<u32> {
    if stop() {
        return Err(Interrupted.into());
    }
    crate::pyrowave::probe_flags(&crate::pyrowave::Options::default())
}

/// Probe one packed 4:4:4 mode: three pictures (a key picture and two that refer to it)
/// packed and encoded as a stream of the mode would be. An invalid request is refused as such;
/// a mode this host cannot encode (10-bit, the layered version) fails with why, so clients are
/// not offered it.
pub fn packed_with(
    config: &Config,
    stream: &Negotiated,
    stop: &dyn Fn() -> bool,
    _launcher: &dyn UserLauncher,
) -> Result<()> {
    stream.validate()?;
    ensure!(
        rubylight_protocol::packed444::supported(stream.chroma_packing),
        "invalid packed probe request"
    );
    if stop() {
        return Err(Interrupted.into());
    }
    let mut encoder =
        crate::encoder::Encoder::new_options(stream, config.get("output_name", ""), config)?;
    // Columns of alternating colours: the chroma a 4:2:0 picture would average away.
    let (width, height) = (stream.width, stream.height);
    let bytes = (0..width * height)
        .flat_map(|i| match i % 3 {
            0 => [0, 0, 255, 255],
            1 => [255, 0, 0, 255],
            _ => [0, 255, 0, 255],
        })
        .collect();
    let image = crate::capture::Image {
        width,
        height,
        stride: width as usize * 4,
        bytes,
        captured: std::time::Instant::now(),
        pixel: crate::capture::Pixel::Bgra8,
    };
    for picture in 0..3 {
        if stop() {
            return Err(Interrupted.into());
        }
        let encoded = encoder.encode(&image, picture == 0, stream.bitrate_kbps.max(1000))?;
        ensure!(
            !encoded.is_empty(),
            "the packed encoder gave no picture {picture}"
        );
    }
    Ok(())
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
        // A stop already asked for interrupts the probe before it opens anything.
        let stopped = pyrowave_with(&Config::default(), &|| true, &NoLauncher).unwrap_err();
        assert!(stopped.is::<Interrupted>());
        // Otherwise it finds PyroWave's two SDR modes, or fails with an error the host does not
        // retry: no library beside the host, or no Vulkan device.
        match pyrowave_with(&Config::default(), &|| false, &NoLauncher) {
            Ok(flags) => assert_eq!(flags & !0x0180_0000, 0, "{flags:#x}"),
            Err(error) => assert!(
                !error.is::<SessionNotReady>()
                    && !error.is::<TimedOut>()
                    && !error.is::<Crashed>()
                    && !error.is::<Interrupted>(),
                "{error:#}"
            ),
        }
        assert_eq!(
            SessionNotReady::new("nobody is signed in").to_string(),
            "nobody is signed in"
        );
        assert_eq!(Crashed::new("exit 5").to_string(), "exit 5");
    }
}
