//! The encoding backend of the host's [`Encoder`](crate::encoder::Encoder): a VA-API session
//! plus what feeds it. A captured picture is encoded where it is when it already has the
//! encoder's size; a CPU picture is uploaded into an RGB surface first, and a picture of
//! another size is scaled, both by video processing into a surface of the encoder's own format
//! (NV12, or P010 for a ten-bit stream, which video processing also makes of an 8-bit picture).
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
        })
    }

    fn convert(&mut self) -> Result<&mut Convert> {
        if self.convert.is_none() {
            let (width, height) = self.size;
            let format = self.session.format;
            self.convert = Some(Convert {
                converter: Converter::new(&self.va, width, height)?,
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
        // A picture is encoded where it is when it has the stream's size and bit depth.
        let as_is = picture == self.size && surface.pixel() == self.session.format.pixel();
        let (surface_id, held): (VASurfaceID, Option<Pooled>) = if as_is {
            (surface.id(), None)
        } else {
            let size = self.size;
            let convert = self.convert()?;
            let target = convert.target.take().context("no surface for conversion")?;
            convert
                .converter
                .convert(surface.id(), Source::Video, picture, target.id(), size)?;
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
