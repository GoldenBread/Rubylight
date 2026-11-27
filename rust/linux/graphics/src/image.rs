//! Cover images, written as PNG for Moonlight. On Linux the size of a PNG, JPEG, WebP or BMP is read from its header, a PNG is taken as it is, and a JPEG (Steam's covers), a WebP (its first frame when animated) and an uncompressed BMP are rewritten as PNG; decoding any other format is not available yet.

use anyhow::{Context, Result, bail, ensure};
use image_webp::WebPDecoder;
use std::path::Path;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
/// No cover is larger; a header that claims more is refused before anything is allocated.
const LARGEST_SIDE: u32 = 16_384;

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("decoding {}", path.display()))
}
fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}
fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}
fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// The size in a PNG's IHDR, which is its first chunk.
fn png_size(bytes: &[u8]) -> Result<(u32, u32)> {
    ensure!(
        bytes.get(12..16) == Some(b"IHDR"),
        "the PNG does not start with its header"
    );
    let size = be32(bytes, 16).zip(be32(bytes, 20));
    size.filter(|&(width, height)| width > 0 && height > 0)
        .context("the PNG header has no size")
}
/// Walks a PNG's chunks to its end, checking each one's checksum: a download that was cut
/// short or damaged is refused. The pixels themselves are not decoded.
fn check_png(bytes: &[u8]) -> Result<()> {
    let mut at = PNG_SIGNATURE.len();
    loop {
        let length = be32(bytes, at).context("the PNG ends before its last chunk")? as usize;
        let end = at
            .checked_add(length)
            .and_then(|end| end.checked_add(12))
            .filter(|end| *end <= bytes.len())
            .context("the PNG ends inside a chunk")?;
        let kind = &bytes[at + 4..at + 8];
        ensure!(
            be32(bytes, end - 4) == Some(crc32(&bytes[at + 4..end - 4])),
            "a PNG chunk is damaged"
        );
        if kind == b"IEND" {
            return Ok(());
        }
        at = end;
    }
}
/// The size in a JPEG's frame header.
fn jpeg_size(bytes: &[u8]) -> Result<(u32, u32)> {
    let mut at = 2;
    loop {
        ensure!(bytes.get(at) == Some(&0xFF), "the JPEG has no frame header");
        let marker = *bytes.get(at + 1).context("the JPEG ends early")?;
        match marker {
            // Fill bytes before a marker.
            0xFF => at += 1,
            // Markers without a segment.
            0x01 | 0xD0..=0xD8 => at += 2,
            // The picture's data or end comes before any frame header.
            0xD9 | 0xDA => bail!("the JPEG has no frame header"),
            // A frame header: every SOF but the Huffman tables, JPG and arithmetic conditioning.
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                let side = |at: usize| {
                    bytes
                        .get(at..at + 2)
                        .map(|side| u32::from(u16::from_be_bytes([side[0], side[1]])))
                };
                let (height, width) = side(at + 5)
                    .zip(side(at + 7))
                    .context("the JPEG ends inside its frame header")?;
                ensure!(width > 0 && height > 0, "the JPEG header has no size");
                return Ok((width, height));
            }
            _ => {
                let length = bytes
                    .get(at + 2..at + 4)
                    .map(|length| usize::from(u16::from_be_bytes([length[0], length[1]])))
                    .filter(|length| *length >= 2)
                    .context("the JPEG ends inside a segment")?;
                at += 2 + length;
            }
        }
    }
}

/// A JPEG decoded with `zune-jpeg` and written as an 8-bit RGB or grey PNG.
fn jpeg_to_png(bytes: &[u8]) -> Result<((u32, u32), Vec<u8>)> {
    let (width, height) = jpeg_size(bytes)?;
    ensure!(
        width <= LARGEST_SIDE && height <= LARGEST_SIDE,
        "the JPEG is {width}x{height}, not a picture's size"
    );
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(LARGEST_SIDE as usize)
        .set_max_height(LARGEST_SIDE as usize);
    let mut decoder = JpegDecoder::new_with_options(std::io::Cursor::new(bytes), options);
    let pixels = decoder.decode()?;
    let (width, height) = decoder
        .dimensions()
        .context("the JPEG has no frame header")?;
    let color = match decoder.output_colorspace() {
        Some(ColorSpace::RGB) => png::ColorType::Rgb,
        Some(ColorSpace::Luma) => png::ColorType::Grayscale,
        other => bail!("the JPEG decodes to {other:?}, not RGB or grey"),
    };
    let (width, height) = (width as u32, height as u32);
    Ok(((width, height), write_png(width, height, color, &pixels)?))
}

