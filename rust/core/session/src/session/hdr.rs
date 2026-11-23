//! Live HDR state, fallback/recovery transitions, and display mastering metadata for a session.
use super::*;

impl<P, A> Session<P, A> {
    /// Whether the stream is currently encoding and signalling HDR.
    pub fn hdr_active(&self) -> bool {
        self.hdr_active.load(Ordering::Acquire)
    }
    /// Negotiated stream parameters adjusted for whether HDR stayed active.
    pub fn effective_config(&self) -> Negotiated {
        let mut config = self.config.clone();
        if config.hdr && !self.hdr_active() {
            butterpollo_media_policy::stream_policy::stream_sdr_10bit(&mut config);
        }
        config
    }
    /// Drop the stream from HDR back to 10-bit SDR and advance `hdr_revision`
    /// so control loops send `0x010e` (`enabled=false`). Returns `true` when
    /// active HDR turned off.
    pub fn disable_hdr(&self) -> bool {
        let changed = self.hdr_active.swap(false, Ordering::AcqRel);
        if changed {
            self.hdr_revision.fetch_add(1, Ordering::Release);
        }
        changed
    }
    /// Return a session that fell back to 10-bit SDR back to its negotiated HDR
    /// mode with fresh display metadata, advancing `hdr_revision` once after
    /// both fields are written so control loops send a single `0x010e`
    /// (`enabled=true`) carrying that metadata. Returns `true` when HDR turned
    /// back on.
    pub fn restore_hdr(&self, display: butterpollo_media_policy::hdr::Metadata) -> bool {
        if !self.config.hdr || self.hdr_active() {
            return false;
        }
        self.hdr_source.lock().unwrap().display = Some(display);
        *self.hdr_metadata.write().unwrap() = Some(display);
        self.hdr_active.store(true, Ordering::Release);
        self.hdr_revision.fetch_add(1, Ordering::Release);
        true
    }
    /// Records the captured source's HDR metadata for the encoder and client.
    pub fn set_display_hdr_metadata(
        &self,
        display: butterpollo_media_policy::hdr::Metadata,
    ) -> butterpollo_media_policy::hdr::Metadata {
        self.hdr_source.lock().unwrap().display = Some(display);
        *self.hdr_metadata.write().unwrap() = Some(display);
        // After the write, with release: a loop that reads the new number also
        // reads the new metadata; one that misses it re-reads on its next pass.
        self.hdr_revision.fetch_add(1, Ordering::Release);
        display
    }
    /// Record receiver capabilities for diagnostics. They describe the output
    /// panel, not the source pixels, so must not update stream HDR metadata;
    /// only the receiver display peak the encoder is told follows them.
    pub fn record_display_caps(
        &self,
        payload: &[u8],
    ) -> Option<butterpollo_media_policy::display_caps::Update> {
        self.hdr_source.lock().unwrap().on_payload(payload)
    }
    /// The receiver's display peak, for dynamic metadata that names a target
    /// display ([`butterpollo_media_policy::display_caps::HdrSource::target_nits`]).
    pub fn hdr_target_nits(&self) -> u16 {
        self.hdr_source.lock().unwrap().target_nits()
    }
}
