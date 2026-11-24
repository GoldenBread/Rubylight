//! Pure NVENC parameter helpers: codec/preset GUIDs, VUI/color mapping,
//! HDR10 mastering/light metadata conversion, and API fallback diagnostics.

use anyhow::{Result, bail};
use butterpollo_media_policy::{
    nvenc::{ApiVersion, Tuning},
    rtsp::Negotiated,
};
use butterpollo_win_abi::nvenc::*;

/// What a stream loses when the driver rejected a newer API and an older one
/// opened instead, for its session warning: the retry must not drop HDR10
/// metadata or split encoding silently.
pub(super) fn api_fallback_warning(
    failures: &[String],
    api: ApiVersion,
    stream: &Negotiated,
    tuning: &Tuning,
) -> String {
    let mut lost = Vec::new();
    if stream.hdr && api < ApiVersion(13, 0) {
        lost.push("HDR10 mastering metadata is sent on the control channel only");
    }
    if stream.codec != 0
        && tuning.split != butterpollo_media_policy::nvenc::SPLIT_DISABLED
        && !api.modern_depth()
    {
        lost.push("split-frame encoding is unavailable");
    }
    let lost = if lost.is_empty() {
        "The stream continues, but without the newer API's encoder features.".to_owned()
    } else {
        format!("The stream continues, but {}.", lost.join(", and "))
    };
    format!(
        "The NVIDIA driver rejected a newer NVENC API ({}) and Rubylight opened {}.{}. {lost} Update the NVIDIA driver if HDR looks wrong or encoding is slower than expected.",
        failures.join("; "),
        api.0,
        api.1
    )
}

pub(super) fn validate(functions: &NV_ENCODE_API_FUNCTION_LIST) -> Result<()> {
    macro_rules! require { ($($entry:ident),+ $(,)?) => { $(
        if functions.$entry.is_none() { bail!(concat!("NVENC driver lacks ", stringify!($entry))); }
    )+ }; }
    require!(
        nvEncOpenEncodeSessionEx,
        nvEncGetEncodeCaps,
        nvEncInitializeEncoder,
        nvEncRegisterResource,
        nvEncUnregisterResource,
        nvEncMapInputResource,
        nvEncUnmapInputResource,
        nvEncCreateBitstreamBuffer,
        nvEncDestroyBitstreamBuffer,
        nvEncEncodePicture,
        nvEncLockBitstream,
        nvEncUnlockBitstream,
        nvEncDestroyEncoder
    );
    Ok(())
}

pub(super) fn codec_guid(codec: u8) -> Result<GUID> {
    match codec {
        0 => Ok(NV_ENC_CODEC_H264_GUID),
        1 => Ok(NV_ENC_CODEC_HEVC_GUID),
        2 => Ok(NV_ENC_CODEC_AV1_GUID),
        _ => bail!("unsupported NVENC codec"),
    }
}

pub(super) fn preset_guid(preset: usize) -> GUID {
    [
        NV_ENC_PRESET_P1_GUID,
        NV_ENC_PRESET_P2_GUID,
        NV_ENC_PRESET_P3_GUID,
        NV_ENC_PRESET_P4_GUID,
        NV_ENC_PRESET_P5_GUID,
        NV_ENC_PRESET_P6_GUID,
        NV_ENC_PRESET_P7_GUID,
    ][preset.clamp(1, 7) - 1]
}

pub(super) fn buffer_format(config: &Negotiated) -> NV_ENC_BUFFER_FORMAT {
    match (config.yuv444, config.ten_bit()) {
        (false, false) => _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_NV12,
        (false, true) => _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_YUV420_10BIT,
        (true, false) => _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_AYUV,
        (true, true) => _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_YUV444_10BIT,
    }
}

pub(super) fn vui(config: &Negotiated) -> NV_ENC_CONFIG_H264_VUI_PARAMETERS {
    let (primaries, transfer, matrix) = colors(config);
    NV_ENC_CONFIG_H264_VUI_PARAMETERS {
        videoSignalTypePresentFlag: 1,
        videoFormat: 5,
        videoFullRangeFlag: u32::from(config.full_range()),
        colourDescriptionPresentFlag: 1,
        colourPrimaries: primaries,
        transferCharacteristics: transfer,
        colourMatrix: matrix,
        chromaSampleLocationFlag: u32::from(!config.yuv444),
        bitstreamRestrictionFlag: 1,
        ..Default::default()
    }
}

