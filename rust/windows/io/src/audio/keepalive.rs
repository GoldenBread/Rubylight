//! Low-period shared WASAPI keep-alive stream for endpoint loopback capture.
//! Loopback capture stops delivering when no stream renders to the endpoint;
//! rendering one silent buffer and leaving it to starve keeps the endpoint clock
//! active, and requesting `min_period` via `IAudioClient3` lets the shared audio
//! engine tick at its lowest period when the driver supports it.

use anyhow::Result;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::core::Interface;

/// A started render stream of silence on the captured endpoint. Loopback capture delivers
/// nothing while no stream renders to its endpoint, so when the sound stopped the host went
/// quiet until it noticed (50 ms) and the client heard an underrun; with this the endpoint keeps
/// delivering silent audio at its own pace, and sound turning to silence leaves no gap. It
/// renders one buffer of silence and is left to starve, which keeps the stream active (OBS does
/// the same); the client gets the silent Opus packets it got after the 50 ms anyway.
pub(super) struct KeepAlive(IAudioClient);

fn should_use_low_period(min_period: u32, default_period: u32) -> bool {
    min_period > 0 && min_period < default_period
}

impl KeepAlive {
    /// # Safety
    /// COM is initialized on this thread and `format` is the endpoint's mix format.
    pub(super) unsafe fn new(device: &IMMDevice, format: *const WAVEFORMATEX) -> Result<Self> {
        // SAFETY: as the caller promises; the buffer is released before Start.
        unsafe {
            let mut client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let mut initialized = false;
            if let Ok(client3) = client.cast::<IAudioClient3>() {
                let (mut default_period, mut fundamental_period, mut min_period, mut max_period) =
                    (0, 0, 0, 0);
                if client3
                    .GetSharedModeEnginePeriod(
                        format,
                        &mut default_period,
                        &mut fundamental_period,
                        &mut min_period,
                        &mut max_period,
                    )
                    .is_ok()
                    && should_use_low_period(min_period, default_period)
                {
                    if client3
                        .InitializeSharedAudioStream(0, min_period, format, None)
                        .is_ok()
                    {
                        initialized = true;
                        tracing::debug!(
                            min_period_frames = min_period,
                            default_period_frames = default_period,
                            "audio keep-alive using low-period shared WASAPI stream"
                        );
                    } else {
                        // COM IAudioClient cannot be Initialized twice after a failed Initialize*.
                        drop(client3);
                        client = device.Activate(CLSCTX_ALL, None)?;
                    }
                }
            }
            if !initialized {
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 100_000, 0, format, None)?;
            }
            let frames = client.GetBufferSize()?;
            let render: IAudioRenderClient = client.GetService()?;
            render.GetBuffer(frames)?;
            render.ReleaseBuffer(frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
            client.Start()?;
            Ok(Self(client))
        }
    }
}

impl Drop for KeepAlive {
    fn drop(&mut self) {
        // SAFETY: the client is live and started.
        unsafe {
            let _ = self.0.Stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::should_use_low_period;

    #[test]
    fn low_period_is_requested_only_when_strictly_smaller_than_default() {
        assert!(!should_use_low_period(0, 480));
        assert!(!should_use_low_period(480, 480));
        assert!(!should_use_low_period(960, 480));
        assert!(should_use_low_period(96, 480));
        assert!(should_use_low_period(128, 480));
    }
}
