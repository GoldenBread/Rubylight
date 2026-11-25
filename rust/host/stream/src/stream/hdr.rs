//! Stream-level HDR source enforcement, SDR fallback, and automatic HDR recovery.
use super::*;
use butterpollo_media_policy::hdr::{HdrFallback, HdrTransition, restore_hdr};
use butterpollo_media_policy::stream_policy::stream_sdr_10bit;
use butterpollo_session::session::Transport;
use butterpollo_win_graphics::capture::{GpuImage, Pixel};

/// A WebRTC HDR stream carries the display's HDR or nothing: SDR content labelled HDR, or HDR
/// tone-mapped to SDR, is not what the client asked for, so the session ends with an error.
/// `display_in_hdr` is the capture's reading of the output's colour space (HDR10 or not); the
/// frames' pixel format cannot stand in for it, since FP16 frames come from SDR outputs too.
/// GameStream keeps its own behaviour for this case. `running` tells a display that left HDR
/// from one that never was in it.
pub(crate) fn require_hdr_source(
    s: &Session,
    display_in_hdr: impl FnOnce() -> Option<bool>,
    running: bool,
) -> Result<()> {
    if s.launch.transport != Transport::WebRtc || !s.config.hdr {
        return Ok(());
    }
    match display_in_hdr() {
        Some(true) => Ok(()),
        Some(false) if running => bail!(
            "the display left HDR during the stream; the HDR stream ended instead of showing SDR as HDR. Keep Windows HDR on for this display, or start an SDR stream"
        ),
        Some(false) => bail!(
            "HDR was requested but the display is not in HDR; turn on HDR for this display in Windows settings (or set the HDR policy to auto in the host's display settings), or start an SDR stream"
        ),
        // A reading that fails mid-stream is the capture's to report (it ends on a colour space
        // error itself); only a display seen leaving HDR ends the stream here.
        None if running => Ok(()),
        None => bail!(
            "HDR was requested but Windows did not report the display's colour space, so the stream cannot tell HDR from SDR; start an SDR stream"
        ),
    }
}

/// Whether a non-WebRTC stream configured for HDR must fall back to 10-bit SDR
/// because capture shows the display stayed in SDR (`Some(false)`) with an SDR
/// surface and RTX HDR is not generating HDR from it.
pub(crate) fn should_fallback_hdr_to_sdr(
    transport: Transport,
    stream_hdr: bool,
    source_is_bgra8: bool,
    display_in_hdr: Option<bool>,
    rtx_hdr_active: bool,
) -> bool {
    HdrFallback::decide_startup(
        transport == Transport::WebRtc,
        stream_hdr,
        source_is_bgra8,
        display_in_hdr,
        rtx_hdr_active,
    )
}

impl Media {
    pub(super) fn set_display_hdr_metadata(
        &self,
        s: &Session,
        display: butterpollo_media_policy::hdr::Metadata,
    ) -> butterpollo_media_policy::hdr::Metadata {
        let hdr = s.hdr_active();
        let previous = s.hdr_metadata.read().unwrap().map(|m| m.wire(hdr));
        let metadata = s.set_display_hdr_metadata(display);
        if previous != Some(metadata.wire(hdr)) {
            let _ = self.control_wake.send(&[1]);
        }
        metadata
    }

    /// Drops a GameStream session out of HDR to 10-bit SDR immediately at
    /// startup when Windows left the captured display in SDR with a BGRA8
    /// surface and RTX HDR is not running, waking control so clients receive
    /// `0x010e` (`enabled=false`).
    pub(super) fn fallback_hdr_if_sdr(
        &self,
        s: &Session,
        stream: &mut butterpollo_media_policy::rtsp::Negotiated,
        image: &GpuImage,
        rtx_hdr_active: bool,
    ) -> bool {
        let display_in_hdr = image.gpu.display_in_hdr();
        if !should_fallback_hdr_to_sdr(
            s.launch.transport,
            stream.hdr,
            image.pixel == Pixel::Bgra8,
            display_in_hdr,
            rtx_hdr_active,
        ) {
            return false;
        }
        self.apply_hdr_fallback(s, stream, image, false);
        true
    }

