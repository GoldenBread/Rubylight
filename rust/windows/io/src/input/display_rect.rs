//! Display and streamed-window rectangle lookup and coordinate mapping for [`Injector`].

use super::*;

/// A display looked up on its own thread.
pub(super) struct Lookup {
    pub(super) output: String,
    pub(super) started: std::time::Instant,
    /// None once its result was used.
    pub(super) thread: Option<std::thread::JoinHandle<Result<RECT>>>,
}

/// How often a display's rectangle is read again, and a missing display
/// looked up again: the lookup enumerates every display.
pub(super) const RECT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
/// How often a streamed window's rectangle is read again: a window is moved
/// and resized far more often than a display, and reading it is cheap.
pub(super) const WINDOW_RECT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
/// The start of a window capture target (`capture::WindowTarget`).
const WINDOW_PREFIX: &str = "window:";
/// How often absolute input ignored for a missing display is reported.
const MISSING_DISPLAY_WARNING: std::time::Duration = std::time::Duration::from_secs(10);

/// How long the rectangle of `output` is used before it is read again.
pub(super) fn rect_interval(output: &str) -> std::time::Duration {
    if output.starts_with(WINDOW_PREFIX) {
        WINDOW_RECT_INTERVAL
    } else {
        RECT_INTERVAL
    }
}

#[derive(Clone, Copy)]
pub struct RectHooks {
    pub window_rect: fn(&str) -> Option<RECT>,
    pub display_name_for_id: fn(&str) -> Option<String>,
}
static RECT_HOOKS: std::sync::OnceLock<RectHooks> = std::sync::OnceLock::new();
pub fn set_rect_hooks(hooks: RectHooks) {
    let _ = RECT_HOOKS.set(hooks);
}

/// The desktop rectangle of `output`: a GDI name, a device ID, or empty for
/// the first display.
pub(super) fn display_rect(output: &str) -> Result<RECT> {
    // The app's window streamed in place of a display: input maps onto the
    // window's captured area, wherever it is (butterpollo_media_policy::app_window).
    if output.starts_with(WINDOW_PREFIX) {
        return RECT_HOOKS
            .get()
            .and_then(|hooks| (hooks.window_rect)(output))
            .ok_or_else(|| anyhow::anyhow!("input window missing"));
    }
    // Before capture publishes its GDI name, the configured display is
    // often a device ID ({...}); input arriving then was dropped.
    let name = RECT_HOOKS
        .get()
        .and_then(|hooks| (hooks.display_name_for_id)(output))
        .unwrap_or_else(|| output.to_owned());
    crate::os_helpers::active_displays_gdi()
        .into_iter()
        .find_map(|(display_name, rect)| {
            (output.is_empty() || display_name.eq_ignore_ascii_case(&name)).then_some(rect)
        })
        .ok_or_else(|| anyhow::anyhow!("input display missing"))
}

/// The rectangle after a periodic re-read (`read`): a display that could not be read keeps its
/// last one (a mode change mid-read), but a streamed window that is closed, minimised or replaced
/// has none, so absolute input waits instead of landing on whatever is there now. Relative input
/// needs no rectangle and goes on.
pub(super) fn reread_rect(output: &str, last: Option<RECT>, read: Option<RECT>) -> Option<RECT> {
    match read {
        Some(rect) => Some(rect),
        None if output.starts_with(WINDOW_PREFIX) => None,
        None => last,
    }
}

/// The current rectangle of a display by GDI name, read cheaply from its mode.
pub(super) fn current_rect(name: &str) -> Option<RECT> {
    if name.starts_with(WINDOW_PREFIX) {
        return RECT_HOOKS.get().and_then(|hooks| (hooks.window_rect)(name));
    }
    crate::os_helpers::current_display_rect_gdi(name)
}

/// A width and height, and a rectangle (left, top, width, height) in it.
pub(super) type Size = (u32, u32);
pub(super) type Picture = (u32, u32, u32, u32);

/// A desktop point as fractions of the stream, when it is on the display at
/// `rect` whose picture sits at `picture` in the stream (None: it fills it).
pub(super) fn cursor_in_stream(
    point: POINT,
    rect: RECT,
    picture: Option<(Size, Picture)>,
) -> Option<(f32, f32)> {
    let on = |value: i32, start: i32, end: i32| {
        (start < end && (start..end).contains(&value))
            .then(|| f64::from(value - start) / f64::from(end - start))
    };
    let (x, y) = (
        on(point.x, rect.left, rect.right)?,
        on(point.y, rect.top, rect.bottom)?,
    );
    let (x, y) = match picture {
        Some((stream, (left, top, width, height))) => (
            (f64::from(left) + x * f64::from(width)) / f64::from(stream.0),
            (f64::from(top) + y * f64::from(height)) / f64::from(stream.1),
        ),
        None => (x, y),
    };
    Some((x as f32, y as f32))
}

