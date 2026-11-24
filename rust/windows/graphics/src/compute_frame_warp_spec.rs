//! Frame Warp compute shader constants, HUD buffer layout and root signature.

use super::super::*;
use crate::capture::Pixel;

/// The largest thumbnail: `butterpollo_media_policy::frame_warp::THUMBNAIL_WIDTH`
/// by the tallest `thumbnail_height`.
pub const THUMBNAIL_EDGE: u32 = 256;
pub(super) const THUMBNAIL_BYTES: u64 = THUMBNAIL_EDGE as u64 * THUMBNAIL_EDGE as u64;
/// HUD cells across or down at most
/// (`rubylight_protocol::frame_warp::MAX_MASK_EDGE`).
pub const MAX_GRID_EDGE: u32 = 2048;
/// The score from which a cell is HUD: it stayed through about 24
/// comparisons while the camera turned, and every change halves it.
pub const HUD_SCORE: u32 = 24;
/// `cbuffer FrameWarp` (b4).
pub(super) const CONSTANTS: u32 = 16;

/// What to run over one capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pass {
    /// The thumbnail's width, a multiple of 4, and height, each at most
    /// [`THUMBNAIL_EDGE`].
    pub thumbnail: (u32, u32),
    /// The HUD work while the camera turns; `None` while it stands still.
    pub hud: Option<Hud>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hud {
    /// The mask cells across and down ([`hud_grid`]).
    pub grid: (u32, u32),
    /// Copy the mask bits back for [`FrameWarpGpu::poll`]; ignored while one
    /// copy is still being read.
    pub read: bool,
    /// How far apart, in the `at` of [`FrameWarpGpu::submit`], the captures
    /// the pass compares are at least: the captures in between run none, so
    /// that a slow turn at a high frame rate still moves the scene far enough
    /// between the two for its pixels to change. 0 compares each capture with
    /// the one before.
    pub every_us: u64,
}
impl Pass {
    pub(super) fn check(&self) -> Result<()> {
        let (width, height) = self.thumbnail;
        anyhow::ensure!(
            width % 4 == 0
                && (4..=THUMBNAIL_EDGE).contains(&width)
                && (1..=THUMBNAIL_EDGE).contains(&height),
            "Frame Warp thumbnail of {width}x{height}"
        );
        if let Some(hud) = &self.hud {
            anyhow::ensure!(
                (1..=MAX_GRID_EDGE).contains(&hud.grid.0)
                    && (1..=MAX_GRID_EDGE).contains(&hud.grid.1),
                "Frame Warp HUD grid of {}x{}",
                hud.grid.0,
                hud.grid.1
            );
        }
        Ok(())
    }
}

/// The HUD cells of a mask `width` cells wide over a `source` picture: as many
/// rows as keep the cells' shape the picture's, and no more cells than pixels.
/// `None` for no mask.
pub fn hud_grid(width: u16, source: (u32, u32)) -> Option<(u32, u32)> {
    if width == 0 || source.0 == 0 || source.1 == 0 {
        return None;
    }
    let across = u32::from(width).min(source.0).min(MAX_GRID_EDGE);
    let down =
        (u64::from(across) * u64::from(source.1) + u64::from(source.0) / 2) / u64::from(source.0);
    Some((across, (down as u32).clamp(1, source.1.min(MAX_GRID_EDGE))))
}

/// Where the HUD pass keeps its words in its buffer: a code per pixel, then
/// per cell its changed, textured and textured-and-changed pixels, then per
/// cell the score, then the mask bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    pub(super) codes: u32,
    pub(super) counts: u32,
    pub(super) scores: u32,
    pub(super) bits: u32,
    /// Mask words per row.
    pub(super) stride: u32,
    pub(super) words: u64,
}
pub(super) fn layout(source: (u32, u32), grid: (u32, u32)) -> Result<Layout> {
    let pixels = u64::from(source.0) * u64::from(source.1);
    let cells = u64::from(grid.0) * u64::from(grid.1);
    let stride = grid.0.div_ceil(32);
    let bits = pixels + 4 * cells;
    let words = bits + u64::from(stride) * u64::from(grid.1);
    anyhow::ensure!(
        words <= u64::from(u32::MAX),
        "Frame Warp HUD buffer for {}x{}",
        source.0,
        source.1
    );
    Ok(Layout {
        codes: 0,
        counts: pixels as u32,
        scores: (pixels + 3 * cells) as u32,
        bits: bits as u32,
        stride,
        words,
    })
}
/// `cbuffer FrameWarp`'s values.
pub(super) fn constants(
    source: (u32, u32),
    pixel: Pixel,
    pass: &Pass,
    mode: u32,
    layout: Option<&Layout>,
) -> [u32; CONSTANTS as usize] {
    let grid = pass.hud.map_or((0, 0), |hud| hud.grid);
    let pixel = match pixel {
        Pixel::Bgra8 => 0,
        Pixel::RgbaF16 => 1,
        Pixel::Rgba10Pq => 2,
    };
    let layout = layout.copied().unwrap_or(Layout {
        codes: 0,
        counts: 0,
        scores: 0,
        bits: 0,
        stride: 0,
        words: 0,
    });
    [
        source.0,
        source.1,
        pass.thumbnail.0,
        pass.thumbnail.1,
        grid.0,
        grid.1,
        pixel,
        mode,
        0,
        layout.codes,
        layout.counts,
        layout.scores,
        layout.bits,
        layout.stride,
        HUD_SCORE,
        0,
    ]
}
/// `fwMode` of the score pass: clear the counts only, score, or start over.
pub(super) const CLEAR: u32 = 0;
pub(super) const SCORE: u32 = 1;
pub(super) const RESET: u32 = 2;

/// A finished thumbnail: `width` x `height` luma bytes, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailReadout {
    /// What [`FrameWarpGpu::submit`] was given for the capture.
    pub at: u64,
    pub width: usize,
    pub height: usize,
    pub luma: Vec<u8>,
}
/// A finished HUD mask: `height` rows of `width.div_ceil(32)` words, bit `i`
/// of word `w` being column `32 * w + i`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskReadout {
    pub width: u32,
    pub height: u32,
    pub words: Vec<u32>,
}

/// Constants (b4), the capture's view (t0) and the words (u8).
pub(super) fn root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature> {
    // SAFETY: `srv` and `parameters` outlive the serialize call that reads them, and the slice
    // covers exactly the blob's GetBufferSize bytes while `blob` is alive.
    unsafe {
        let srv = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 1,
            BaseShaderRegister: 0,
            RegisterSpace: 0,
            OffsetInDescriptorsFromTableStart: 0,
        };
        let parameters = [
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Constants: D3D12_ROOT_CONSTANTS {
                        ShaderRegister: 4,
                        RegisterSpace: 0,
                        Num32BitValues: CONSTANTS,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: &srv,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Descriptor: D3D12_ROOT_DESCRIPTOR {
                        ShaderRegister: 8,
                        RegisterSpace: 0,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
        ];
        let mut blob = None;
        D3D12SerializeRootSignature(
            &D3D12_ROOT_SIGNATURE_DESC {
                NumParameters: parameters.len() as u32,
                pParameters: parameters.as_ptr(),
                ..Default::default()
            },
            D3D_ROOT_SIGNATURE_VERSION_1,
            &mut blob,
            None,
        )?;
        let blob = blob.context("no Frame Warp root signature")?;
        Ok(device.CreateRootSignature(
            0,
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()),
        )?)
    }
}