    /// Mid-stream HDR/SDR tracking: absorbs transient SDR frames during
    /// exclusive-fullscreen or display mode transitions before falling back to
    /// 10-bit SDR, and returns a fallen-back GameStream session to HDR once
    /// capture reports display HDR active again on a wide/HDR surface. Returns
    /// `true` when the encoder must rebuild for the new colour mode.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sync_hdr_state(
        &self,
        s: &Session,
        stream: &mut butterpollo_media_policy::rtsp::Negotiated,
        image: &GpuImage,
        prepared: &crate::display_session::Ready,
        rtx_hdr_active: bool,
        encoder_missing: bool,
        tracker: &mut HdrFallback,
        now: Instant,
    ) -> bool {
        match tracker.observe_for_encoder(
            s.launch.transport == Transport::WebRtc,
            s.config.hdr,
            stream.hdr,
            image.pixel == Pixel::Bgra8,
            image.gpu.display_in_hdr(),
            rtx_hdr_active,
            encoder_missing,
            now,
        ) {
            HdrTransition::None => false,
            HdrTransition::FallbackToSdr => {
                self.apply_hdr_fallback(s, stream, image, !encoder_missing);
                prepared.note_hdr_fallback();
                true
            }
            HdrTransition::RestoreHdr => {
                restore_hdr(stream, &s.config);
                let display_metadata = image.gpu.hdr_metadata();
                if s.restore_hdr(display_metadata) {
                    let _ = self.control_wake.send(&[1]);
                }
                s.launch.warnings.clear("display_hdr");
                prepared.clear_hdr_fallback();
                tracing::info!(
                    client = %s.launch.client.name,
                    output = %image.gpu.display.display_name,
                    source_pixel = ?image.pixel,
                    sdr_white_nits = image.gpu.sdr_white_nits(),
                    "display HDR is active again; returning stream to HDR"
                );
                true
            }
        }
    }

    fn apply_hdr_fallback(
        &self,
        s: &Session,
        stream: &mut butterpollo_media_policy::rtsp::Negotiated,
        image: &GpuImage,
        mid_stream: bool,
    ) {
        stream_sdr_10bit(stream);
        if s.disable_hdr() {
            let _ = self.control_wake.send(&[1]);
        }
        let warning = hdr_fallback_warning(mid_stream);
        if mid_stream {
            tracing::warn!(
                client = %s.launch.client.name,
                output = %image.gpu.display.display_name,
                source_pixel = ?image.pixel,
                "display left HDR during the stream; falling back to 10-bit SDR"
            );
        } else {
            tracing::warn!(
                client = %s.launch.client.name,
                output = %image.gpu.display.display_name,
                source_pixel = ?image.pixel,
                "display HDR did not stay active; falling back to 10-bit SDR for this stream"
            );
        }
        s.launch.warnings.set("display_hdr", warning);
    }
}

fn hdr_fallback_warning(mid_stream: bool) -> &'static str {
    if mid_stream {
        "Display HDR turned off during the stream; this stream fell back to 10-bit SDR and will return to HDR automatically if HDR turns back on."
    } else {
        "Requested display HDR state was not applied; capture source HDR is off, so this stream fell back to SDR. Check Windows HDR and the display policy, or reconnect with HDR disabled."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restoring_hdr_updates_effective_config_info_and_hdr_control_payload() {
        let f = crate::state::test_support::Fixture::new();
        let negotiated = butterpollo_media_policy::rtsp::Negotiated {
            codec: 2,
            hdr: true,
            hdr_format: 1,
            csc_mode: 2,
            ..Default::default()
        };
        let client = f.client(u32::MAX);
        let session = Session::new(f.launch(client, Role::Stream), negotiated);
        session.set_display_hdr_metadata(butterpollo_media_policy::hdr::Metadata::default());
        assert!(session.disable_hdr());
        assert!(!session.hdr_active());

        let revision_sdr = session
            .hdr_revision
            .load(std::sync::atomic::Ordering::Acquire);
        let restored_meta = butterpollo_media_policy::hdr::Metadata::display(1000., 0.001, 400.);
        assert!(session.restore_hdr(restored_meta));
        assert!(!session.restore_hdr(restored_meta));
        assert!(session.hdr_active());
        assert_eq!(session.info()["hdr"], true);
        assert_eq!(
            session
                .hdr_revision
                .load(std::sync::atomic::Ordering::Acquire),
            revision_sdr + 1
        );
        assert_eq!(*session.hdr_metadata.read().unwrap(), Some(restored_meta));
        let restored_effective = session.effective_config();
        assert!(restored_effective.hdr);
        assert!(!restored_effective.sdr_10bit);
        assert_eq!(restored_effective.hdr_format, 1);
        assert_eq!(restored_effective.color_matrix(), 2);
        assert_eq!(restored_meta.wire(session.hdr_active())[0], 1);

        // An originally SDR session cannot be turned into HDR via restore_hdr.
        let sdr_session = Session::new(
            f.launch(f.client(u32::MAX), Role::Stream),
            butterpollo_media_policy::rtsp::Negotiated {
                codec: 2,
                hdr: false,
                sdr_10bit: true,
                ..Default::default()
            },
        );
        assert!(!sdr_session.restore_hdr(restored_meta));
        assert!(!sdr_session.hdr_active());
    }

    #[test]
    fn startup_and_mid_stream_hdr_fallback_warnings_are_distinct() {
        let startup = hdr_fallback_warning(false);
        let mid_stream = hdr_fallback_warning(true);
        assert_ne!(startup, mid_stream);
        assert!(startup.contains("not applied"));
        assert!(mid_stream.contains("during the stream"));
        assert!(mid_stream.contains("return to HDR automatically"));
    }
}
