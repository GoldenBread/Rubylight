//! Shared-mode `WAVEFORMATEXTENSIBLE` builders and format matching helpers.
use super::{FormatChange, Journal, hooks, policy::Policy, save};
use anyhow::{Context, Result, bail};
use std::path::Path;
use windows::{Win32::Media::Audio::*, core::GUID};

/// Minimal subset of `Policy` needed for format probing/application (mockable in tests).
pub(super) trait FormatPolicy {
    fn format(&self, id: &str) -> Result<Vec<u8>>;
    fn set_format(&self, id: &str, bytes: &[u8]) -> Result<()>;
}

impl FormatPolicy for Policy {
    fn format(&self, id: &str) -> Result<Vec<u8>> {
        Policy::format(self, id)
    }
    fn set_format(&self, id: &str, bytes: &[u8]) -> Result<()> {
        Policy::set_format(self, id, bytes)
    }
}

/// Surround channel-count fallback ladder (`12 -> 8 -> 6`; `8 -> 6`; `6`; `2`).
pub(super) fn fallback_channels(channels: usize) -> Result<&'static [usize]> {
    match channels {
        2 => Ok(&[2]),
        6 => Ok(&[6]),
        8 => Ok(&[8, 6]),
        12 => Ok(&[12, 8, 6]),
        _ => bail!("virtual audio requires stereo, 5.1, 7.1 or 7.1.4"),
    }
}

/// What Steam Streaming Microphone's driver copies between its pins as it is:
/// channels, rate, container and valid bits, and whether samples are float.
pub(super) fn sample_layout(bytes: &[u8]) -> (u16, u32, u16, u16, bool) {
    assert!(
        bytes.len() >= size_of::<WAVEFORMATEX>(),
        "audio format shorter than WAVEFORMATEX"
    );
    // SAFETY: `bytes` was just checked to hold a whole WAVEFORMATEX, and the read is unaligned.
    let format = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<WAVEFORMATEX>()) };
    let float = format.wFormatTag == 3
        || (format.wFormatTag == 65534
            && bytes.len() >= size_of::<WAVEFORMATEXTENSIBLE>()
            // SAFETY: `bytes` was just checked to hold a whole WAVEFORMATEXTENSIBLE, and the read
            // is unaligned.
            && unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<WAVEFORMATEXTENSIBLE>()) }
                .SubFormat
                .data1
                == 3);
    (
        format.nChannels,
        format.nSamplesPerSec,
        format.wBitsPerSample,
        valid_bits(bytes),
        float,
    )
}

/// "2 ch 48000 Hz 32-bit PCM", for the log.
pub(super) fn describe_format(bytes: &[u8]) -> String {
    let (channels, rate, bits, valid, float) = sample_layout(bytes);
    let depth = if valid == bits {
        format!("{bits}-bit")
    } else {
        format!("{valid}-in-{bits}-bit")
    };
    format!(
        "{channels} ch {rate} Hz {depth} {}",
        if float { "float" } else { "PCM" }
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sample {
    Float32,
    Pcm32,
    Pcm24In32,
    Pcm24,
    Pcm16,
}
impl Sample {
    pub(super) fn bits(self) -> (u16, u16) {
        match self {
            Self::Float32 | Self::Pcm32 => (32, 32),
            Self::Pcm24In32 => (32, 24),
            Self::Pcm24 => (24, 24),
            Self::Pcm16 => (16, 16),
        }
    }
}

pub(super) fn virtual_format(channels: usize, sample: Sample, side: bool) -> Vec<u8> {
    let (bits, valid_bits) = sample.bits();
    let format = WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: 65534,
            nChannels: channels as u16,
            nSamplesPerSec: 48000,
            nAvgBytesPerSec: 48000 * channels as u32 * u32::from(bits / 8),
            nBlockAlign: channels as u16 * (bits / 8),
            wBitsPerSample: bits,
            cbSize: 22,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: valid_bits,
        },
        dwChannelMask: match channels {
            6 if side => 0x60f,
            6 => 0x3f,
            8 => 0x63f,
            // 7.1.4: 7.1 and TFL TFR TBL TBR (`audio_ext::CHANNEL_MASK_714`).
            12 => 0x2d63f,
            _ => 3,
        },
        SubFormat: GUID::from_u128(if sample == Sample::Float32 {
            0x00000003_0000_0010_8000_00aa00389b71
        } else {
            0x00000001_0000_0010_8000_00aa00389b71
        }),
    };
    // SAFETY: `format` is a live packed WAVEFORMATEXTENSIBLE with no padding, so all its bytes are
    // initialised.
    unsafe {
        std::slice::from_raw_parts(
            (&format as *const WAVEFORMATEXTENSIBLE).cast(),
            size_of::<WAVEFORMATEXTENSIBLE>(),
        )
        .to_vec()
    }
}

pub(super) fn valid_bits(bytes: &[u8]) -> u16 {
    // Policy::format has already checked the native format's allocation size.
    // SAFETY: callers pass bytes from Policy::format or virtual_format, which hold at least a whole
    // WAVEFORMATEX, and the read is unaligned.
    let format = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<WAVEFORMATEX>()) };
    if format.wFormatTag == 65534 && bytes.len() >= size_of::<WAVEFORMATEXTENSIBLE>() {
        // SAFETY: `bytes` was just checked to hold a whole WAVEFORMATEXTENSIBLE, and the read is
        // unaligned.
        let extended =
            unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<WAVEFORMATEXTENSIBLE>()) };
        // SAFETY: both fields of the union are u16, so any bits are a valid value.
        let bits = unsafe { extended.Samples.wValidBitsPerSample };
        if bits > 0 && bits <= format.wBitsPerSample {
            return bits;
        }
    }
    format.wBitsPerSample
}

