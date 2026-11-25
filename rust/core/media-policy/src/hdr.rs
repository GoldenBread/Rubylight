//! Moonlight's HDR mastering metadata, matching encoder values, and HDR fallback policy.
use std::time::{Duration, Instant};

/// How long a running HDR stream must continuously observe an SDR display and
/// BGRA8 capture surface before dropping to ten-bit SDR. Absorbs transient SDR
/// frames during exclusive-fullscreen and display mode transitions without
/// flapping encoder configuration or client HDR mode (`0x010e`).
pub const HDR_FALLBACK_GRACE: Duration = Duration::from_secs(2);

/// Mid-stream HDR/SDR transition decided by [`HdrFallback`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HdrTransition {
    None,
    /// Display HDR stayed off with an SDR surface past [`HDR_FALLBACK_GRACE`].
    FallbackToSdr,
    /// Display HDR returned with a wide/HDR capture surface after fallback.
    RestoreHdr,
}

/// Restore a running stream's HDR fields from its originally negotiated config
/// after an earlier fallback to ten-bit SDR, preserving any mid-stream size,
/// frame-rate or bitrate reconfiguration.
pub fn restore_hdr(stream: &mut crate::rtsp::Negotiated, negotiated: &crate::rtsp::Negotiated) {
    if negotiated.hdr {
        stream.hdr = true;
        stream.sdr_10bit = negotiated.sdr_10bit;
        stream.hdr_format = negotiated.hdr_format;
    }
}

/// Tracks mid-stream HDR-to-SDR fallback debounce and automatic HDR recovery
/// for non-WebRTC streams that negotiated HDR.
#[derive(Default, Debug)]
pub struct HdrFallback {
    sdr_since: Option<Instant>,
    seen_hdr: bool,
}

impl HdrFallback {
    /// Whether this stream has already carried active HDR (used to tell a
    /// mid-stream capture recovery fallback apart from a startup HDR failure).
    pub fn ever_hdr(&self) -> bool {
        self.seen_hdr
    }

    /// Record that the stream started in HDR (e.g. initial encoder creation
    /// succeeded in HDR before entering the main frame loop).
    pub fn note_hdr_active(&mut self) {
        self.seen_hdr = true;
    }

    /// Whether a stream must fall back to ten-bit SDR immediately at startup
    /// (first frame before encoder creation) because capture shows the display
    /// stayed in SDR with a BGRA8 surface and RTX HDR is not generating HDR.
    pub fn decide_startup(
        webrtc: bool,
        stream_hdr: bool,
        source_is_bgra8: bool,
        display_in_hdr: Option<bool>,
        rtx_hdr_active: bool,
    ) -> bool {
        !webrtc && stream_hdr && source_is_bgra8 && display_in_hdr == Some(false) && !rtx_hdr_active
    }

    /// Evaluate the current captured frame during an active session.
    ///
    /// - While encoding HDR, requires continuous SDR frames (`Bgra8` with
    ///   `display_in_hdr == Some(false)` and no active RTX HDR) for
    ///   [`HDR_FALLBACK_GRACE`] before falling back to ten-bit SDR.
    /// - While fallen back to ten-bit SDR on an originally negotiated HDR
    ///   stream, returns to HDR immediately once capture delivers a non-BGRA8
    ///   surface and Windows reports the output back in HDR (`Some(true)`).
    #[allow(clippy::too_many_arguments)]
    pub fn observe(
        &mut self,
        webrtc: bool,
        negotiated_hdr: bool,
        stream_hdr: bool,
        source_is_bgra8: bool,
        display_in_hdr: Option<bool>,
        rtx_hdr_active: bool,
        now: Instant,
    ) -> HdrTransition {
        self.observe_for_encoder(
            webrtc,
            negotiated_hdr,
            stream_hdr,
            source_is_bgra8,
            display_in_hdr,
            rtx_hdr_active,
            false,
            now,
        )
    }