impl Injector {
    /// The stream's size, so absolute input skips the bars around a display of
    /// another shape, as the encoder adds them.
    pub fn set_stream_size(&mut self, width: u32, height: u32) {
        self.stream = (width > 0 && height > 0).then_some((width, height));
    }
    /// Where the display's picture sits in the stream, when it does not fill it.
    pub(super) fn picture(&self) -> Option<(Size, Picture)> {
        let stream = self.stream?;
        let rect = self.rect?;
        let display = (
            u32::try_from(rect.right - rect.left).ok()?,
            u32::try_from(rect.bottom - rect.top).ok()?,
        );
        butterpollo_media_policy::display_policy::picture(display, stream)
            .map(|picture| (stream, picture))
    }
    /// A point on the stream as fractions of the display.
    pub(super) fn on_display(&self, x: f32, y: f32) -> (f32, f32) {
        match self.picture() {
            Some((stream, picture)) => {
                let (x, y) = butterpollo_media_policy::display_policy::stream_to_picture(
                    (f64::from(x), f64::from(y)),
                    stream,
                    picture,
                );
                (x as f32, y as f32)
            }
            None => (x, y),
        }
    }
    /// The host cursor as fractions of the whole stream, bars included, or
    /// None off the stream's display or before it is known. The couch menu
    /// reads it only when a relative mouse's hold or click needs a position
    /// (docs/couch-menu-design.md 3.2).
    pub fn pointer_in_stream(&mut self) -> Option<(f32, f32)> {
        self.settle_lookup(false);
        let rect = self.rect?;
        let mut point = POINT::default();
        // SAFETY: `point` is a live POINT for the call to write.
        if let Err(error) = unsafe { GetCursorPos(&mut point) } {
            tracing::warn!(%error, "couch menu: the cursor position is unavailable");
            return None;
        }
        cursor_in_stream(point, rect, self.picture())
    }
    /// Map absolute input onto `output` from now on: the stream's display can
    /// be created, recreated or renamed after input began.
    pub fn set_output(&mut self, output: &str) {
        if output.is_empty() {
            return;
        }
        if self
            .lookup
            .as_ref()
            .is_some_and(|lookup| !lookup.output.eq_ignore_ascii_case(output))
        {
            self.lookup = None;
        }
        self.settle_lookup(false);
        let renamed = !output.eq_ignore_ascii_case(&self.output);
        if self.rect.is_none() {
            // Until the display exists, follow the session's name, and look
            // the same name up again only every RECT_INTERVAL.
            if renamed || self.rect_read.elapsed() >= RECT_INTERVAL {
                self.output = output.to_owned();
                self.resolve_rect();
            }
            return;
        }
        // The rectangle and the name change together once it is found; a
        // name not found is looked up again every RECT_INTERVAL.
        if renamed
            && self.lookup.as_ref().is_none_or(|lookup| {
                !lookup.output.eq_ignore_ascii_case(output)
                    || lookup.started.elapsed() >= RECT_INTERVAL
            })
        {
            self.look_up(output);
        }
    }
    /// Look up a display that did not exist yet.
    pub(super) fn resolve_rect(&mut self) {
        self.rect_read = std::time::Instant::now();
        let output = self.output.clone();
        self.look_up(&output);
    }
    /// Look `output` up on another thread, unless it is being looked up: the
    /// lookup enumerates every display, which took 1.5-2.5 ms and once 18 ms
    /// on the input thread.
    pub(super) fn look_up(&mut self, output: &str) {
        if self.lookup.as_ref().is_some_and(|lookup| {
            lookup.thread.is_some() && lookup.output.eq_ignore_ascii_case(output)
        }) {
            return;
        }
        let name = output.to_owned();
        let thread = std::thread::Builder::new()
            .name("display lookup".into())
            .spawn(move || display_rect(&name));
        self.lookup = Some(Lookup {
            output: output.to_owned(),
            started: std::time::Instant::now(),
            thread: None,
        });
        match thread {
            Ok(thread) => self.lookup.as_mut().unwrap().thread = Some(thread),
            Err(_) => self.found(output.to_owned(), display_rect(output)),
        }
    }
    /// Use the result of a lookup that finished; with `wait`, of one still
    /// running.
    pub(super) fn settle_lookup(&mut self, wait: bool) {
        let Some(lookup) = &mut self.lookup else {
            return;
        };
        let Some(thread) = lookup.thread.take_if(|thread| wait || thread.is_finished()) else {
            return;
        };
        let output = lookup.output.clone();
        if let Ok(rect) = thread.join() {
            self.found(output, rect);
        }
    }
    fn found(&mut self, output: String, rect: Result<RECT>) {
        match rect {
            Ok(rect) => {
                tracing::debug!(output, "input display found");
                self.rect = Some(rect);
                self.output = output;
                self.rect_read = std::time::Instant::now();
            }
            Err(error) => tracing::debug!(%error, output, "input display unavailable"),
        }
    }
    /// The display's rectangle. Until the display exists, absolute mouse,
    /// touch and pen input is ignored, and that is reported now and then.
    pub(super) fn display(&mut self) -> Option<RECT> {
        // Wait only for the initial display lookup; once a display rectangle is
        // known, rename/refresh lookups settle asynchronously so they never block
        // the ENet control thread.
        self.settle_lookup(self.rect.is_none());
        if self.rect.is_none()
            && self
                .display_warned
                .is_none_or(|at| at.elapsed() >= MISSING_DISPLAY_WARNING)
        {
            self.display_warned = Some(std::time::Instant::now());
            tracing::warn!(
                output = %self.output,
                "input display not found yet; absolute mouse, touch and pen input is ignored"
            );
        }
        self.rect
    }
}
