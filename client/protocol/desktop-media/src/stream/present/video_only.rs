//! Linux HDR video-only frames through SDL's `SDL.renderer.gpu.video_only` path.

/// `0` keeps every HDR frame on the PQ pass through the back buffer, as before video-only frames.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const VIDEO_ONLY_ENVIRONMENT: &str = "NIGHTJAR_PQ_VIDEO_ONLY";

/// How long the first frame that lost draws holds video-only frames off; each further one doubles it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const VIDEO_ONLY_HOLD_US: u64 = 2_000_000;

/// After this many frames that lost draws, a stream draws no more video-only frames.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const VIDEO_ONLY_MISSES: u32 = 4;

/// What a frame on the window's target will draw, for whether it can be video only.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VideoOnlyFrame {
    /// The renderer presents through the patched SDL's PQ pass (`handheld::Output::Pq`).
    pub pq_output: bool,
    /// The plane pass draws the picture straight into the window's target (`draws_planes_direct`),
    /// with no upscaler or AI enhancement drawing it instead.
    pub planes_direct: bool,
    /// Nothing else draws there that SDL would draw itself: no Remote Input screen, and the
    /// overlay, a toast or the stream menu only as meshes that write PQ (`ui_pq::UiPq`).
    pub nothing_else: bool,
    /// A read-back of the frame is asked for (a screenshot), which reads the back buffer as scRGB.
    pub read_back: bool,
}

/// HDR frames that draw only the picture, through the patched SDL's `SDL.renderer.gpu.video_only`
/// (`vendor/sdl3-src/README.md`): the plane pass writes PQ into the HDR10 swapchain image itself,
/// and SDL skips the RGBA16F back buffer and its full-screen PQ pass. SDL drops its own draws in
/// such a frame and counts them; a frame that lost any (something drew that `VideoOnlyFrame` did
/// not foresee) holds video-only frames off for a while, longer each time, and after
/// `VIDEO_ONLY_MISSES` for the rest of the stream.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug)]
pub struct VideoOnly {
    switched_on: bool,
    /// The path was logged.
    said: bool,
    /// SDL's count of the draws it dropped, as last read.
    dropped_seen: i64,
    misses: u32,
    held_until_us: u64,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl VideoOnly {
    pub fn of(value: Option<&std::ffi::OsStr>) -> Self {
        Self { switched_on: !value.is_some_and(|value| value == "0"), said: false, dropped_seen: 0, misses: 0, held_until_us: 0 }
    }

    pub fn from_environment() -> Self {
        Self::of(std::env::var_os(VIDEO_ONLY_ENVIRONMENT).as_deref())
    }

    /// Whether to ask SDL for a video-only frame, and a line to log the first time one could be
    /// asked for while the environment switches them off.
    pub fn wanted(&mut self, frame: VideoOnlyFrame, now_us: u64) -> (bool, Option<&'static str>) {
        let could = frame.pq_output && frame.planes_direct && frame.nothing_else && !frame.read_back;
        let line = (could && !self.switched_on && !std::mem::replace(&mut self.said, true))
            .then_some("nightjar: HDR frames go through the back buffer and its PQ pass (NIGHTJAR_PQ_VIDEO_ONLY=0)");
        (could && self.switched_on && self.misses < VIDEO_ONLY_MISSES && now_us >= self.held_until_us, line)
    }

    /// What SDL settled for a frame asked to be video only; a line to log the first time.
    pub fn settled(&mut self, video_only: bool) -> Option<&'static str> {
        (!std::mem::replace(&mut self.said, true)).then_some(if video_only {
            "nightjar: HDR frames with only the picture take PQ straight from the plane pass, without the back buffer and its PQ pass"
        } else {
            "nightjar: SDL did not take a video-only frame; HDR frames go through the back buffer and its PQ pass"
        })
    }

    /// SDL's count of the draws it dropped in video-only frames, after a present; a line to log
    /// when this frame lost any.
    pub fn presented(&mut self, dropped: i64, now_us: u64) -> Option<String> {
        // A renderer made again counts from 0.
        let lost = if dropped < self.dropped_seen { 0 } else { dropped - self.dropped_seen };
        self.dropped_seen = dropped;
        if lost == 0 {
            return None;
        }
        self.misses += 1;
        if self.misses >= VIDEO_ONLY_MISSES {
            return Some(format!("nightjar: SDL dropped {lost} draws of a video-only HDR frame; HDR frames go through the PQ pass for the rest of the stream"));
        }
        let hold_us = VIDEO_ONLY_HOLD_US << (self.misses - 1);
        self.held_until_us = now_us + hold_us;
        Some(format!("nightjar: SDL dropped {lost} draws of a video-only HDR frame; HDR frames go through the PQ pass for {} s", hold_us / 1_000_000))
    }
}
