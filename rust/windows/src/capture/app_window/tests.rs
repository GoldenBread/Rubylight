use super::*;

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}
fn at(x: i32, y: i32) -> POINT {
    POINT { x, y }
}

#[test]
fn windows_ten_borders_are_left_out_of_the_client_area() {
    // A 1000x700 frame with 8 px side and bottom borders and a 31 px title bar.
    let frame = rect(100, 100, 1100, 800);
    let crop = client_box(frame, at(108, 131), (984, 661), (1000, 700)).unwrap();
    assert_eq!(
        crop,
        Crop {
            left: 8,
            top: 31,
            right: 992,
            bottom: 692,
        }
    );
    assert_eq!((crop.width(), crop.height()), (984, 661));
    // Both copy sites take the same rectangle.
    let d3d11 = crop.d3d11();
    assert_eq!(
        (d3d11.left, d3d11.top, d3d11.right, d3d11.bottom),
        (8, 31, 992, 692)
    );
    assert_eq!((d3d11.front, d3d11.back), (0, 1));
    let d3d12 = crop.d3d12();
    assert_eq!(
        (d3d12.left, d3d12.top, d3d12.right, d3d12.bottom),
        (8, 31, 992, 692)
    );
    assert_eq!((d3d12.front, d3d12.back), (0, 1));
}

#[test]
fn a_menu_bar_above_the_client_area_is_cropped_with_the_title_bar() {
    // Title bar 31 px, menu bar 24 px: the client starts 55 px below the frame.
    let frame = rect(0, 0, 1000, 800);
    let crop = client_box(frame, at(8, 55), (984, 745), (1000, 800)).unwrap();
    assert_eq!(crop.top, 55);
    assert_eq!(crop.bottom, 800);
    assert_eq!((crop.left, crop.right), (8, 992));
}

#[test]
fn a_client_area_larger_than_the_surface_is_capped_to_it() {
    let frame = rect(0, 0, 2000, 1200);
    let crop = client_box(frame, at(8, 31), (1984, 1161), (1920, 1080)).unwrap();
    assert_eq!(
        crop,
        Crop {
            left: 8,
            top: 31,
            right: 1920,
            bottom: 1080,
        }
    );
}

#[test]
fn offsets_before_the_frame_are_capped_but_leave_the_visible_client_pixels() {
    // The client starts 10 px left of and above the captured frame.
    let frame = rect(100, 50, 1100, 850);
    let crop = client_box(frame, at(90, 40), (100, 100), (1000, 800)).unwrap();
    assert_eq!(
        crop,
        Crop {
            left: 0,
            top: 0,
            right: 90,
            bottom: 90,
        }
    );
}

#[test]
fn a_window_that_draws_its_own_title_bar_keeps_the_whole_frame() {
    let frame = rect(0, 0, 1280, 720);
    let crop = client_box(frame, at(0, 0), (1280, 720), (1280, 720)).unwrap();
    assert_eq!((crop.left, crop.top), (0, 0));
    assert_eq!((crop.width(), crop.height()), (1280, 720));
}

#[test]
fn no_frame_or_an_empty_box_leaves_the_whole_frame() {
    // A closed or minimised window has no frame to crop.
    assert_eq!(client_box(rect(0, 0, 0, 0), at(0, 0), (0, 0), (0, 0)), None);
    assert_eq!(
        client_box(rect(10, 10, 10, 500), at(0, 0), (100, 100), (500, 500)),
        None
    );
    // No client area to show.
    let frame = rect(0, 0, 800, 600);
    assert_eq!(client_box(frame, at(0, 0), (0, 0), (800, 600)), None);
    assert_eq!(client_box(frame, at(0, 0), (-1, 100), (800, 600)), None);
    assert_eq!(client_box(frame, at(0, 0), (100, 100), (0, 0)), None);
    // The client area starts past the surface's right edge.
    assert_eq!(client_box(frame, at(800, 10), (100, 100), (800, 600)), None);
    assert_eq!(client_box(frame, at(10, 600), (100, 100), (800, 600)), None);
}
