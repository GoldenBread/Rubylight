//! HID report descriptors and Precision Touchpad certification constants for the virtual mouse
//! and virtual Precision Touchpad.
//!
//! The mouse report descriptor is our own (relative + resolution multipliers + absolute).
//! The Precision Touchpad certification status blob is from imbushuo's mac-precision-touchpad
//! (`AmtPtpDeviceSpiKm/Hid.h`, MIT, Copyright 2018 Bingxing Wang;
//! `third-party/licenses/mac-precision-touchpad-LICENSE.txt`).

pub use rubylight_wire::input::{Contact, MAX_CONTACTS};

pub(super) const RELATIVE_ID: u8 = 1;
pub(super) const ABSOLUTE_ID: u8 = 2;
pub(super) const MULTIPLIER_ID: u8 = 3;
/// The wheel's resolution multiplier once Windows sets it (Logitech's eight steps a notch).
pub const WHEEL_MULTIPLIER: i32 = 8;
/// The absolute collection's logical maximum on both axes.
pub const ABSOLUTE_MAX: u16 = 32767;
/// Windows' wheel delta of one notch.
pub(super) const NOTCH: i32 = 120;

/// The mouse's report descriptor: a relative mouse with a high-resolution wheel and pan
/// (report 1; feature report 3 holds both resolution multipliers, the wheel's in bits 0-1 and
/// the pan's in bits 2-3, as in Microsoft's enhanced wheel layout) and an absolute mouse
/// (report 2).
pub const MOUSE_REPORT_DESCRIPTOR: &[u8] = &[
    0x05,
    0x01, // Usage Page (Generic Desktop)
    0x09,
    0x02, // Usage (Mouse)
    0xa1,
    0x01, // Collection (Application)
    0x85,
    RELATIVE_ID, //   Report ID (1)
    0x09,
    0x01, //   Usage (Pointer)
    0xa1,
    0x00, //   Collection (Physical)
    0x05,
    0x09, //     Usage Page (Button)
    0x19,
    0x01, //     Usage Minimum (1)
    0x29,
    0x05, //     Usage Maximum (5)
    0x15,
    0x00, //     Logical Minimum (0)
    0x25,
    0x01, //     Logical Maximum (1)
    0x75,
    0x01, //     Report Size (1)
    0x95,
    0x05, //     Report Count (5)
    0x81,
    0x02, //     Input (Data, Var, Abs)
    0x75,
    0x03, //     Report Size (3)
    0x95,
    0x01, //     Report Count (1)
    0x81,
    0x01, //     Input (Const)
    0x05,
    0x01, //     Usage Page (Generic Desktop)
    0x09,
    0x30, //     Usage (X)
    0x09,
    0x31, //     Usage (Y)
    0x16,
    0x01,
    0x80, //     Logical Minimum (-32767)
    0x26,
    0xff,
    0x7f, //     Logical Maximum (32767)
    0x75,
    0x10, //     Report Size (16)
    0x95,
    0x02, //     Report Count (2)
    0x81,
    0x06, //     Input (Data, Var, Rel)
    0xa1,
    0x02, //     Collection (Logical)
    0x85,
    MULTIPLIER_ID, //       Report ID (3)
    0x09,
    0x48, //       Usage (Resolution Multiplier)
    0x15,
    0x00, //       Logical Minimum (0)
    0x25,
    0x01, //       Logical Maximum (1)
    0x35,
    0x01, //       Physical Minimum (1)
    0x45,
    WHEEL_MULTIPLIER as u8, //       Physical Maximum (8)
    0x75,
    0x02, //       Report Size (2)
    0x95,
    0x01, //       Report Count (1)
    0xb1,
    0x02, //       Feature (Data, Var, Abs)
    0x85,
    RELATIVE_ID, //       Report ID (1)
    0x35,
    0x00, //       Physical Minimum (0)
    0x45,
    0x00, //       Physical Maximum (0)
    0x09,
    0x38, //       Usage (Wheel)
    0x15,
    0x81, //       Logical Minimum (-127)
    0x25,
    0x7f, //       Logical Maximum (127)
    0x75,
    0x08, //       Report Size (8)
    0x95,
    0x01, //       Report Count (1)
    0x81,
    0x06, //       Input (Data, Var, Rel)
    0xc0, //     End Collection
    0xa1,
    0x02, //     Collection (Logical)
    0x85,
    MULTIPLIER_ID, //       Report ID (3)
    0x09,
    0x48, //       Usage (Resolution Multiplier)
    0x15,
    0x00, //       Logical Minimum (0)
    0x25,
    0x01, //       Logical Maximum (1)
    0x35,
    0x01, //       Physical Minimum (1)
    0x45,
    WHEEL_MULTIPLIER as u8, //       Physical Maximum (8)
    0x75,
    0x02, //       Report Size (2)
    0x95,
    0x01, //       Report Count (1)
    0xb1,
    0x02, //       Feature (Data, Var, Abs)
    0x35,
    0x00, //       Physical Minimum (0)
    0x45,
    0x00, //       Physical Maximum (0)
    0x75,
    0x04, //       Report Size (4)
    0xb1,
    0x01, //       Feature (Const)
    0x85,
    RELATIVE_ID, //       Report ID (1)
    0x05,
    0x0c, //       Usage Page (Consumer)
    0x0a,
    0x38,
    0x02, //       Usage (AC Pan)
    0x15,
    0x81, //       Logical Minimum (-127)
    0x25,
    0x7f, //       Logical Maximum (127)
    0x75,
    0x08, //       Report Size (8)
    0x95,
    0x01, //       Report Count (1)
    0x81,
    0x06, //       Input (Data, Var, Rel)
    0xc0, //     End Collection
    0xc0, //   End Collection
    0xc0, // End Collection
    0x05,
    0x01, // Usage Page (Generic Desktop)
    0x09,
    0x02, // Usage (Mouse)
    0xa1,
    0x01, // Collection (Application)
    0x85,
    ABSOLUTE_ID, //   Report ID (2)
    0x09,
    0x01, //   Usage (Pointer)
    0xa1,
    0x00, //   Collection (Physical)
    0x05,
    0x09, //     Usage Page (Button)
    0x19,
    0x01, //     Usage Minimum (1)
    0x29,
    0x05, //     Usage Maximum (5)
    0x15,
    0x00, //     Logical Minimum (0)
    0x25,
    0x01, //     Logical Maximum (1)
    0x75,
    0x01, //     Report Size (1)
    0x95,
    0x05, //     Report Count (5)
    0x81,
    0x02, //     Input (Data, Var, Abs)
    0x75,
    0x03, //     Report Size (3)
    0x95,
    0x01, //     Report Count (1)
    0x81,
    0x01, //     Input (Const)
    0x05,
    0x01, //     Usage Page (Generic Desktop)
    0x09,
    0x30, //     Usage (X)
    0x09,
    0x31, //     Usage (Y)
    0x15,
    0x00, //     Logical Minimum (0)
    0x26,
    0xff,
    0x7f, //     Logical Maximum (32767)
    0x75,
    0x10, //     Report Size (16)
    0x95,
    0x02, //     Report Count (2)
    0x81,
    0x02, //     Input (Data, Var, Abs)
    0xc0, //   End Collection
    0xc0, // End Collection
];