/// A WebP decoded with `image-webp` (its first frame when animated) and written as an 8-bit
/// RGB PNG, or RGBA when it has transparency.
fn webp_to_png(bytes: &[u8]) -> Result<((u32, u32), Vec<u8>)> {
    let mut decoder = WebPDecoder::new(std::io::Cursor::new(bytes))?;
    let (width, height) = decoder.dimensions();
    ensure!(
        width > 0 && height > 0 && width <= LARGEST_SIDE && height <= LARGEST_SIDE,
        "the WebP is {width}x{height}, not a picture's size"
    );
    let size = decoder
        .output_buffer_size()
        .context("the WebP is too large to decode")?;
    decoder.set_memory_limit(size.saturating_add(1 << 20));
    let mut pixels = vec![0; size];
    decoder.read_image(&mut pixels)?;
    let color = if decoder.has_alpha() {
        png::ColorType::Rgba
    } else {
        png::ColorType::Rgb
    };
    Ok(((width, height), write_png(width, height, color, &pixels)?))
}

/// 8-bit pixels of `color` as a PNG.
fn write_png(width: u32, height: u32, color: png::ColorType, pixels: &[u8]) -> Result<Vec<u8>> {
    let mut png = Vec::with_capacity(pixels.len() / 2);
    let mut encoder = png::Encoder::new(&mut png, width, height);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(pixels)?;
    writer.finish()?;
    Ok(png)
}

/// An uncompressed bitmap's layout: where its rows are and how to read them.
struct Bitmap {
    width: u32,
    height: u32,
    /// Bytes per pixel: blue, green, red, and with four an unused byte.
    pixel: usize,
    stride: usize,
    offset: usize,
    top_down: bool,
}
fn bitmap(bytes: &[u8]) -> Result<Bitmap> {
    let field = |at| le32(bytes, at).context("the bitmap ends inside its header");
    ensure!(
        field(14)? >= 40,
        "the bitmap has a header older than Windows 3"
    );
    let (width, rows) = (field(18)? as i32, field(22)? as i32);
    let height = rows.unsigned_abs();
    ensure!(
        width > 0 && height > 0 && width as u32 <= LARGEST_SIDE && height <= LARGEST_SIDE,
        "the bitmap is {width}x{rows}, not a picture's size"
    );
    let bits = le16(bytes, 28).context("the bitmap ends inside its header")?;
    let compression = field(30)?;
    ensure!(
        compression == 0 && matches!(bits, 24 | 32),
        "only uncompressed 24 and 32-bit bitmaps are decoded (this one: {bits}-bit, compression {compression})"
    );
    let pixel = usize::from(bits / 8);
    let stride = (width as usize * pixel).next_multiple_of(4);
    let offset = field(10)? as usize;
    // The last row may come without its padding.
    let needed = offset + stride * (height as usize - 1) + width as usize * pixel;
    ensure!(bytes.len() >= needed, "the bitmap ends before its last row");
    Ok(Bitmap {
        width: width as u32,
        height,
        pixel,
        stride,
        offset,
        top_down: rows < 0,
    })
}

enum Format {
    Png,
    Jpeg,
    WebP,
    Bmp,
    Other,
}
fn format(bytes: &[u8]) -> Format {
    if bytes.starts_with(PNG_SIGNATURE) {
        Format::Png
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        Format::Jpeg
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Format::WebP
    } else if bytes.starts_with(b"BM") {
        Format::Bmp
    } else {
        Format::Other
    }
}

