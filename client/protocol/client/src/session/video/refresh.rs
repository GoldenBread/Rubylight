//! Pacing of a failing decoder's key frame requests (`KeyFrameRequests`).

/// Paces the key frame requests of a decoder that keeps failing (Nightjar issue 77: a V4L2
/// decoder that could not set up its buffers answered every frame, the requested key frames
/// too, with [`DecodeStatus::NeedIdr`](super::DecodeStatus::NeedIdr), and the stream asked for
/// 99 key frames in 2 s while the app took a second to replace the decoder). The first request
/// goes out at once, so a single failure recovers as fast as before. It then stays outstanding
/// until a key frame decodes or the window passes. A requested key frame that arrives but fails
/// to decode does not clear it: that is the failing case, and the answer to an immediate new
/// request would fail alike. Asked again within the window, the request is deferred, not
/// dropped: it goes out when the window ends ([`held_until_us`](Self::held_until_us)), with the
/// next frame or, without one (a still picture the host does not resend), from the decoder
/// thread's timed wait or the receive thread's poll, so a decoder replaced meanwhile still gets
/// its key frame. Only the decoder's requests are paced; the receiver's own, for frames lost on
/// the way, are not. The receiver ([`VideoReceiver::request_decoder_refresh`]) stays as the C
/// core's, which the differential tests compare it with.
///
/// [`VideoReceiver::request_decoder_refresh`]: crate::video::VideoReceiver::request_decoder_refresh
pub(super) struct KeyFrameRequests {
    pub(super) window_us: u64,
    /// When the request not yet answered by a decoded key frame went out.
    sent_us: Option<u64>,
    /// The decoder asked again within the window.
    deferred: bool,
}

impl KeyFrameRequests {
    /// The window is the time the host needs to answer: a round trip, for which 100 ms allows
    /// Wi-Fi and most internet paths, plus four frame intervals for the host to wait for the
    /// next captured frame, encode it, send the key frame (several times an inter frame's size,
    /// paced over more than one interval) and for the decoder to take it. That is 133 ms at
    /// 120 fps and 167 ms at 60 fps, so a decoder failing on every frame makes at most 6 to 8
    /// requests a second instead of one per frame, and a request answered in time is never
    /// repeated. An unknown frame rate counts as 60 fps.
    pub(super) fn new(fps: i32) -> Self {
        let fps = if fps > 0 { fps as u64 } else { 60 };
        Self {
            window_us: 100_000 + 4 * 1_000_000 / fps,
            sent_us: None,
            deferred: false,
        }
    }

    /// A window of its own, for tests.
    #[cfg(test)]
    pub(super) fn with_window_us(window_us: u64) -> Self {
        Self {
            window_us,
            sent_us: None,
            deferred: false,
        }
    }

    /// When a deferred request falls due (session clock, us), if one waits.
    pub(super) fn held_until_us(&self) -> Option<u64> {
        if !self.deferred {
            return None;
        }
        self.sent_us.map(|sent| sent + self.window_us)
    }

    /// The decoder took a key frame: the request is answered and the next goes out at once.
    pub(super) fn key_frame_decoded(&mut self) {
        self.sent_us = None;
        self.deferred = false;
    }

    /// Whether to request a key frame after a frame the decoder answered with `need_idr`. The
    /// clock is read only when the decoder asks or a request waits.
    pub(super) fn request(&mut self, need_idr: bool, now_us: impl FnOnce() -> u64) -> bool {
        if !need_idr && !self.deferred {
            return false;
        }
        let now = now_us();
        match self.sent_us {
            Some(sent) if now.saturating_sub(sent) < self.window_us => {
                self.deferred = true;
                false
            }
            _ => {
                self.sent_us = Some(now);
                self.deferred = false;
                true
            }
        }
    }
}
