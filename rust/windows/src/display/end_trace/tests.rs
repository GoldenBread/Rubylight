use super::*;

fn rect(left: i32, top: i32, width: i32, height: i32) -> Rect {
    Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}
fn window(handle: u64, monitor: &str, rect: Rect, dpi: u32) -> WindowState {
    WindowState {
        handle,
        pid: 100 + handle as u32,
        visible: true,
        monitor: monitor.into(),
        rect,
        dpi,
    }
}

#[test]
fn displays_are_described_with_position_size_and_scale() {
    let displays = [
        DisplayState {
            name: r"\\.\DISPLAY1".into(),
            label: "DELL U2720Q".into(),
            virtual_display: false,
            primary: true,
            rect: rect(0, 0, 2560, 1440),
            dpi: Some(120),
        },
        DisplayState {
            name: r"\\.\DISPLAY2".into(),
            label: String::new(),
            virtual_display: false,
            primary: false,
            rect: rect(-1080, -240, 1080, 1920),
            dpi: Some(96),
        },
        DisplayState {
            name: r"\\.\DISPLAY9".into(),
            label: "Rubylight".into(),
            virtual_display: true,
            primary: false,
            rect: rect(2560, 0, 1920, 1080),
            dpi: None,
        },
    ];
    assert_eq!(
        describe_displays(&displays),
        r#"\\.\DISPLAY1 "DELL U2720Q" 0,0 2560x1440 120dpi(125%) primary; \\.\DISPLAY2 -1080,-240 1080x1920 96dpi(100%); \\.\DISPLAY9 "Rubylight" virtual 2560,0 1920x1080 dpi unreadable"#
    );
    assert_eq!(primary_of(&displays), r"\\.\DISPLAY1");
    assert_eq!(describe_displays(&[]), "none");
    assert_eq!(primary_of(&[]), "none");
}

#[test]
fn scales_round_to_the_windows_setting() {
    assert_eq!(scale_percent(96), 100);
    assert_eq!(scale_percent(120), 125);
    assert_eq!(scale_percent(144), 150);
    assert_eq!(scale_percent(168), 175);
}

#[test]
fn only_windows_that_changed_display_dpi_or_size_are_listed() {
    let before = [
        window(1, "V", rect(0, 0, 800, 600), 120),
        window(2, "V", rect(0, 0, 800, 600), 120),
        window(3, "V", rect(10, 10, 800, 600), 120),
        window(4, "V", rect(0, 0, 800, 600), 120),
        window(5, "A", rect(0, 0, 800, 600), 120),
    ];
    let after = [
        // Moved on the same display at the same size and DPI: not listed.
        window(1, "V", rect(50, 50, 800, 600), 120),
        // Another display.
        window(2, "A", rect(0, 0, 800, 600), 120),
        // Another DPI.
        window(3, "V", rect(10, 10, 800, 600), 96),
        // Another size.
        window(4, "V", rect(0, 0, 1000, 750), 120),
        // Opened during the restore: no before to compare with.
        window(6, "A", rect(0, 0, 800, 600), 96),
    ];
    let (changes, total) = changed_windows(&before, &after, WINDOW_LINES);
    assert_eq!(total, 3);
    let handles: Vec<_> = changes.iter().map(|c| c.after.handle).collect();
    // Resized first, then rescaled, then moved.
    assert_eq!(handles, [4, 3, 2]);
}

#[test]
fn a_reused_handle_of_another_process_is_another_window() {
    let before = [window(1, "V", rect(0, 0, 800, 600), 120)];
    let mut reused = window(1, "A", rect(0, 0, 400, 300), 96);
    reused.pid = 999;
    let (changes, total) = changed_windows(&before, &[reused], WINDOW_LINES);
    assert!(changes.is_empty());
    assert_eq!(total, 0);
}

#[test]
fn visible_windows_come_before_hidden_ones_of_the_same_kind() {
    let before = [
        window(1, "V", rect(0, 0, 800, 600), 120),
        window(2, "V", rect(0, 0, 800, 600), 120),
    ];
    let mut hidden = window(1, "A", rect(0, 0, 800, 600), 120);
    hidden.visible = false;
    let after = [hidden, window(2, "A", rect(0, 0, 800, 600), 120)];
    let (changes, _) = changed_windows(&before, &after, WINDOW_LINES);
    let handles: Vec<_> = changes.iter().map(|c| c.after.handle).collect();
    assert_eq!(handles, [2, 1]);
}

#[test]
fn the_cap_holds_and_keeps_the_resized_windows() {
    let before: Vec<_> = (0..100)
        .map(|i| window(i, "V", rect(0, 0, 800, 600), 120))
        .collect();
    let after: Vec<_> = (0..100)
        .map(|i| {
            // Every window changed display; the last three also changed size.
            let size = if i >= 97 { 1000 } else { 800 };
            window(i, "A", rect(0, 0, size, 600), 120)
        })
        .collect();
    let (changes, total) = changed_windows(&before, &after, WINDOW_LINES);
    assert_eq!(total, 100);
    assert_eq!(changes.len(), WINDOW_LINES);
    let first: Vec<_> = changes.iter().take(3).map(|c| c.after.handle).collect();
    assert_eq!(first, [97, 98, 99]);
    let (none, total) = changed_windows(&before, &after, 0);
    assert!(none.is_empty());
    assert_eq!(total, 100);
}

#[test]
fn a_change_is_described_without_a_title() {
    let mut after = window(1, r"\\.\DISPLAY1", rect(0, 0, 1505, 1000), 120);
    after.visible = false;
    let change = Change {
        before: window(1, r"\\.\DISPLAY9", rect(2560, 0, 1200, 800), 120),
        after,
    };
    assert_eq!(
        describe_change(&change, "AJAZZ.exe"),
        r"AJAZZ.exe hidden \\.\DISPLAY9 -> \\.\DISPLAY1, 2560,0 1200x800 -> 0,0 1505x1000, dpi 120 -> 120"
    );
    let mut lost = change.clone();
    lost.after.monitor.clear();
    assert!(describe_change(&lost, "?").contains(r"\\.\DISPLAY9 -> none"));
}

#[test]
fn only_the_executable_name_is_kept() {
    assert_eq!(
        exe_name(r"C:\Users\someone\AppData\Local\App\app.exe"),
        "app.exe"
    );
    assert_eq!(exe_name("app.exe"), "app.exe");
    assert_eq!(exe_name(""), "");
}

#[test]
fn outcomes_name_each_step() {
    assert_eq!(describe_outcomes(&[]), "none");
    assert_eq!(
        describe_outcomes(&[("saved entry", true), ("user's layout", false)]),
        "saved entry ok, user's layout failed"
    );
}

#[test]
fn hooks_do_nothing_unless_armed() {
    // No trace on this thread: neither touches a display nor arms anything.
    phase("entry: displays on");
    committed("switch on", true);
    assert!(ARMED.with_borrow(|armed| armed.is_none()));
}