/// An image's width and height, read from the header of a PNG, JPEG, WebP or BMP file.
pub fn dimensions(path: &Path) -> Result<(u32, u32)> {
    let bytes = read(path)?;
    match format(&bytes) {
        Format::Png => png_size(&bytes),
        Format::Jpeg => jpeg_size(&bytes),
        Format::WebP => WebPDecoder::new(std::io::Cursor::new(&bytes[..]))
            .map(|decoder| decoder.dimensions())
            .map_err(Into::into),
        Format::Bmp => bitmap(&bytes).map(|bitmap| (bitmap.width, bitmap.height)),
        Format::Other => Err(crate::unsupported("Reading this image format")),
    }
    .with_context(|| format!("decoding {}", path.display()))
}

/// Write `source` to `destination` as a PNG, replacing it only once the new file is
/// complete. A PNG is copied once its chunks check out, and a JPEG, a WebP or an uncompressed
/// BMP is encoded; any other format is an error, since Linux has no decoder for it yet.
pub fn to_png(source: &Path, destination: &Path) -> Result<(u32, u32)> {
    let bytes = read(source)?;
    let (size, png) = (|| match format(&bytes) {
        Format::Png => {
            let size = png_size(&bytes)?;
            check_png(&bytes)?;
            Ok((size, None))
        }
        Format::Bmp => {
            let bitmap = bitmap(&bytes)?;
            Ok(((bitmap.width, bitmap.height), Some(encode(&bitmap, &bytes))))
        }
        Format::Jpeg => jpeg_to_png(&bytes).map(|(size, png)| (size, Some(png))),
        Format::WebP => webp_to_png(&bytes).map(|(size, png)| (size, Some(png))),
        Format::Other => Err(crate::unsupported("Converting this image format to PNG")),
    })()
    .with_context(|| format!("decoding {}", source.display()))?;
    let mut partial = destination.as_os_str().to_owned();
    partial.push(".partial");
    let partial = std::path::PathBuf::from(partial);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let written = std::fs::write(&partial, png.as_deref().unwrap_or(&bytes))
        .and_then(|()| std::fs::rename(&partial, destination));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&partial);
        return Err(error).with_context(|| format!("writing {}", destination.display()));
    }
    Ok(size)
}