    /// Like [`Self::observe`], but when `encoder_missing` is true (capture
    /// recovery / backend switch before the replacement encoder is created),
    /// skips [`HDR_FALLBACK_GRACE`] so the new encoder starts directly in
    /// ten-bit SDR instead of encoding BGRA8 under HDR10 metadata for 2 s.
    #[allow(clippy::too_many_arguments)]
    pub fn observe_for_encoder(
        &mut self,
        webrtc: bool,
        negotiated_hdr: bool,
        stream_hdr: bool,
        source_is_bgra8: bool,
        display_in_hdr: Option<bool>,
        rtx_hdr_active: bool,
        encoder_missing: bool,
        now: Instant,
    ) -> HdrTransition {
        if webrtc || !negotiated_hdr {
            self.sdr_since = None;
            return HdrTransition::None;
        }
        if stream_hdr {
            if source_is_bgra8 && display_in_hdr == Some(false) && !rtx_hdr_active {
                if encoder_missing {
                    self.sdr_since = None;
                    return HdrTransition::FallbackToSdr;
                }
                let since = *self.sdr_since.get_or_insert(now);
                if now.saturating_duration_since(since) >= HDR_FALLBACK_GRACE {
                    self.sdr_since = None;
                    return HdrTransition::FallbackToSdr;
                }
            } else {
                self.sdr_since = None;
                if !source_is_bgra8 || display_in_hdr == Some(true) || rtx_hdr_active {
                    self.seen_hdr = true;
                }
            }
            HdrTransition::None
        } else {
            self.sdr_since = None;
            if !source_is_bgra8 && display_in_hdr == Some(true) {
                self.seen_hdr = true;
                HdrTransition::RestoreHdr
            } else {
                HdrTransition::None
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Metadata {
    pub primaries: [[u16; 2]; 3],
    pub white: [u16; 2],
    pub maximum_nits: u16,
    /// Units of 1/10,000 nit, as in the Moonlight and HEVC metadata syntax.
    pub minimum: u16,
    pub max_cll: u16,
    pub max_fall: u16,
    pub full_frame_nits: u16,
}
impl Default for Metadata {
    fn default() -> Self {
        Self {
            primaries: [[35400, 14600], [8500, 39850], [6550, 2300]],
            white: [15635, 16450],
            maximum_nits: 1000,
            minimum: 1,
            max_cll: 0,
            max_fall: 0,
            full_frame_nits: 0,
        }
    }
}
impl Metadata {
    pub fn display(maximum: f32, minimum: f32, full_frame: f32) -> Self {
        let mut metadata = Self::default();
        if maximum.is_finite() && maximum > 0. {
            metadata.maximum_nits = maximum.clamp(1., u16::MAX as f32) as u16;
        }
        if minimum.is_finite() && minimum >= 0. {
            metadata.minimum = (minimum * 10000.).clamp(0., u16::MAX as f32) as u16;
        }
        if full_frame.is_finite() && full_frame > 0. {
            metadata.full_frame_nits = full_frame.clamp(1., metadata.maximum_nits as f32) as u16;
        }
        // The source panel's peak and full-frame luminance stand in for the content's light
        // levels: Windows reports no MaxCLL or MaxFALL for the desktop. A full-frame level the
        // panel does not report stays 0, which CTA-861.3 reads as unknown: 1 nit would be read as
        // a real average, and the Dolby Vision and HDR10+ fallbacks (`dovi::Level1::from_static`,
        // `hdr_dynamic::FrameStats::from_static`) would take the picture as nearly black instead
        // of using the reference white.
        metadata.max_cll = metadata.maximum_nits.max(1);
        metadata.max_fall = metadata.full_frame_nits;
        metadata
    }
    /// The metadata for a picture an enhancement generated at `peak` nits
    /// (RTX HDR): its own peak takes the source panel's place, and the
    /// content's light levels stay unknown (0, as CTA-861.3 reads them),
    /// because nothing reports them for the generated picture.
    pub fn generated(mut self, peak: u16) -> Self {
        self.maximum_nits = peak.max(1);
        self.full_frame_nits = 0;
        self.max_cll = 0;
        self.max_fall = 0;
        self
    }
    pub fn wire(self, enabled: bool) -> [u8; 27] {
        let mut bytes = [0; 27];
        bytes[0] = u8::from(enabled);
        let values = self
            .primaries
            .into_iter()
            .flatten()
            .chain(self.white)
            .chain([
                self.maximum_nits,
                self.minimum,
                self.max_cll,
                self.max_fall,
                self.full_frame_nits,
            ]);
        for (value, out) in values.zip(bytes[1..].as_chunks_mut::<2>().0) {
            *out = value.to_le_bytes();
        }
        bytes
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_luminance_is_finite_bounded_and_uses_wire_units() {
        let metadata = Metadata::display(1499.9, 0.002, 800.);
        assert_eq!(metadata.maximum_nits, 1499);
        assert_eq!(metadata.minimum, 20);
        assert_eq!(metadata.full_frame_nits, 800);
        let wire = metadata.wire(true);
        assert_eq!(wire[0], 1);
        let fields: Vec<_> = wire[1..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| u16::from_le_bytes(*v))
            .collect();
        assert_eq!(
            fields,
            [
                35400, 14600, 8500, 39850, 6550, 2300, 15635, 16450, 1499, 20, 1499, 800, 800
            ]
        );
        // A broken report keeps the defaults, light levels included.
        let broken = Metadata::display(f32::NAN, f32::INFINITY, -1.);
        assert_eq!(
            (broken.maximum_nits, broken.minimum, broken.full_frame_nits),
            (1000, 1, 0)
        );
        assert_eq!((broken.max_cll, broken.max_fall), (1000, 0));
        assert_eq!(Metadata::display(500., 0., 1000.).full_frame_nits, 500);
    }

    /// What `IDXGIOutput6::GetDesc1` reports for physical panels: an EDID
    /// without a luminance block gives zeros, which keep the 1000-nit default
    /// instead of advertising an SDR-level display to the client.
    #[test]
    fn physical_panel_luminance_reports_are_usable_or_default() {
        // HDR400 panel: 400 peak, 0.1 nit black, 400 sustained.
        let hdr400 = Metadata::display(400., 0.1, 400.);
        assert_eq!(
            (hdr400.maximum_nits, hdr400.minimum, hdr400.full_frame_nits),
            (400, 1000, 400)
        );
        // The panel's levels stand in for the content's light levels.
        assert_eq!((hdr400.max_cll, hdr400.max_fall), (400, 400));
        // OLED: perfect black stays zero.
        assert_eq!(Metadata::display(1000., 0., 250.).minimum, 0);
        // No luminance block in the EDID: peak and full-frame unknown.
        let unknown = Metadata::display(0., 0., 0.);
        assert_eq!(unknown.maximum_nits, 1000);
        assert_eq!(unknown.full_frame_nits, 0);
        // MaxCLL follows the default peak; MaxFALL stays unknown (0), never a 1-nit average.
        assert_eq!((unknown.max_cll, unknown.max_fall), (1000, 0));
        // Mini-LED claims past the 16-bit wire range are clamped, not wrapped.
        assert_eq!(
            Metadata::display(100_000., 0.0001, 5000.).maximum_nits,
            u16::MAX
        );
        // A full-frame figure above the peak is a driver error: cap it.
        assert_eq!(Metadata::display(600., 0.05, 900.).full_frame_nits, 600);
    }

    /// RTX HDR generates its own HDR picture: the stream describes the peak
    /// it was made for, not the SDR panel's 300 nits, and the content light
    /// levels it cannot know stay 0 (unknown).
    #[test]
    fn generated_hdr_describes_its_own_peak_not_the_sdr_panel() {
        let panel = Metadata::display(300., 0.1, 200.);
        let generated = panel.generated(1000);
        assert_eq!(generated.maximum_nits, 1000);
        assert_eq!((generated.max_cll, generated.max_fall), (0, 0));
        assert_eq!(generated.full_frame_nits, 0);
        // The panel's colour primaries and floor still describe the output.
        assert_eq!(
            (generated.primaries, generated.minimum),
            (panel.primaries, 1000)
        );
        let wire = generated.wire(true);
        let fields: Vec<_> = wire[1..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| u16::from_le_bytes(*v))
            .collect();
        assert_eq!(&fields[8..], [1000, 1000, 0, 0, 0]);
    }

    #[test]
    fn hdr_fallback_absorbs_transient_sdr_transitions_and_recovers_when_hdr_returns() {
        let start = Instant::now();
        let mut tracker = HdrFallback::default();

        // Transient BGRA8 + in_hdr=false frames shorter than HDR_FALLBACK_GRACE do not drop HDR.
        assert_eq!(
            tracker.observe(false, true, true, true, Some(false), false, start),
            HdrTransition::None
        );
        assert_eq!(
            tracker.observe(
                false,
                true,
                true,
                true,
                Some(false),
                false,
                start + HDR_FALLBACK_GRACE - Duration::from_millis(1),
            ),
            HdrTransition::None
        );

        // An FP16 frame or unreadable colour space resets the grace window.
        assert_eq!(
            tracker.observe(
                false,
                true,
                true,
                false,
                Some(false),
                false,
                start + HDR_FALLBACK_GRACE,
            ),
            HdrTransition::None
        );
        let second_sdr = start + HDR_FALLBACK_GRACE + Duration::from_millis(100);
        assert_eq!(
            tracker.observe(false, true, true, true, Some(false), false, second_sdr),
            HdrTransition::None
        );
        assert_eq!(
            tracker.observe(
                false,
                true,
                true,
                true,
                None,
                false,
                second_sdr + Duration::from_millis(500),
            ),
            HdrTransition::None
        );

        // Continuous SDR past HDR_FALLBACK_GRACE falls back to 10-bit SDR.
        let sustained = second_sdr + Duration::from_secs(1);
        assert_eq!(
            tracker.observe(false, true, true, true, Some(false), false, sustained),
            HdrTransition::None
        );
        assert_eq!(
            tracker.observe(
                false,
                true,
                true,
                true,
                Some(false),
                false,
                sustained + HDR_FALLBACK_GRACE,
            ),
            HdrTransition::FallbackToSdr
        );

        // While fallen back, ACM/WCG FP16 (in_hdr=false) or BGRA8 with in_hdr=true do not restore HDR.
        let after = sustained + HDR_FALLBACK_GRACE + Duration::from_millis(100);
        assert_eq!(
            tracker.observe(false, true, false, false, Some(false), false, after),
            HdrTransition::None
        );
        assert_eq!(
            tracker.observe(false, true, false, true, Some(true), false, after),
            HdrTransition::None
        );

        // Once capture delivers FP16/PQ10 with display HDR active, HDR restores immediately.
        assert_eq!(
            tracker.observe(false, true, false, false, Some(true), false, after),
            HdrTransition::RestoreHdr
        );

        // Originally SDR (including hdrMode=0 / prefer_sdr_10bit) and WebRTC never transition here.
        assert_eq!(
            tracker.observe(false, false, false, false, Some(true), false, after),
            HdrTransition::None
        );
        assert_eq!(
            tracker.observe(
                true,
                true,
                true,
                true,
                Some(false),
                false,
                after + HDR_FALLBACK_GRACE * 2,
            ),
            HdrTransition::None
        );
        // Active RTX HDR on BGRA8 never falls back.
        assert_eq!(
            tracker.observe(
                false,
                true,
                true,
                true,
                Some(false),
                true,
                after + HDR_FALLBACK_GRACE * 2,
            ),
            HdrTransition::None
        );
    }

    #[test]
    fn restore_hdr_recovers_negotiated_hdr_fields_without_clobbering_reconfigure() {
        let negotiated = crate::rtsp::Negotiated {
            width: 2560,
            height: 1440,
            fps: 144,
            codec: 2,
            hdr: true,
            sdr_10bit: false,
            hdr_format: rubylight_protocol::hdr::DV_101,
            csc_mode: 2,
            ..Default::default()
        };
        let mut stream = negotiated.clone();
        crate::stream_policy::stream_sdr_10bit(&mut stream);
        // Simulate a mid-stream 0x5532 mode reconfigure while fallen back to SDR.
        stream.width = 1920;
        stream.height = 1080;
        stream.fps = 120;
        assert!(!stream.hdr && stream.sdr_10bit && stream.hdr_format == 0);

        restore_hdr(&mut stream, &negotiated);
        assert!(stream.hdr);
        assert!(!stream.sdr_10bit);
        assert_eq!(stream.hdr_format, rubylight_protocol::hdr::DV_101);
        assert_eq!(stream.color_matrix(), 2);
        assert_eq!((stream.width, stream.height, stream.fps), (1920, 1080, 120));

        // A stream that never negotiated HDR stays SDR.
        let sdr_negotiated = crate::rtsp::Negotiated {
            hdr: false,
            sdr_10bit: true,
            ..Default::default()
        };
        let mut sdr_stream = sdr_negotiated.clone();
        restore_hdr(&mut sdr_stream, &sdr_negotiated);
        assert!(!sdr_stream.hdr && sdr_stream.sdr_10bit);
    }

    #[test]
    fn hdr_fallback_is_immediate_when_encoder_is_missing_after_recovery() {
        let now = Instant::now();
        // Startup failure before any HDR frame: immediate fallback, ever_hdr stays false.
        let mut startup = HdrFallback::default();
        assert!(!startup.ever_hdr());
        assert_eq!(
            startup.observe_for_encoder(false, true, true, true, Some(false), false, true, now),
            HdrTransition::FallbackToSdr
        );
        assert!(!startup.ever_hdr());

        // Mid-stream capture recovery after active HDR: immediate fallback, ever_hdr is true.
        let mut mid_stream = HdrFallback::default();
        assert_eq!(
            mid_stream.observe_for_encoder(false, true, true, false, Some(true), false, false, now),
            HdrTransition::None
        );
        assert!(mid_stream.ever_hdr());
        assert_eq!(
            mid_stream.observe_for_encoder(
                false,
                true,
                true,
                true,
                Some(false),
                false,
                true,
                now + Duration::from_millis(16),
            ),
            HdrTransition::FallbackToSdr
        );
        assert!(mid_stream.ever_hdr());

        // Active RTX HDR or unreadable colour space still does not drop HDR.
        let mut tracker = HdrFallback::default();
        assert_eq!(
            tracker.observe_for_encoder(false, true, true, true, Some(false), true, true, now),
            HdrTransition::None
        );
        assert!(tracker.ever_hdr());
        let mut unknown = HdrFallback::default();
        assert_eq!(
            unknown.observe_for_encoder(false, true, true, true, None, false, true, now),
            HdrTransition::None
        );
        assert!(!unknown.ever_hdr());
    }
}
