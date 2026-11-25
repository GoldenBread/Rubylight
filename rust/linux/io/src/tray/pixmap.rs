//! The tray icon's pictures: the PNG images inside the host's `.ico`, each with the state's
//! dot (`butterpollo_session::tray::badge`, as on Windows), in the form a StatusNotifierItem
//! takes them.
use butterpollo_session::{
    clipboard_image::{Image, decode_png},
    tray::{Icon, badge},
};

/// One size of a picture: width, height and ARGB32 pixels in network byte order.
pub(super) type Pixmap = (i32, i32, Vec<u8>);
/// The sizes of the picture while idle, streaming and paused.
pub(super) type Pixmaps = [Vec<Pixmap>; 3];

/// Larger images cost every read of the icon over the bus, and no tray shows them.
const LARGEST: u32 = 128;

/// The PNG images of an `.ico` file up to [`LARGEST`], smallest first. An image stored as a
/// bitmap, and anything unreadable, is left out.
pub(super) fn ico_images(ico: &[u8]) -> Vec<Image> {
    let bytes = |at: usize, len: usize| ico.get(at..at.checked_add(len)?);
    let u16_at = |at| bytes(at, 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at| bytes(at, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    // An icon file: reserved 0, type 1.
    if u16_at(0) != Some(0) || u16_at(2) != Some(1) {
        return Vec::new();
    }
    let count = usize::from(u16_at(4).unwrap_or(0));
    let mut images: Vec<Image> = (0..count)
        .filter_map(|entry| {
            let at = 6 + entry * 16;
            let (size, offset) = (u32_at(at + 8)?, u32_at(at + 12)?);
            let png = bytes(offset as usize, size as usize)?;
            decode_png(png)
                .inspect_err(|error| tracing::debug!(?error, entry, "an icon image left out"))
                .ok()
        })
        .filter(|image| image.width <= LARGEST && image.height <= LARGEST)
        .collect();
    images.sort_by_key(|image| image.width);
    images
}

/// `image` as the icon shows it in state `icon`.
pub(super) fn pixmap(image: &Image, icon: Icon) -> Pixmap {
    let (width, height) = (image.width as usize, image.height as usize);
    // The badge draws on BGRA.
    let mut pixels = image.rgba.clone();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    badge(&mut pixels, width, height, icon);
    let argb = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[b, g, r, a]| [a, r, g, b])
        .collect();
    (image.width as i32, image.height as i32, argb)
}

/// Every size of the picture, in each state.
pub(super) fn pixmaps(images: &[Image]) -> Pixmaps {
    [Icon::Idle, Icon::Streaming, Icon::Paused]
        .map(|icon| images.iter().map(|image| pixmap(image, icon)).collect())
}