/// The bitmap as an 8-bit RGB PNG. Its pixel data is stored, not compressed.
fn encode(bitmap: &Bitmap, bytes: &[u8]) -> Vec<u8> {
    let (width, height) = (bitmap.width as usize, bitmap.height as usize);
    // Each row is a filter byte (0: none) and its pixels.
    let mut rows = Vec::with_capacity(height * (1 + width * 3));
    for row in 0..height {
        let line = if bitmap.top_down {
            row
        } else {
            height - 1 - row
        };
        let start = bitmap.offset + line * bitmap.stride;
        rows.push(0);
        for pixel in bytes[start..start + width * bitmap.pixel].chunks_exact(bitmap.pixel) {
            rows.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
    }
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&bitmap.width.to_be_bytes());
    header.extend_from_slice(&bitmap.height.to_be_bytes());
    // Bit depth 8, colour type 2 (RGB), deflate, adaptive filtering, not interlaced.
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut png = Vec::with_capacity(rows.len() + rows.len() / 65_535 * 5 + 128);
    png.extend_from_slice(PNG_SIGNATURE);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &stored(&rows));
    chunk(&mut png, b"IEND", &[]);
    png
}
fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let checksum = crc32(&png[start..]);
    png.extend_from_slice(&checksum.to_be_bytes());
}
/// `data` as a zlib stream of stored deflate blocks.
fn stored(data: &[u8]) -> Vec<u8> {
    let mut stream = Vec::with_capacity(data.len() + data.len() / 65_535 * 5 + 16);
    stream.extend_from_slice(&[0x78, 0x01]);
    let mut blocks = data.chunks(65_535).peekable();
    if blocks.peek().is_none() {
        stream.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(block) = blocks.next() {
        let length = block.len() as u16;
        stream.push(u8::from(blocks.peek().is_none()));
        stream.extend_from_slice(&length.to_le_bytes());
        stream.extend_from_slice(&(!length).to_le_bytes());
        stream.extend_from_slice(block);
    }
    stream.extend_from_slice(&adler32(data).to_be_bytes());
    stream
}
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 bytes is the most that cannot overflow the sums before the modulo.
    for part in data.chunks(5552) {
        for byte in part {
            a += u32::from(*byte);
            b += a;
        }
        a %= 65_521;
        b %= 65_521;
    }
    b << 16 | a
}
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                0xEDB8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
};
fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |crc, byte| {
        CRC_TABLE[usize::from(crc as u8 ^ byte)] ^ (crc >> 8)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An uncompressed bottom-up 24-bit bitmap whose rows need padding, each pixel coloured
    /// by its place.
    fn bmp(width: u32, height: u32) -> Vec<u8> {
        let stride = (width as usize * 3).next_multiple_of(4);
        let mut bmp = vec![0u8; 54 + stride * height as usize];
        let size = bmp.len() as u32;
        bmp[..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&size.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&width.to_le_bytes());
        bmp[22..26].copy_from_slice(&height.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        for y in 0..height as usize {
            for x in 0..width as usize {
                let at = 54 + (height as usize - 1 - y) * stride + x * 3;
                bmp[at..at + 3].copy_from_slice(&[x as u8, y as u8, 200]);
            }
        }
        bmp
    }
    fn folder(name: &str) -> std::path::PathBuf {
        let folder =
            std::env::temp_dir().join(format!("butterpollo-image-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn checksums_match_their_reference_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(&[]), 1);
    }

    #[test]
    fn a_bitmap_becomes_a_png_of_the_same_pixels() {
        let folder = folder("bmp");
        let (source, png) = (folder.join("cover.bmp"), folder.join("covers/cover.png"));
        std::fs::write(&source, bmp(5, 3)).unwrap();
        assert_eq!(dimensions(&source).unwrap(), (5, 3));
        assert_eq!(to_png(&source, &png).unwrap(), (5, 3));
        assert_eq!(dimensions(&png).unwrap(), (5, 3));
        assert!(!folder.join("covers/cover.png.partial").exists());
        let bytes = std::fs::read(&png).unwrap();
        check_png(&bytes).unwrap();
        // One stored block: the zlib header, the block header, then the rows as they are.
        let data = &bytes[8 + 25 + 8..];
        assert_eq!(&data[..3], [0x78, 0x01, 1]);
        let rows = &data[7..7 + 3 * 16];
        for y in 0..3 {
            assert_eq!(rows[y * 16], 0);
            for x in 0..5 {
                let at = y * 16 + 1 + x * 3;
                assert_eq!(rows[at..at + 3], [200, y as u8, x as u8]);
            }
        }
        // A PNG source is copied as it is.
        let copy = folder.join("copy.png");
        assert_eq!(to_png(&png, &copy).unwrap(), (5, 3));
        assert_eq!(std::fs::read(&copy).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_large_bitmap_is_stored_in_several_blocks() {
        let folder = folder("large");
        let (source, png) = (folder.join("cover.bmp"), folder.join("cover.png"));
        std::fs::write(&source, bmp(600, 900)).unwrap();
        assert_eq!(to_png(&source, &png).unwrap(), (600, 900));
        check_png(&std::fs::read(&png).unwrap()).unwrap();
        assert_eq!(dimensions(&png).unwrap(), (600, 900));
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// The pixel at `x`, `y` of an 8-bit PNG, and its channel count.
    fn pixel(png: &Path, x: usize, y: usize) -> Vec<u8> {
        let decoder =
            ::png::Decoder::new(std::io::BufReader::new(std::fs::File::open(png).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut buffer).unwrap();
        let channels = frame.color_type.samples();
        let at = y * frame.line_size + x * channels;
        buffer[at..at + channels].to_vec()
    }

    #[test]
    fn a_jpeg_cover_becomes_a_png_of_the_same_picture() {
        let folder = folder("jpeg");
        let (source, png) = (folder.join("cover.jpg"), folder.join("covers/cover.png"));
        // 60x90, red above and blue below, as Steam's covers are JPEG.
        std::fs::write(&source, include_bytes!("image_fixtures/cover.jpg")).unwrap();
        assert_eq!(to_png(&source, &png).unwrap(), (60, 90));
        check_png(&std::fs::read(&png).unwrap()).unwrap();
        assert_eq!(dimensions(&png).unwrap(), (60, 90));
        assert!(!folder.join("covers/cover.png.partial").exists());
        let near = |got: Vec<u8>, want: [u8; 3]| {
            got.len() == 3 && got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 8)
        };
        assert!(near(pixel(&png, 30, 10), [220, 30, 30]));
        assert!(near(pixel(&png, 30, 80), [30, 30, 220]));
        // A grey JPEG comes out grey.
        let (grey, grey_png) = (folder.join("grey.jpg"), folder.join("grey.png"));
        std::fs::write(&grey, include_bytes!("image_fixtures/grey.jpg")).unwrap();
        assert_eq!(to_png(&grey, &grey_png).unwrap(), (30, 20));
        let value = pixel(&grey_png, 15, 10);
        assert!(near(value.clone(), [128; 3]), "{value:?}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_webp_cover_becomes_a_png_of_the_same_picture() {
        let folder = folder("webp");
        let (source, png) = (folder.join("cover.webp"), folder.join("covers/cover.png"));
        // Lossy, 60x90, red above and blue below.
        std::fs::write(&source, include_bytes!("image_fixtures/cover.webp")).unwrap();
        assert_eq!(dimensions(&source).unwrap(), (60, 90));
        assert_eq!(to_png(&source, &png).unwrap(), (60, 90));
        check_png(&std::fs::read(&png).unwrap()).unwrap();
        let near = |got: Vec<u8>, want: &[u8]| {
            got.len() == want.len() && got.iter().zip(want).all(|(g, w)| g.abs_diff(*w) <= 10)
        };
        assert!(near(pixel(&png, 30, 10), &[220, 30, 30]));
        assert!(near(pixel(&png, 30, 80), &[30, 30, 220]));
        // Lossless with transparency: green above, clear below, kept as RGBA.
        let (alpha, alpha_png) = (folder.join("alpha.webp"), folder.join("alpha.png"));
        std::fs::write(&alpha, include_bytes!("image_fixtures/alpha.webp")).unwrap();
        assert_eq!(to_png(&alpha, &alpha_png).unwrap(), (20, 30));
        assert_eq!(pixel(&alpha_png, 5, 5), vec![10, 200, 10, 255]);
        assert_eq!(pixel(&alpha_png, 5, 25)[3], 0);
        // A WebP cut short is refused and leaves nothing.
        let mut cut = include_bytes!("image_fixtures/cover.webp").to_vec();
        cut.truncate(cut.len() / 2);
        let (broken, out) = (folder.join("broken.webp"), folder.join("broken.png"));
        std::fs::write(&broken, cut).unwrap();
        assert!(to_png(&broken, &out).is_err());
        assert!(!out.exists());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_damaged_png_and_an_undecodable_format_are_refused_and_leave_no_file() {
        let folder = folder("refused");
        let (source, png) = (folder.join("cover.bmp"), folder.join("cover.png"));
        std::fs::write(&source, bmp(4, 4)).unwrap();
        to_png(&source, &png).unwrap();
        let mut cut = std::fs::read(&png).unwrap();
        cut.truncate(cut.len() - 20);
        let damaged = folder.join("damaged.png");
        std::fs::write(&damaged, cut).unwrap();
        let destination = folder.join("out.png");
        assert!(to_png(&damaged, &destination).is_err());
        // A JPEG's size is read from its header, but one with no picture after it is refused.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC2, 0, 11, 8];
        jpeg.extend_from_slice(&[0x03, 0x84, 0x02, 0x58, 1, 0, 0, 0]);
        let photo = folder.join("photo.jpg");
        std::fs::write(&photo, jpeg).unwrap();
        assert_eq!(dimensions(&photo).unwrap(), (600, 900));
        assert!(to_png(&photo, &destination).is_err());
        let text = folder.join("notes.txt");
        std::fs::write(&text, b"not a picture").unwrap();
        assert!(dimensions(&text).is_err() && to_png(&text, &destination).is_err());
        assert!(dimensions(&folder.join("missing.png")).is_err());
        assert!(!destination.exists());
        let _ = std::fs::remove_dir_all(&folder);
    }
}
