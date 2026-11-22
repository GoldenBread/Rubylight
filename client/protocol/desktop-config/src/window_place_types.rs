//! The window's saved place between launches: the display it was on, its bounds, and its
//! windowed position and size, stored in `window.json`.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// The display's name and its bounds then: two displays of one model share a name.
    pub display: String,
    pub display_bounds: Rect,
    /// The windowed position, relative to the display's top left corner, and size.
    pub window: Rect,
    pub maximized: bool,
}

/// Where the window opens: the saved display by name and bounds, else by name alone; None when
/// it is gone, so the window opens on the primary display. The window is moved and shrunk to fit
/// the display's usable area if that changed. `frame` is the window's border sizes (top, left,
/// bottom, right): positions are the client area's, and the title bar stays on the display too.
pub fn restore(saved: &Placement, displays: &[(String, Rect)], frame: (i32, i32, i32, i32)) -> Option<Rect> {
    let (_, usable) = displays
        .iter()
        .find(|(name, bounds)| *name == saved.display && *bounds == saved.display_bounds)
        .or_else(|| displays.iter().find(|(name, _)| *name == saved.display))?;
    let (top, left, bottom, right) = frame;
    let area = Rect {
        x: usable.x + left,
        y: usable.y + top,
        w: (usable.w - left - right).max(1),
        h: (usable.h - top - bottom).max(1),
    };
    let w = saved.window.w.clamp(1, area.w);
    let h = saved.window.h.clamp(1, area.h);
    let x = (usable.x + saved.window.x).clamp(area.x, area.x + area.w - w);
    let y = (usable.y + saved.window.y).clamp(area.y, area.y + area.h - h);
    Some(Rect { x, y, w, h })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(display: &str, bounds: Rect, window: Rect) -> Placement {
        Placement { display: display.into(), display_bounds: bounds, window, maximized: false }
    }

    const LEFT: Rect = Rect { x: 0, y: 0, w: 1920, h: 1040 };
    const RIGHT: Rect = Rect { x: 1920, y: 0, w: 2560, h: 1400 };
    const NO_FRAME: (i32, i32, i32, i32) = (0, 0, 0, 0);

    #[test]
    fn the_window_returns_to_its_display() {
        let displays = vec![("DELL U2720Q".to_owned(), LEFT), ("LG ULTRAGEAR".to_owned(), RIGHT)];
        let saved = placement("LG ULTRAGEAR", RIGHT, Rect { x: 100, y: 50, w: 1280, h: 800 });
        assert_eq!(restore(&saved, &displays, NO_FRAME), Some(Rect { x: 2020, y: 50, w: 1280, h: 800 }));
    }

    #[test]
    fn a_gone_display_falls_back_to_the_primary() {
        let displays = vec![("DELL U2720Q".to_owned(), LEFT)];
        let saved = placement("LG ULTRAGEAR", RIGHT, Rect { x: 100, y: 50, w: 1280, h: 800 });
        assert_eq!(restore(&saved, &displays, NO_FRAME), None);
    }

    #[test]
    fn twins_are_told_apart_by_bounds_and_a_moved_display_is_still_found() {
        let twins = vec![("DELL".to_owned(), LEFT), ("DELL".to_owned(), RIGHT)];
        let saved = placement("DELL", RIGHT, Rect { x: 0, y: 0, w: 1280, h: 800 });
        assert_eq!(restore(&saved, &twins, NO_FRAME).unwrap().x, 1920);
        let moved = Rect { x: -2560, ..RIGHT };
        let displays = vec![("DELL U2720Q".to_owned(), LEFT), ("LG".to_owned(), moved)];
        let saved = placement("LG", RIGHT, Rect { x: 10, y: 10, w: 1280, h: 800 });
        assert_eq!(restore(&saved, &displays, NO_FRAME), Some(Rect { x: -2550, y: 10, w: 1280, h: 800 }));
    }

    #[test]
    fn a_window_that_no_longer_fits_is_shrunk_and_kept_on_screen() {
        let small = Rect { x: 0, y: 0, w: 1280, h: 680 };
        let displays = vec![("TV".to_owned(), small)];
        let saved = placement("TV", LEFT, Rect { x: 1500, y: 900, w: 1600, h: 900 });
        assert_eq!(restore(&saved, &displays, NO_FRAME), Some(Rect { x: 0, y: 0, w: 1280, h: 680 }));
        let saved = placement("TV", small, Rect { x: 1000, y: -40, w: 800, h: 600 });
        assert_eq!(restore(&saved, &displays, NO_FRAME), Some(Rect { x: 480, y: 0, w: 800, h: 600 }));
        // A 31 px title bar and 8 px borders stay on the display as well.
        assert_eq!(restore(&saved, &displays, (31, 8, 8, 8)), Some(Rect { x: 472, y: 31, w: 800, h: 600 }));
        let saved = placement("TV", small, Rect { x: 0, y: 0, w: 1280, h: 680 });
        assert_eq!(restore(&saved, &displays, (31, 8, 8, 8)), Some(Rect { x: 8, y: 31, w: 1264, h: 641 }));
    }

    #[test]
    fn the_file_round_trips() {
        let saved = Placement { maximized: true, ..placement("LG", RIGHT, Rect { x: 1, y: 2, w: 3, h: 4 }) };
        let json = serde_json::to_string(&saved).unwrap();
        assert_eq!(serde_json::from_str::<Placement>(&json).unwrap(), saved);
    }
}
