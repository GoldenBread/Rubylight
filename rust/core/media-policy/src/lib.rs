//! Media, encoder, capture pacing, display policy, HDR, packet framing, FEC, and RTSP decisions.
#![warn(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::undocumented_unsafe_blocks
)]

/// Sink for user-facing session warnings raised by media-policy helpers.
pub trait WarningSink {
    fn set(&self, code: &str, message: &str);
    fn clear(&self, code: &str);
}

impl<T: WarningSink + ?Sized> WarningSink for std::sync::Arc<T> {
    fn set(&self, code: &str, message: &str) {
        (**self).set(code, message);
    }

    fn clear(&self, code: &str) {
        (**self).clear(code);
    }
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TestWarning {
    pub(crate) code: String,
    pub(crate) message: String,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestWarnings {
    entries: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
}

#[cfg(test)]
impl WarningSink for TestWarnings {
    fn set(&self, code: &str, message: &str) {
        self.entries
            .lock()
            .unwrap()
            .insert(code.to_owned(), message.to_owned());
    }

    fn clear(&self, code: &str) {
        self.entries.lock().unwrap().remove(code);
    }
}

#[cfg(test)]
impl TestWarnings {
    pub(crate) fn snapshot(&self) -> Vec<TestWarning> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .map(|(code, message)| TestWarning {
                code: code.clone(),
                message: message.clone(),
            })
            .collect()
    }
}

pub mod adaptive_bitrate;
pub mod adaptive_fec;
pub mod adaptive_pacing;
pub mod amf_vpe;
pub mod app_window;
pub mod arrival_gaps;
pub mod audio;
pub mod audio_cadence;
pub mod audio_defaults;
pub mod audio_dsp;
pub mod audio_passthrough;
pub mod bandwidth_probe;
pub mod bitstream;
pub mod capture_policy;
pub mod clock_sync;
pub mod codec_support;
pub mod congestion;
pub mod display_caps;
pub mod display_policy;
pub mod dovi;
pub mod edid;
pub mod encoder_backlog;
pub mod encoder_focus;
pub mod encoder_gpu;
pub mod encoder_policy;
pub mod extensions;
pub mod frame_warp;
pub mod framegen;
pub mod hdr;
pub mod hdr_dynamic;
pub mod host_timing;
pub mod image_quality;
pub mod keyframe_loss;
pub mod link_control;
pub mod link_notice;
pub mod link_rate;
pub mod lossless;
pub mod ltr;
pub mod network_pacing;
pub mod network_path;
pub mod nvenc;
pub mod nvidia_component;
pub mod nvidia_enhance;
pub mod packet;
pub mod performance;
pub mod phase_sync;
pub mod pipeline_rates;
pub mod present_timing;
pub mod pyrowave;
pub mod pyrowave_probe;
pub mod radeon_profile;
pub mod rate_change;
pub mod reconfigure;
pub mod rtsp;
pub mod stall_watch;
pub mod statistical_fec;
pub mod stream_policy;
pub mod stream_preset;
pub mod stutter_report;
pub mod timing_record;
pub mod topology;
pub mod virtual_output;
