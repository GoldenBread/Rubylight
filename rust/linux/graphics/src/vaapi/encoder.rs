//! The encoding backend of the host's [`Encoder`](crate::encoder::Encoder): a VA-API session
//! plus what feeds it. A captured picture is encoded where it is when it already has the
//! encoder's size; a CPU picture is uploaded into an RGB surface first, and a picture of
//! another size is scaled, both by video processing into a surface of the encoder's own format
//! (NV12, or P010 for a ten-bit stream, which video processing also makes of an 8-bit picture).
//! A packed 4:4:4 stream ([`crate::packed444`]) encodes the packed picture of an RGB one
//! instead, scaled first by video processing when it is not the stream's size.
use super::{
    Va,
    ffi::VASurfaceID,
    pool::{Pool, Pooled},
    session::{Codec, Format, Session},
    vpp::{Converter, Source},
};
use anyhow::{Context, Result};
use std::sync::Arc;

pub struct Backend {
    va: Arc<Va>,
    session: Session,
    /// The size the stream codes.
    size: (u32, u32),
    /// Made when a picture needs converting or scaling.
    convert: Option<Convert>,
    /// Encoded bytes, before they are copied into a buffer of the frame pool.
    scratch: Vec<u8>,
    /// Set for a packed 4:4:4 stream (boxed: the Vulkan device's function tables are large).
    packed: Option<Box<Packed>>,
}

/// What a packed 4:4:4 stream needs besides its session.
struct Packed {
    packer: crate::packed444::Packer,
    /// The stream's picture size; the session codes [`crate::packed444::Packer::coded_size`].
    picture: (u32, u32),
    /// Linear NV12 surfaces of the coded size, which the pass writes and the session reads.
    target: Arc<Pool>,
    /// Linear BGRX surfaces of the stream's size: a picture of another size scaled, or a
    /// surface that is not linear copied, for the pass to read. Made when first needed.
    rgb: Option<Arc<Pool>>,
    /// Linear BGRX surfaces a CPU picture is uploaded into, of that picture's size.
    upload: Option<Arc<Pool>>,
    converter: Option<Converter>,
}

/// What converting a CPU picture or scaling a captured one needs.
struct Convert {
    converter: Converter,
    /// Surfaces of the encoder's size and format, which the session reads.
    target: Arc<Pool>,
    /// BGRX surfaces a CPU picture is uploaded into, made with the first CPU picture.
    rgb: Option<Arc<Pool>>,
}

/// One encoded picture's bytes.
pub struct Output {
    pub bytes: Vec<u8>,
    pub idr: bool,
    /// The picture is the first after a loss report and refers to a picture from before it.
    pub recovered: bool,
}