/// The certification status feature report's blob (Usage 0xC5 of page 0xFF00), as
/// open-source Precision Touchpad drivers send it (mac-precision-touchpad, MIT).
pub const PTP_CERTIFICATION_BLOB: [u8; 256] = [
    0xfc, 0x28, 0xfe, 0x84, 0x40, 0xcb, 0x9a, 0x87, 0x0d, 0xbe, 0x57, 0x3c, 0xb6, 0x70, 0x09, 0x88,
    0x07, 0x97, 0x2d, 0x2b, 0xe3, 0x38, 0x34, 0xb6, 0x6c, 0xed, 0xb0, 0xf7, 0xe5, 0x9c, 0xf6, 0xc2,
    0x2e, 0x84, 0x1b, 0xe8, 0xb4, 0x51, 0x78, 0x43, 0x1f, 0x28, 0x4b, 0x7c, 0x2d, 0x53, 0xaf, 0xfc,
    0x47, 0x70, 0x1b, 0x59, 0x6f, 0x74, 0x43, 0xc4, 0xf3, 0x47, 0x18, 0x53, 0x1a, 0xa2, 0xa1, 0x71,
    0xc7, 0x95, 0x0e, 0x31, 0x55, 0x21, 0xd3, 0xb5, 0x1e, 0xe9, 0x0c, 0xba, 0xec, 0xb8, 0x89, 0x19,
    0x3e, 0xb3, 0xaf, 0x75, 0x81, 0x9d, 0x53, 0xb9, 0x41, 0x57, 0xf4, 0x6d, 0x39, 0x25, 0x29, 0x7c,
    0x87, 0xd9, 0xb4, 0x98, 0x45, 0x7d, 0xa7, 0x26, 0x9c, 0x65, 0x3b, 0x85, 0x68, 0x89, 0xd7, 0x3b,
    0xbd, 0xff, 0x14, 0x67, 0xf2, 0x2b, 0xf0, 0x2a, 0x41, 0x54, 0xf0, 0xfd, 0x2c, 0x66, 0x7c, 0xf8,
    0xc0, 0x8f, 0x33, 0x13, 0x03, 0xf1, 0xd3, 0xc1, 0x0b, 0x89, 0xd9, 0x1b, 0x62, 0xcd, 0x51, 0xb7,
    0x80, 0xb8, 0xaf, 0x3a, 0x10, 0xc1, 0x8a, 0x5b, 0xe8, 0x8a, 0x56, 0xf0, 0x8c, 0xaa, 0xfa, 0x35,
    0xe9, 0x42, 0xc4, 0xd8, 0x55, 0xc3, 0x38, 0xcc, 0x2b, 0x53, 0x5c, 0x69, 0x52, 0xd5, 0xc8, 0x73,
    0x02, 0x38, 0x7c, 0x73, 0xb6, 0x41, 0xe7, 0xff, 0x05, 0xd8, 0x2b, 0x79, 0x9a, 0xe2, 0x34, 0x60,
    0x8f, 0xa3, 0x32, 0x1f, 0x09, 0x78, 0x62, 0xbc, 0x80, 0xe3, 0x0f, 0xbd, 0x65, 0x20, 0x08, 0x13,
    0xc1, 0xe2, 0xee, 0x53, 0x2d, 0x86, 0x7e, 0xa7, 0x5a, 0xc5, 0xd3, 0x7d, 0x98, 0xbe, 0x31, 0x48,
    0x1f, 0xfb, 0xda, 0xaf, 0xa2, 0xa8, 0x6a, 0x89, 0xd6, 0xbf, 0xf2, 0xd3, 0x32, 0x2a, 0x9a, 0xe4,
    0xcf, 0x17, 0xb7, 0xb8, 0xf4, 0xe1, 0x33, 0x08, 0x24, 0x8b, 0xc4, 0x43, 0xa5, 0xe5, 0x24, 0xc2,
];