pub(super) fn virtual_formats(channels: usize, preferred_bits: u16) -> Result<Vec<Vec<u8>>> {
    if !matches!(channels, 2 | 6 | 8 | 12) {
        bail!("virtual audio requires stereo, 5.1, 7.1 or 7.1.4");
    }
    let samples = if channels == 2 {
        // Retain the old host's stereo preference, including spatial audio.
        [
            Sample::Pcm24In32,
            Sample::Pcm24,
            Sample::Pcm16,
            Sample::Float32,
            Sample::Pcm32,
        ]
    } else {
        [
            Sample::Float32,
            Sample::Pcm32,
            Sample::Pcm24In32,
            Sample::Pcm24,
            Sample::Pcm16,
        ]
    };
    let mut output = Vec::new();
    // Match the playback device's valid depth first. A rejected format must
    // not prevent capturing an otherwise usable virtual speaker endpoint.
    for preferred in [true, false] {
        for sample in samples {
            if (sample.bits().1 == preferred_bits) != preferred {
                continue;
            }
            output.push(virtual_format(channels, sample, false));
            if channels == 6 {
                output.push(virtual_format(channels, sample, true));
            }
        }
    }
    Ok(output)
}

pub(super) fn apply_virtual_format(
    policy: &Policy,
    id: &str,
    channels: usize,
    bits: u16,
    directory: &Path,
    journal: &mut Journal,
) -> Result<usize> {
    apply_virtual_format_with(
        policy,
        id,
        channels,
        bits,
        directory,
        journal,
        hooks::mark_audio,
    )
}

/// Try `channels` and lower surround channel counts (`12 -> 8 -> 6`) on `id`.
///
/// Snapshots `journal.format` before mutating it and restores + re-saves the
/// previous journal entry if every candidate is rejected, so teardown always
/// sees the last actually-applied format rather than a rejected candidate.
pub(super) fn apply_virtual_format_with(
    policy: &dyn FormatPolicy,
    id: &str,
    channels: usize,
    bits: u16,
    directory: &Path,
    journal: &mut Journal,
    mark_audio: impl Fn(bool) -> Result<()>,
) -> Result<usize> {
    let counts = fallback_channels(channels)?;
    let previous = journal.format.clone();
    let current = policy.format(id)?;
    let current_channels = usize::from(sample_layout(&current).0);
    if journal.format.is_none() {
        journal.format = Some(FormatChange {
            id: id.to_owned(),
            before: current.clone(),
            applied: Vec::new(),
        });
    }
    let mut last_error = None;
    let mut wrote_journal = false;
    for &target in counts {
        // When falling back to a lower surround layout that is already active
        // and recorded as applied, keep it without triggering a redundant
        // format switch.
        if target < channels
            && current_channels == target
            && previous
                .as_ref()
                .is_some_and(|f| f.id == id && f.applied == current)
        {
            journal.format = previous;
            if wrote_journal {
                save(directory, journal)?;
            }
            return Ok(target);
        }
        for desired in virtual_formats(target, bits)? {
            journal
                .format
                .as_mut()
                .context("virtual audio format is not owned")?
                .applied = desired.clone();
            save(directory, journal)?;
            wrote_journal = true;
            mark_audio(true)?;
            match policy.set_format(id, &desired) {
                Ok(()) => {
                    journal.format.as_mut().unwrap().applied = policy.format(id)?;
                    save(directory, journal)?;
                    return Ok(target);
                }
                Err(error) => last_error = Some(error),
            }
        }
    }
    // Restore the previous applied format so a rejected candidate is never
    // left in `audio-recovery.json`.
    journal.format = previous;
    if wrote_journal && let Err(save_error) = save(directory, journal) {
        tracing::warn!(
            error = %format!("{save_error:#}"),
            "failed to restore audio recovery journal after rejected format"
        );
    }
    Err(last_error.context("no usable virtual audio format")?)
        .with_context(|| format!("setting {channels}-channel virtual speaker format"))
}

/// Gives the virtual sink `id` a `channels`-channel shared-mode format without touching any
/// default device, and returns the format it had, for `restore_sink_format`. For test tooling:
/// a stream's own route does this for itself, but it also takes over the defaults.
pub fn set_sink_channels(id: &str, channels: usize) -> Result<Vec<u8>> {
    let policy = Policy::new()?;
    let before = policy.format(id)?;
    let mut last_error = None;
    for desired in virtual_formats(channels, valid_bits(&before))? {
        match policy.set_format(id, &desired) {
            Ok(()) => return Ok(before),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.context("no usable virtual audio format")?)
        .with_context(|| format!("setting {channels}-channel virtual speaker format"))
}

/// Puts back what `set_sink_channels` returned.
pub fn restore_sink_format(id: &str, format: &[u8]) -> Result<()> {
    Policy::new()?.set_format(id, format)
}