impl Backend {
    pub fn new(
        va: &Arc<Va>,
        codec: Codec,
        format: Format,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self> {
        let session = Session::new(va, codec, format, width, height, fps)?;
        Ok(Self {
            va: va.clone(),
            session,
            size: (width, height),
            convert: None,
            scratch: Vec::new(),
            packed: None,
        })
    }

    /// A packed 4:4:4 stream of `width`x`height` pictures, whose YCbCr has `color_matrix`
    /// (0 BT.601, 1 BT.709, 2 BT.2020) in full or limited range. The session codes 8-bit
    /// pictures of twice the padded height.
    pub fn new_packed(
        va: &Arc<Va>,
        codec: Codec,
        (width, height): (u32, u32),
        fps: u32,
        (color_matrix, full_range): (u8, bool),
    ) -> Result<Self> {
        let packer = crate::packed444::Packer::new(va, (width, height), color_matrix, full_range)?;
        let (coded_width, coded_height) = packer.coded_size();
        let mut backend = Self::new(va, codec, Format::SDR, coded_width, coded_height, fps)?;
        backend.packed = Some(Box::new(Packed {
            packer,
            picture: (width, height),
            target: Pool::new_linear(
                va,
                super::ffi::VA_RT_FORMAT_YUV420,
                super::NV12,
                coded_width,
                coded_height,
                2,
                2,
            )?,
            rgb: None,
            upload: None,
            converter: None,
        }));
        Ok(backend)
    }

    fn convert(&mut self) -> Result<&mut Convert> {
        if self.convert.is_none() {
            let (width, height) = self.size;
            let format = self.session.format;
            self.convert = Some(Convert {
                converter: Converter::new(&self.va, width, height)?.writing(format.colors),
                target: Pool::new(
                    &self.va,
                    format.rt_format(),
                    format.pixel(),
                    width,
                    height,
                    2,
                    2,
                )?,
                rgb: None,
            });
        }
        Ok(self.convert.as_mut().expect("made above"))
    }

    /// Encodes the picture on `surface` (`picture` pixels of it are the picture), scaling it
    /// first when it is not the stream's size.
    pub fn encode_surface(
        &mut self,
        surface: &Pooled,
        picture: (u32, u32),
        idr: bool,
        bits_per_second: u32,
    ) -> Result<Output> {
        if self.packed.is_some() {
            return self.encode_packed(surface, picture, idr, bits_per_second);
        }
        // A picture is encoded where it is when it has the stream's size and bit depth.
        let format = self.session.format;
        // A YCbCr capture wrote the picture in the stream's colours (its settings say which).
        let source = if matches!(surface.pixel(), super::NV12 | super::P010) {
            Source::Video(format.colors)
        } else {
            Source::Rgb
        };
        let as_is = picture == self.size && surface.pixel() == format.pixel();
        let (surface_id, held): (VASurfaceID, Option<Pooled>) = if as_is {
            (surface.id(), None)
        } else {
            let size = self.size;
            let convert = self.convert()?;
            let target = convert.target.take().context("no surface for conversion")?;
            convert
                .converter
                .convert(surface.id(), source, picture, target.id(), size)?;
            (target.id(), Some(target))
        };
        let output = self.run(surface_id, idr, bits_per_second);
        drop(held);
        output
    }

    /// Encodes a BGRA picture in system memory.
    pub fn encode_bgra(
        &mut self,
        image: (u32, u32, usize, &[u8]),
        idr: bool,
        bits_per_second: u32,
    ) -> Result<Output> {
        let (width, height, stride, bytes) = image;
        let va = self.va.clone();
        if let Some(packed) = &mut self.packed {
            let upload = match &packed.upload {
                Some(pool)
                    if pool.width == super::pool::aligned(width)
                        && pool.height == super::pool::aligned(height) =>
                {
                    pool.clone()
                }
                _ => {
                    let pool = Pool::new_linear(
                        &va,
                        super::ffi::VA_RT_FORMAT_RGB32,
                        super::BGRX,
                        width,
                        height,
                        1,
                        2,
                    )?;
                    packed.upload = Some(pool.clone());
                    pool
                }
            };
            let source = upload.take().context("no surface for the upload")?;
            va.upload_bgra(source.id(), width, height, stride, bytes)?;
            va.sync(source.id())?;
            return self.encode_packed(&source, (width, height), idr, bits_per_second);
        }
        let size = self.size;
        let convert = self.convert()?;
        let rgb = match &convert.rgb {
            Some(pool)
                if pool.width == super::pool::aligned(width)
                    && pool.height == super::pool::aligned(height) =>
            {
                pool.clone()
            }
            _ => {
                let pool = Pool::new(
                    &va,
                    super::ffi::VA_RT_FORMAT_RGB32,
                    super::BGRX,
                    width,
                    height,
                    1,
                    2,
                )?;
                convert.rgb = Some(pool.clone());
                pool
            }
        };
        let source = rgb.take().context("no surface for the upload")?;
        va.upload_bgra(source.id(), width, height, stride, bytes)?;
        let target = convert.target.take().context("no surface for conversion")?;
        convert
            .converter
            .convert(source.id(), Source::Rgb, (width, height), target.id(), size)?;
        let output = self.run(target.id(), idr, bits_per_second);
        drop((source, target));
        output
    }

    /// Packs `surface` (an RGB picture of `picture` pixels) and encodes the packed picture.
    fn encode_packed(
        &mut self,
        surface: &Pooled,
        picture: (u32, u32),
        idr: bool,
        bits_per_second: u32,
    ) -> Result<Output> {
        let va = self.va.clone();
        let packed = self.packed.as_mut().context("not a packed stream")?;
        anyhow::ensure!(
            surface.pixel() != super::NV12 && surface.pixel() != super::P010,
            "packed 4:4:4 needs the captured picture in RGB; this capture delivers YCbCr 4:2:0"
        );
        // The pass reads a linear picture of the stream's size; video processing makes one of
        // anything else.
        let copied = if picture == packed.picture && surface.linear() {
            None
        } else {
            let (width, height) = packed.picture;
            if packed.converter.is_none() {
                packed.converter = Some(Converter::new(&va, width, height)?);
            }
            let rgb = match &packed.rgb {
                Some(pool) => pool.clone(),
                None => {
                    let pool = Pool::new_linear(
                        &va,
                        super::ffi::VA_RT_FORMAT_RGB32,
                        super::BGRX,
                        width,
                        height,
                        1,
                        2,
                    )?;
                    packed.rgb = Some(pool.clone());
                    pool
                }
            };
            let copy = rgb.take().context("no surface for the scaled picture")?;
            packed.converter.as_ref().expect("made above").scale_rgb(
                surface.id(),
                picture,
                copy.id(),
                packed.picture,
            )?;
            va.sync(copy.id())?;
            Some(copy)
        };
        let target = packed
            .target
            .take()
            .context("no surface for the packed picture")?;
        packed
            .packer
            .pack(copied.as_ref().unwrap_or(surface), &target)?;
        drop(copied);
        let output = self.run(target.id(), idr, bits_per_second);
        drop(target);
        output
    }

    fn run(&mut self, input: VASurfaceID, idr: bool, bits_per_second: u32) -> Result<Output> {
        self.va.sync(input)?;
        let keyframe = self
            .session
            .encode(input, idr, bits_per_second, &mut self.scratch)?;
        Ok(Output {
            bytes: crate::frame_pool::copy(&self.scratch),
            idr: keyframe,
            recovered: self.session.recovered,
        })
    }

    /// Whether pictures can be told to refer to older ones after a loss.
    pub fn supports_invalidation(&self) -> bool {
        self.session.supports_invalidation()
    }

    /// Pictures `first` to `last` were lost; false when only a keyframe repairs that.
    pub fn invalidate(&mut self, first: u64, last: u64) -> bool {
        self.session.invalidate(first, last)
    }

    /// The wire number the next picture gets.
    pub fn set_next_frame(&mut self, wire: u64) {
        self.session.set_next_frame(wire);
    }

    pub fn set_hdr_metadata(&mut self, metadata: butterpollo_media_policy::hdr::Metadata) {
        self.session.set_hdr_metadata(metadata);
    }
}