/// The size a client that reports none gets: a typical tablet keyboard cover's touchpad.
pub const DEFAULT_SIZE_MM: (u16, u16) = (105, 70);
pub(super) const INPUT_MODE_ID: u8 = 3;
pub(super) const TOUCH_ID: u8 = 4;
pub(super) const CAPABILITIES_ID: u8 = 5;
pub(super) const CERTIFICATION_ID: u8 = 6;
pub(super) const FUNCTION_SWITCH_ID: u8 = 7;
/// Input mode Windows sets to take touchpad reports instead of a mouse's.
pub const INPUT_MODE_TOUCHPAD: u8 = 3;
/// One report: ID, five fingers of five bytes, scan time, contact count and the button.
pub(super) const TOUCH_REPORT_BYTES: usize = 1 + MAX_CONTACTS * 5 + 2 + 1 + 1;

/// The touchpad's report descriptor for a surface of `width_mm` by `height_mm`: one logical
/// unit is 0.1 mm.
pub fn touchpad_report_descriptor(width_mm: u16, height_mm: u16) -> Vec<u8> {
    let (x, y) = touchpad_extent(width_mm, height_mm);
    let [xl, xh] = x.to_le_bytes();
    let [yl, yh] = y.to_le_bytes();
    let mut d = vec![
        0x05, 0x0d, // Usage Page (Digitizer)
        0x09, 0x05, // Usage (Touch Pad)
        0xa1, 0x01, // Collection (Application)
        0x85, TOUCH_ID, //   Report ID (4)
    ];
    for _ in 0..MAX_CONTACTS {
        d.extend_from_slice(&[
            0x05, 0x0d, //   Usage Page (Digitizer)
            0x09, 0x22, //   Usage (Finger)
            0xa1, 0x02, //   Collection (Logical)
            0x15, 0x00, //     Logical Minimum (0)
            0x25, 0x01, //     Logical Maximum (1)
            0x75, 0x01, //     Report Size (1)
            0x95, 0x01, //     Report Count (1)
            0x09, 0x47, //     Usage (Confidence)
            0x81, 0x02, //     Input (Data, Var, Abs)
            0x09, 0x42, //     Usage (Tip Switch)
            0x81, 0x02, //     Input (Data, Var, Abs)
            0x25, 0x07, //     Logical Maximum (7)
            0x75, 0x03, //     Report Size (3)
            0x09, 0x51, //     Usage (Contact Identifier)
            0x81, 0x02, //     Input (Data, Var, Abs)
            0x81, 0x03, //     Input (Const): 3 bits
            0x05, 0x01, //     Usage Page (Generic Desktop)
            0x15, 0x00, //     Logical Minimum (0)
            0x26, xl, xh, //     Logical Maximum (width in 0.1 mm)
            0x75, 0x10, //     Report Size (16)
            0x55, 0x0e, //     Unit Exponent (-2)
            0x65, 0x11, //     Unit (SI Linear: centimetre)
            0x09, 0x30, //     Usage (X)
            0x35, 0x00, //     Physical Minimum (0)
            0x46, xl, xh, //     Physical Maximum (width)
            0x95, 0x01, //     Report Count (1)
            0x81, 0x02, //     Input (Data, Var, Abs)
            0x26, yl, yh, //     Logical Maximum (height in 0.1 mm)
            0x46, yl, yh, //     Physical Maximum (height)
            0x09, 0x31, //     Usage (Y)
            0x81, 0x02, //     Input (Data, Var, Abs)
            0x45, 0x00, //     Physical Maximum (0)
            0x55, 0x00, //     Unit Exponent (0)
            0x65, 0x00, //     Unit (None)
            0xc0, //   End Collection
        ]);
    }
    d.extend_from_slice(&[
        0x05,
        0x0d, //   Usage Page (Digitizer)
        0x55,
        0x0c, //   Unit Exponent (-4)
        0x66,
        0x01,
        0x10, //   Unit (SI Linear: second)
        0x47,
        0xff,
        0xff,
        0x00,
        0x00, //   Physical Maximum (65535)
        0x27,
        0xff,
        0xff,
        0x00,
        0x00, //   Logical Maximum (65535)
        0x75,
        0x10, //   Report Size (16)
        0x95,
        0x01, //   Report Count (1)
        0x09,
        0x56, //   Usage (Scan Time)
        0x81,
        0x02, //   Input (Data, Var, Abs)
        0x45,
        0x00, //   Physical Maximum (0)
        0x55,
        0x00, //   Unit Exponent (0)
        0x65,
        0x00, //   Unit (None)
        0x09,
        0x54, //   Usage (Contact Count)
        0x25,
        0x7f, //   Logical Maximum (127)
        0x75,
        0x08, //   Report Size (8)
        0x81,
        0x02, //   Input (Data, Var, Abs)
        0x05,
        0x09, //   Usage Page (Button)
        0x09,
        0x01, //   Usage (Button 1)
        0x25,
        0x01, //   Logical Maximum (1)
        0x75,
        0x01, //   Report Size (1)
        0x81,
        0x02, //   Input (Data, Var, Abs)
        0x95,
        0x07, //   Report Count (7)
        0x81,
        0x03, //   Input (Const)
        0x05,
        0x0d, //   Usage Page (Digitizer)
        0x85,
        CAPABILITIES_ID, //   Report ID (5)
        0x09,
        0x55, //   Usage (Contact Count Maximum)
        0x09,
        0x59, //   Usage (Pad Type)
        0x25,
        0x0f, //   Logical Maximum (15)
        0x75,
        0x04, //   Report Size (4)
        0x95,
        0x02, //   Report Count (2)
        0xb1,
        0x02, //   Feature (Data, Var, Abs)
        0x06,
        0x00,
        0xff, //   Usage Page (Vendor 0xFF00)
        0x85,
        CERTIFICATION_ID, //   Report ID (6)
        0x09,
        0xc5, //   Usage (Certification Status)
        0x15,
        0x00, //   Logical Minimum (0)
        0x26,
        0xff,
        0x00, //   Logical Maximum (255)
        0x75,
        0x08, //   Report Size (8)
        0x96,
        0x00,
        0x01, //   Report Count (256)
        0xb1,
        0x02, //   Feature (Data, Var, Abs)
        0xc0, // End Collection
        0x05,
        0x0d, // Usage Page (Digitizer)
        0x09,
        0x0e, // Usage (Device Configuration)
        0xa1,
        0x01, // Collection (Application)
        0x85,
        INPUT_MODE_ID, //   Report ID (3)
        0x09,
        0x22, //   Usage (Finger)
        0xa1,
        0x02, //   Collection (Logical)
        0x09,
        0x52, //     Usage (Input Mode)
        0x15,
        0x00, //     Logical Minimum (0)
        0x25,
        0x0a, //     Logical Maximum (10)
        0x75,
        0x08, //     Report Size (8)
        0x95,
        0x01, //     Report Count (1)
        0xb1,
        0x02, //     Feature (Data, Var, Abs)
        0xc0, //   End Collection
        0x09,
        0x22, //   Usage (Finger)
        0xa1,
        0x00, //   Collection (Physical)
        0x85,
        FUNCTION_SWITCH_ID, //     Report ID (7)
        0x09,
        0x57, //     Usage (Surface Switch)
        0x09,
        0x58, //     Usage (Button Switch)
        0x25,
        0x01, //     Logical Maximum (1)
        0x75,
        0x01, //     Report Size (1)
        0x95,
        0x02, //     Report Count (2)
        0xb1,
        0x02, //     Feature (Data, Var, Abs)
        0x95,
        0x06, //     Report Count (6)
        0xb1,
        0x03, //     Feature (Const)
        0xc0, //   End Collection
        0xc0, // End Collection
    ]);
    d
}

/// The logical (and physical) maximum of each axis, in 0.1 mm: the client's size, or
/// [`DEFAULT_SIZE_MM`] for one it did not report, at most 3 m.
pub(super) fn touchpad_extent(width_mm: u16, height_mm: u16) -> (u16, u16) {
    let (width_mm, height_mm) = if width_mm == 0 || height_mm == 0 {
        DEFAULT_SIZE_MM
    } else {
        (width_mm, height_mm)
    };
    (
        width_mm.clamp(10, 3000) * 10,
        height_mm.clamp(10, 3000) * 10,
    )
}
