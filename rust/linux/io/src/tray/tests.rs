//! The tray's pure parts: the icon's pictures, its menu, its tooltip and the notifications' text.
use super::{Action, item, menu, notices, pixmap};
use butterpollo_session::tray::Icon;

/// An `.ico` holding `pngs`, as the host's `apollo.ico` does.
fn ico(pngs: &[Vec<u8>]) -> Vec<u8> {
    let mut file = vec![0, 0, 1, 0];
    file.extend((pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len();
    for png in pngs {
        file.extend([0u8; 8]);
        file.extend((png.len() as u32).to_le_bytes());
        file.extend((offset as u32).to_le_bytes());
        offset += png.len();
    }
    for png in pngs {
        file.extend(png);
    }
    file
}

/// A `side`×`side` RGBA PNG of one colour.
fn png(side: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, side, side);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let pixels: Vec<u8> = std::iter::repeat_n(rgba, (side * side) as usize)
        .flatten()
        .collect();
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&pixels).unwrap();
    drop(writer);
    bytes
}

#[test]
fn icon_images_are_read_from_the_ico_smallest_first_up_to_128() {
    let file = ico(&[
        png(32, [1, 2, 3, 255]),
        png(16, [1, 2, 3, 255]),
        png(256, [1, 2, 3, 255]),
    ]);
    let sizes: Vec<u32> = pixmap::ico_images(&file)
        .iter()
        .map(|image| image.width)
        .collect();
    assert_eq!(sizes, [16, 32]);
}

#[test]
fn anything_but_an_ico_has_no_images() {
    assert!(pixmap::ico_images(b"not an icon").is_empty());
    assert!(pixmap::ico_images(&[]).is_empty());
    // An entry that points past the end of the file is left out.
    let mut file = ico(&[png(16, [0, 0, 0, 255])]);
    file.truncate(30);
    assert!(pixmap::ico_images(&file).is_empty());
}

#[test]
fn the_icon_is_argb_and_badged_while_streaming() {
    let images = pixmap::ico_images(&ico(&[png(16, [0x10, 0x20, 0x30, 0xff])]));
    let (width, height, idle) = pixmap::pixmap(&images[0], Icon::Idle);
    assert_eq!((width, height), (16, 16));
    assert_eq!(&idle[..4], &[0xff, 0x10, 0x20, 0x30]);
    let (_, _, streaming) = pixmap::pixmap(&images[0], Icon::Streaming);
    // The dot is in the bottom-right corner, the top-left is the picture.
    assert_eq!(&streaming[..4], &idle[..4]);
    let centre = ((12 * 16) + 12) * 4;
    assert_ne!(&streaming[centre..centre + 4], &idle[centre..centre + 4]);
    let pixmaps = pixmap::pixmaps(&images);
    assert_eq!(pixmaps.map(|sizes| sizes.len()), [1, 1, 1]);
}

#[test]
fn the_menu_is_the_windows_one() {
    let labels = |entries: Vec<menu::Entry>| -> Vec<(i32, String, bool)> {
        entries
            .into_iter()
            .map(|entry| match entry {
                menu::Entry::Item { id, label, enabled } => (id, label, enabled),
                menu::Entry::Separator { id } => (id, "-".into(), true),
            })
            .collect()
    };
    let owned = |items: &[(i32, &str, bool)]| -> Vec<(i32, String, bool)> {
        items
            .iter()
            .map(|(id, l, e)| (*id, l.to_string(), *e))
            .collect()
    };
    assert_eq!(
        labels(menu::entries(true, Some("Game"))),
        owned(&[
            (1, "Open Rubylight", true),
            (7, "Pair a device", true),
            (6, "Check for updates", true),
        ])
    );
    assert_eq!(
        labels(menu::entries(false, None)),
        owned(&[
            (1, "Open Rubylight", true),
            (7, "Pair a device", true),
            (6, "Check for updates", true),
            (2, "Disconnect clients", true),
            (5, "No app running", false),
            (8, "Back up settings", true),
            (9, "Restore settings...", true),
            (3, "Restart", true),
            (10, "-", true),
            (4, "Quit", true),
        ])
    );
    // An underscore in the app's name is shown, not taken as a shortcut.
    assert!(labels(menu::entries(false, Some("My_Game"))).contains(&(
        5,
        "Quit My__Game".into(),
        true
    )));
}

#[test]
fn each_menu_item_does_what_it_does_on_windows() {
    let expected = [
        (1, Action::Open),
        (2, Action::StopSessions),
        (3, Action::Restart),
        (4, Action::Quit),
        (5, Action::QuitApp),
        (6, Action::CheckUpdates),
        (7, Action::PairDevice),
        (8, Action::BackupSettings),
        (9, Action::RestoreSettings),
    ];
    for (id, action) in expected {
        assert_eq!(menu::action(id), Some(action), "item {id}");
    }
    assert_eq!(menu::action(0), None);
    assert_eq!(menu::action(10), None);
}

#[test]
fn the_tooltip_says_what_runs() {
    assert_eq!(item::tool_tip_text(47990, None), "Web port 47990");
    assert_eq!(
        item::tool_tip_text(47990, Some("Streaming Game")),
        "Streaming Game"
    );
}

#[test]
fn notification_text_is_not_markup() {
    assert_eq!(notices::plain("a <b> & c"), "a &lt;b&gt; &amp; c");
}

#[test]
fn a_launch_error_says_how_to_end_the_stream() {
    let (title, text) = notices::launch_error_notice("Game", u32::MAX);
    assert_eq!(title, "Launch Error");
    assert_eq!(
        text,
        "Application Game exited too fast with code -1. Click here to terminate the stream."
    );
}

#[test]
fn without_a_tray_icon_no_notification_goes_up() {
    assert!(super::try_notify("Title", "Text").is_err());
}
