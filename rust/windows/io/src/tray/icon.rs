//! GDI icon badging for tray state overlays.
use butterpollo_session::tray::Icon;
use std::mem::size_of;
use windows::Win32::{
    Foundation::TRUE,
    Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, DIB_RGB_COLORS, DeleteObject,
        GetDC, GetDIBits, GetObjectW, ReleaseDC,
    },
    UI::WindowsAndMessaging::{CreateIconIndirect, GetIconInfo, HICON, ICONINFO},
};

/// `base` with the state's dot in its corner; None for Idle or an icon
/// without alpha, which then shows as is.
pub(super) unsafe fn badged(base: HICON, state: Icon) -> Option<HICON> {
    // SAFETY: The caller retains base on the tray thread. GetIconInfo returns owned
    // bitmaps; checked dimensions bound the 32-bit pixel and word-aligned mask buffers.
    // GDI copies their data synchronously, and each bitmap/DC is deleted/released once.
    unsafe {
        let mut info = ICONINFO::default();
        GetIconInfo(base, &mut info).ok()?;
        let made = (|| {
            if info.hbmColor.is_invalid() {
                return None;
            }
            let mut bitmap = BITMAP::default();
            if GetObjectW(
                info.hbmColor.into(),
                size_of::<BITMAP>() as i32,
                Some((&mut bitmap as *mut BITMAP).cast()),
            ) == 0
            {
                return None;
            }
            let (width, height) = (bitmap.bmWidth, bitmap.bmHeight);
            if width <= 0 || height <= 0 || width > 256 || height > 256 {
                return None;
            }
            let mut header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Top-down rows, as CreateBitmap takes them.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; width as usize * height as usize * 4];
            let screen = GetDC(None);
            let lines = GetDIBits(
                screen,
                info.hbmColor,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut header,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, screen);
            if lines != height
                || !butterpollo_session::tray::badge(
                    &mut pixels,
                    width as usize,
                    height as usize,
                    state,
                )
            {
                return None;
            }
            let color = CreateBitmap(width, height, 1, 32, Some(pixels.as_ptr().cast()));
            // An all-zero mask: the colour image's alpha decides.
            let mask_bytes = vec![0u8; (width as usize).div_ceil(16) * 2 * height as usize];
            let mask = CreateBitmap(width, height, 1, 1, Some(mask_bytes.as_ptr().cast()));
            let icon = (!color.is_invalid() && !mask.is_invalid())
                .then(|| {
                    CreateIconIndirect(&ICONINFO {
                        fIcon: TRUE,
                        xHotspot: 0,
                        yHotspot: 0,
                        hbmMask: mask,
                        hbmColor: color,
                    })
                    .ok()
                })
                .flatten();
            let _ = DeleteObject(color.into());
            let _ = DeleteObject(mask.into());
            icon
        })();
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
        made
    }
}