pub(super) fn colors(config: &Negotiated) -> (u32, u32, u32) {
    let (primaries, matrix, transfer) = match config.color_matrix() {
        0 => (6, 6, 6),
        2 => (9, 9, 14),
        _ => (1, 1, 1),
    };
    // PQ, or ARIB STD-B67 for HLG and Dolby Vision 8.4.
    let transfer = match (config.hdr, config.hlg()) {
        (true, true) => 18,
        (true, false) => 16,
        _ => transfer,
    };
    (primaries, transfer, matrix)
}

pub(super) fn same_guid(a: &GUID, b: &GUID) -> bool {
    a.Data1 == b.Data1 && a.Data2 == b.Data2 && a.Data3 == b.Data3 && a.Data4 == b.Data4
}

/// The codec's name for a refusal.
pub(super) fn codec_name(codec: u8) -> &'static str {
    match codec {
        0 => "H.264",
        1 => "HEVC",
        _ => "AV1",
    }
}

/// What [`super::Session::open`] tried: each rejected API, or, when the driver
/// is older than all of them, its API and the reviewed ones for the codec.
pub(super) fn api_failures(codec: u8, maximum: ApiVersion, failures: &[String]) -> String {
    if !failures.is_empty() {
        return failures.join("; ");
    }
    let reviewed = ApiVersion::candidates(codec, ApiVersion::COMPILED)
        .map(|api| format!("{}.{}", api.0, api.1))
        .collect::<Vec<_>>();
    format!(
        "this driver offers API {}.{}, and {} needs one of {}. Update the NVIDIA driver",
        maximum.0,
        maximum.1,
        codec_name(codec),
        if reviewed.is_empty() {
            "none".into()
        } else {
            reviewed.join(", ")
        }
    )
}

/// The NVENC capability behind AMF's throughput property names, for the
/// codec it names.
pub(super) fn throughput_codec(name: &str) -> Option<u8> {
    match name {
        "MaxThroughput" => Some(0),
        "HevcMaxThroughput" => Some(1),
        "Av1CapMaxThroughput" => Some(2),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EncodedMetadata {
    pub(super) primaries: [[u16; 2]; 3],
    pub(super) white: [u16; 2],
    pub(super) maximum: u32,
    pub(super) minimum: u32,
    pub(super) max_cll: u16,
    pub(super) max_fall: u16,
}

#[derive(Default)]
pub(super) struct Metadata {
    pub(super) mastering: Box<MASTERING_DISPLAY_INFO>,
    pub(super) light: Box<CONTENT_LIGHT_LEVEL>,
}

impl Metadata {
    pub(super) fn new(codec: u8) -> Self {
        let mut metadata = Self::default();
        metadata.update(butterpollo_media_policy::hdr::Metadata::default(), codec);
        metadata
    }

    pub(super) fn update(&mut self, metadata: butterpollo_media_policy::hdr::Metadata, codec: u8) {
        // NVENC takes each codec's syntax units, not a common HDR10 unit.
        // AV1 uses 0.16 chromaticities, 24.8 maximum and 18.14 minimum nits;
        // HEVC uses 1/50000 chromaticities and 1/10000 nit luminance.
        let point = |[x, y]: [u16; 2]| {
            let scale = |value| {
                if codec == 2 {
                    ((u32::from(value) * 65536 + 25000) / 50000).min(u32::from(u16::MAX)) as u16
                } else {
                    value
                }
            };
            CHROMA_POINTS {
                x: scale(x),
                y: scale(y),
            }
        };
        self.mastering.r = point(metadata.primaries[0]);
        self.mastering.g = point(metadata.primaries[1]);
        self.mastering.b = point(metadata.primaries[2]);
        self.mastering.whitePoint = point(metadata.white);
        self.mastering.maxLuma =
            u32::from(metadata.maximum_nits) * if codec == 2 { 256 } else { 10000 };
        self.mastering.minLuma = if codec == 2 {
            (u32::from(metadata.minimum) * 16384 + 5000) / 10000
        } else {
            u32::from(metadata.minimum)
        };
        self.light.maxContentLightLevel = metadata.max_cll;
        self.light.maxPicAverageLightLevel = metadata.max_fall;
    }

    pub(super) fn encoded(&self) -> EncodedMetadata {
        let point = |p: CHROMA_POINTS| [p.x, p.y];
        EncodedMetadata {
            primaries: [
                point(self.mastering.r),
                point(self.mastering.g),
                point(self.mastering.b),
            ],
            white: point(self.mastering.whitePoint),
            maximum: self.mastering.maxLuma,
            minimum: self.mastering.minLuma,
            max_cll: self.light.maxContentLightLevel,
            max_fall: self.light.maxPicAverageLightLevel,
        }
    }
}
