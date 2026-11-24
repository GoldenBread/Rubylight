//! The encoding backend of the host's [`Encoder`](crate::encoder::Encoder): a VA-API session
//! plus what feeds it. A captured picture is encoded where it is when it already has the
//! encoder's size; a CPU picture is uploaded into an RGB surface first, and a picture of
//! another size is scaled, both by video processing into an NV12 surface of the encoder's own.
use super::{
    Va,
    ffi::VASurfaceID,
    pool::{Pool, Pooled},
    session::{Codec, Session},
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
    /// NV12 surfaces of the encoder's size, which the session reads.
    nv12: Arc<Pool>,
    /// BGRX surfaces a CPU picture is uploaded into, made with the first CPU picture.
    rgb: Option<Arc<Pool>>,
}

/// One encoded picture's bytes.
pub struct Output {
    pub bytes: Vec<u8>,
    pub idr: bool,
}

impl Backend {
    pub fn new(va: &Arc<Va>, codec: Codec, width: u32, height: u32, fps: u32) -> Result<Self> {
        let session = Session::new(va, codec, width, height, fps)?;
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
            self.convert = Some(Convert {
                converter: Converter::new(&self.va, width, height)?,
                nv12: Pool::new(
                    &self.va,
                    super::ffi::VA_RT_FORMAT_YUV420,
                    super::NV12,
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
        let (surface_id, held): (VASurfaceID, Option<Pooled>) = if picture == self.size {
            (surface.id(), None)
        } else {
            let size = self.size;
            let convert = self.convert()?;
            let target = convert.nv12.take().context("no surface for scaling")?;
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
        let target = convert.nv12.take().context("no surface for conversion")?;
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
        })
    }
}
