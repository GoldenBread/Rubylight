//! SDL3 window-placement queries and restoration, re-exporting pure placement types from
//! `nightjar-desktop-config::window_place_types`.

use sdl3_sys::everything::*;

pub use nightjar_desktop_config::window_place_types::{restore, Placement, Rect};

/// Each display's name and usable bounds (without the taskbar).
fn displays() -> Vec<(SDL_DisplayID, String, Rect)> {
    let mut count = 0;
    // SAFETY: SDL allocates the ID list, which is read here and then freed; names are owned by SDL.
    unsafe {
        let ids = SDL_GetDisplays(&mut count);
        if ids.is_null() {
            return Vec::new();
        }
        let list = (0..count.max(0) as usize)
            .map(|i| {
                let id = *ids.add(i);
                let mut r = SDL_Rect::default();
                SDL_GetDisplayUsableBounds(id, &mut r);
                let name = SDL_GetDisplayName(id);
                let name = if name.is_null() {
                    String::new()
                } else {
                    std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned()
                };
                (id, name, Rect { x: r.x, y: r.y, w: r.w, h: r.h })
            })
            .collect();
        SDL_free(ids.cast());
        list
    }
}

/// Each display's id and whole bounds.
pub fn display_bounds() -> Vec<(SDL_DisplayID, Rect)> {
    displays()
        .into_iter()
        .map(|(id, _, _)| {
            let mut r = SDL_Rect::default();
            // SAFETY: an id SDL just listed; on failure the bounds stay empty.
            unsafe { SDL_GetDisplayBounds(id, &mut r) };
            (id, Rect { x: r.x, y: r.y, w: r.w, h: r.h })
        })
        .collect()
}

/// Moves the hidden new window to the middle of the display that holds `point` (the second
/// screen's monitor). False, with the window left where it is, when no display holds it.
pub fn move_to_display_at(window: *mut SDL_Window, point: (i32, i32)) -> bool {
    // SAFETY: a live window on the main thread; the point is plain data.
    unsafe {
        let id = SDL_GetDisplayForPoint(&SDL_Point { x: point.0, y: point.1 });
        if id == 0 {
            return false;
        }
        let centred = SDL_WINDOWPOS_CENTERED_DISPLAY(id);
        SDL_SetWindowPosition(window, centred, centred)
    }
}

/// Moves the hidden new window to its saved place, or leaves it on the primary display (logged).
pub fn apply(window: *mut SDL_Window, saved: Option<&Placement>) {
    let Some(saved) = saved else { return };
    let named: Vec<(String, Rect)> = displays()
        .into_iter()
        .map(|(_, name, rect)| (name, rect))
        .collect();
    let (mut top, mut left, mut bottom, mut right) = (0, 0, 0, 0);
    // SAFETY: a live window on the main thread; on failure the borders stay 0.
    unsafe { SDL_GetWindowBordersSize(window, &mut top, &mut left, &mut bottom, &mut right) };
    let Some(rect) = restore(saved, &named, (top, left, bottom, right)) else {
        eprintln!(
            "nightjar: window: display \"{}\" is gone, opening on the primary display",
            saved.display
        );
        return;
    };
    // SAFETY: a live window on the main thread.
    unsafe {
        SDL_SetWindowSize(window, rect.w, rect.h);
        SDL_SetWindowPosition(window, rect.x, rect.y);
        if saved.maximized {
            SDL_MaximizeWindow(window);
        }
    }
}

/// The window's place now, to save at exit. A minimized or fullscreen window keeps the windowed
/// place saved before (`previous`) and only records the display it is on.
pub fn current(window: *mut SDL_Window, previous: Option<&Placement>) -> Option<Placement> {
    // SAFETY: a live window on the main thread.
    unsafe {
        let flags = SDL_GetWindowFlags(window);
        if flags & SDL_WINDOW_MINIMIZED != 0 {
            return None;
        }
        let id = SDL_GetDisplayForWindow(window);
        let (_, display, bounds) = displays().into_iter().find(|(d, _, _)| *d == id)?;
        let mut placement = previous.cloned().unwrap_or_default();
        if placement.display != display || placement.display_bounds != bounds {
            placement.window = Rect { x: 0, y: 0, w: placement.window.w, h: placement.window.h };
        }
        placement.display = display;
        placement.display_bounds = bounds;
        if flags & SDL_WINDOW_FULLSCREEN == 0 {
            placement.maximized = flags & SDL_WINDOW_MAXIMIZED != 0;
        }
        if flags & (SDL_WINDOW_FULLSCREEN | SDL_WINDOW_MAXIMIZED) == 0 {
            let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
            SDL_GetWindowPosition(window, &mut x, &mut y);
            SDL_GetWindowSize(window, &mut w, &mut h);
            placement.window = Rect { x: x - bounds.x, y: y - bounds.y, w, h };
        }
        (placement.window.w > 0 && placement.window.h > 0).then_some(placement)
    }
}

/// The smallest window, in the launcher's points: every page is laid out and checked down to this.
const MINIMUM: (f32, f32) = (720.0, 490.0);

fn minimum_size(scale: f32, room: Option<(i32, i32)>) -> (i32, i32) {
    let scale = scale.max(1.0);
    let (width, height) = (
        (MINIMUM.0 * scale).ceil() as i32,
        (MINIMUM.1 * scale).ceil() as i32,
    );
    match room {
        Some((w, h)) => (width.min(w.max(1)), height.min(h.max(1))),
        None => (width, height),
    }
}

fn usable_room(window: *mut SDL_Window) -> Option<(i32, i32)> {
    let mut usable = SDL_Rect { x: 0, y: 0, w: 0, h: 0 };
    let (mut top, mut left, mut bottom, mut right) = (0, 0, 0, 0);
    // SAFETY: the window is alive, on the main thread; all out pointers are live.
    unsafe {
        if !SDL_GetDisplayUsableBounds(SDL_GetDisplayForWindow(window), &mut usable) {
            eprintln!("nightjar: the display's usable area is unknown, the window's minimum is not capped");
            return None;
        }
        if !SDL_GetWindowBordersSize(window, &mut top, &mut left, &mut bottom, &mut right) {
            (top, left, bottom, right) = (0, 0, 0, 0);
        }
    }
    Some((usable.w - left - right, usable.h - top - bottom))
}

/// Keeps the window at [`MINIMUM`] points on the display it is on now, as far as that display can.
pub fn set_minimum_size(window: *mut SDL_Window) {
    let room = usable_room(window);
    // SAFETY: the window is alive, on the main thread.
    unsafe {
        let (width, height) = minimum_size(SDL_GetWindowDisplayScale(window), room);
        SDL_SetWindowMinimumSize(window, width, height);
    }
}

#[cfg(test)]
mod minimum_tests {
    use super::*;

    #[test]
    fn the_minimum_stays_the_same_in_points_on_any_display() {
        assert_eq!(minimum_size(1.0, None), (720, 490));
        assert_eq!(minimum_size(1.5, None), (1080, 735));
        assert_eq!(minimum_size(0.0, None), (720, 490), "a failed read");
        assert_eq!(minimum_size(1.0, Some((1920, 1040))), (720, 490), "room to spare");
    }

    #[test]
    fn the_minimum_never_exceeds_what_the_display_can_show() {
        assert_eq!(minimum_size(1.5, Some((1280, 680))), (1080, 680));
        assert_eq!(
            minimum_size(3.0, Some((1920, 1040))),
            (1920, 1040),
            "a small display at 300%"
        );
    }
}
