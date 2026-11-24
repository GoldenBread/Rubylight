//! PyroWave native decoder test harness and shared bitstream/readback helpers.

use super::{Api, check};
use crate::{
    capture::{ComGuard, Device, GpuImage, Image, Pixel},
    encoder::Encoder,
};
use anyhow::{Context, Result, ensure};
use butterpollo_config::config::Config;
use butterpollo_media_policy::{
    packet::{PyrowaveFec, VideoPacketizer},
    pyrowave,
    rtsp::Negotiated,
};
use butterpollo_win_abi::pyrowave as p;
use std::{collections::BTreeMap, ffi::c_void, ptr, sync::Arc, time::Instant};
use windows::{
    Win32::{
        Foundation::{CloseHandle, GENERIC_ALL},
        Graphics::{
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
    core::{Interface, PCWSTR},
};

macro_rules! decoder_api {
    ($($name:ident: $ty:ty),* $(,)?) => {
        struct DecoderApi { $($name: $ty,)* }
        impl DecoderApi {
            unsafe fn load(dll: &libloading::Library) -> Result<Self> {
                // SAFETY: Each symbol matches its 1.0 C signature, and callers keep `dll` loaded
                // inside the shared Api.
                Ok(Self { $($name: unsafe {
                    *dll.get::<$ty>(concat!("pyrowave_", stringify!($name), "\0").as_bytes())?
                },)* })
            }
        }
    };
}
decoder_api! {
    decoder_create: unsafe extern "C" fn(*const p::pyrowave_decoder_create_info, *mut p::pyrowave_decoder) -> p::pyrowave_result,
    decoder_destroy: unsafe extern "C" fn(p::pyrowave_decoder),
    decoder_clear: unsafe extern "C" fn(p::pyrowave_decoder),
    decoder_push_packet: unsafe extern "C" fn(p::pyrowave_decoder, *const c_void, usize) -> p::pyrowave_result,
    decoder_decode_is_ready: unsafe extern "C" fn(p::pyrowave_decoder, bool) -> bool,
    decoder_decode_gpu_buffer: unsafe extern "C" fn(p::pyrowave_decoder, *const p::pyrowave_gpu_sync_operation, *const p::pyrowave_gpu_sync_operation, *const p::pyrowave_gpu_buffers) -> p::pyrowave_result,
    sync_object_cpu_wait: unsafe extern "C" fn(p::pyrowave_sync_object, u64, u64) -> p::pyrowave_result,
}

struct Decoder {
    api: Arc<Api>,
    calls: DecoderApi,
    device: p::pyrowave_device,
    decoder: p::pyrowave_decoder,
    images: Vec<p::pyrowave_image>,
    textures: Vec<ID3D11Texture2D>,
    buffers: p::pyrowave_gpu_buffers,
    sync: p::pyrowave_sync_object,
    fence: Option<ID3D11Fence>,
    counter: u64,
}
impl Decoder {
    fn new(gpu: &Device, config: &Negotiated) -> Result<Self> {
        let api = Api::load()?;
        // SAFETY: `gpu` is a live D3D11 device, each create-info outlives its call, and `s` owns
        // every PyroWave handle made here so Drop releases it on error.
        unsafe {
            let mut s = Self {
                calls: DecoderApi::load(&api._dll)?,
                api,
                device: ptr::null_mut(),
                decoder: ptr::null_mut(),
                images: vec![],
                textures: vec![],
                buffers: std::mem::zeroed(),
                sync: ptr::null_mut(),
                fence: None,
                counter: 0,
            };
            let desc = gpu.device.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()?;
            let mut luid = p::pyrowave_luid { luid: [0; 8] };
            luid.luid[..4].copy_from_slice(&desc.AdapterLuid.LowPart.to_le_bytes());
            luid.luid[4..].copy_from_slice(&desc.AdapterLuid.HighPart.to_le_bytes());
            check((s.api.create_device_by_compat)(
                0,
                0,
                ptr::null(),
                ptr::null(),
                &luid,
                p::VkQueueGlobalPriority_VK_QUEUE_GLOBAL_PRIORITY_MEDIUM,
                &mut s.device,
            ))?;
            check((s.calls.decoder_create)(
                &p::pyrowave_decoder_create_info {
                    device: s.device,
                    width: config.width as i32,
                    height: config.height as i32,
                    chroma: u32::from(config.yuv444),
                    fragment_path: false,
                },
                &mut s.decoder,
            ))?;
            for plane in 0..3 {
                let divisor = if plane == 0 || config.yuv444 { 1 } else { 2 };
                let (width, height) = (config.width / divisor, config.height / divisor);
                let mut texture = None;
                gpu.device.CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: width,
                        Height: height,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: if config.ten_bit() {
                            DXGI_FORMAT_R16_UNORM
                        } else {
                            DXGI_FORMAT_R8_UNORM
                        },
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_UNORDERED_ACCESS.0)
                            as u32,
                        MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0
                            | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0)
                            as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut texture),
                )?;
                let texture = texture.unwrap();
                let handle = texture.cast::<IDXGIResource1>()?.CreateSharedHandle(
                    None,
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                    PCWSTR::null(),
                )?;
                let image_info = p::VkImageCreateInfo {
                    sType: p::VkStructureType_VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
                    imageType: p::VkImageType_VK_IMAGE_TYPE_2D,
                    format: if config.ten_bit() {
                        p::VkFormat_VK_FORMAT_R16_UNORM
                    } else {
                        p::VkFormat_VK_FORMAT_R8_UNORM
                    },
                    extent: p::VkExtent3D {
                        width,
                        height,
                        depth: 1,
                    },
                    mipLevels: 1,
                    arrayLayers: 1,
                    samples: p::VkSampleCountFlagBits_VK_SAMPLE_COUNT_1_BIT,
                    tiling: p::VkImageTiling_VK_IMAGE_TILING_OPTIMAL,
                    usage: p::VkImageUsageFlagBits_VK_IMAGE_USAGE_STORAGE_BIT
                        | p::VkImageUsageFlagBits_VK_IMAGE_USAGE_TRANSFER_SRC_BIT,
                    sharingMode: p::VkSharingMode_VK_SHARING_MODE_EXCLUSIVE,
                    ..std::mem::zeroed()
                };
                let mut image = ptr::null_mut();
                let result = (s.api.image_create)(&p::pyrowave_image_create_info {
                    device: s.device,
                    external_handle: handle.0 as usize,
                    handle_type: p::VkExternalMemoryHandleTypeFlagBits_VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_BIT,
                    image_create_info: &image_info,
                }, &mut image);
                if result != 0 {
                    let _ = CloseHandle(handle);
                    check(result).context("importing decoder output")?;
                }
                s.images.push(image);
                s.textures.push(texture);
                check((s.api.image_get_image_view)(
                    image,
                    p::VkImageAspectFlagBits_VK_IMAGE_ASPECT_COLOR_BIT,
                    p::VkImageUsageFlagBits_VK_IMAGE_USAGE_STORAGE_BIT,
                    &mut s.buffers.planes[plane],
                ))?;
            }
            gpu.device.cast::<ID3D11Device5>()?.CreateFence(
                0,
                D3D11_FENCE_FLAG_SHARED,
                &mut s.fence,
            )?;
            let handle = s.fence.as_ref().unwrap().CreateSharedHandle(
                None,
                GENERIC_ALL.0,
                PCWSTR::null(),
            )?;
            let result = (s.api.sync_object_create)(&p::pyrowave_sync_object_create_info {
                device: s.device,
                external_handle: handle.0 as usize,
                handle_type: p::VkExternalSemaphoreHandleTypeFlagBits_VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT,
                semaphore_type: p::VkSemaphoreType_VK_SEMAPHORE_TYPE_TIMELINE,
                import_flags: 0,
            }, &mut s.sync);
            if result != 0 {
                let _ = CloseHandle(handle);
                check(result)?;
            }
            Ok(s)
        }
    }

    fn decode(&mut self, gpu: &Device, packets: &[&[u8]]) -> Result<Vec<Vec<u16>>> {
        // SAFETY: `decoder`, `images` and `sync` were made in `new`; arguments outlive each call.
        unsafe {
            (self.calls.decoder_clear)(self.decoder);
            ensure!(
                !(self.calls.decoder_decode_is_ready)(self.decoder, false),
                "empty decoder is ready"
            );
            for (index, packet) in packets.iter().enumerate() {
                check((self.calls.decoder_push_packet)(
                    self.decoder,
                    packet.as_ptr().cast(),
                    packet.len(),
                ))?;
                if index + 1 < packets.len() {
                    ensure!(
                        !(self.calls.decoder_decode_is_ready)(self.decoder, false),
                        "incomplete frame is ready"
                    );
                }
            }
            ensure!(
                (self.calls.decoder_decode_is_ready)(self.decoder, false),
                "frame is incomplete"
            );
            // Each preceding staging Map has completed. The old output can be discarded.
            let acquire_images: Vec<_> = self
                .images
                .iter()
                .map(|&image| p::pyrowave_gpu_external_reference {
                    image,
                    queue_family_index: u32::MAX,
                })
                .collect();
            let release_images: Vec<_> = self
                .images
                .iter()
                .map(|&image| p::pyrowave_gpu_external_reference {
                    image,
                    queue_family_index: u32::MAX - 1,
                })
                .collect();
            let acquire = p::pyrowave_gpu_sync_operation {
                images: acquire_images.as_ptr(),
                num_images: 3,
                sync: std::mem::zeroed(),
            };
            self.counter += 1;
            let release = p::pyrowave_gpu_sync_operation {
                images: release_images.as_ptr(),
                num_images: 3,
                sync: p::pyrowave_sync_point {
                    semaphore: (self.api.sync_object_get_semaphore)(self.sync),
                    value: self.counter,
                },
            };
            check((self.calls.decoder_decode_gpu_buffer)(
                self.decoder,
                &acquire,
                &release,
                &self.buffers,
            ))?;
            check((self.calls.sync_object_cpu_wait)(
                self.sync,
                self.counter,
                10_000_000_000,
            ))
            .context("waiting for decode")?;
            gpu.context
                .cast::<ID3D11DeviceContext4>()?
                .Wait(self.fence.as_ref().unwrap(), self.counter)?;
        }
        self.textures
            .iter()
            .map(|texture| readback(gpu, texture))
            .collect()
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: Each non-null handle was made in `new` on `self.device`, destroyed last.
        unsafe {
            if !self.decoder.is_null() {
                (self.calls.decoder_destroy)(self.decoder);
            }
            for image in &self.images {
                (self.api.image_destroy)(*image);
            }
            if !self.sync.is_null() {
                (self.api.sync_object_destroy)(self.sync);
            }
            if !self.device.is_null() {
                (self.api.device_destroy)(self.device);
            }
        }
    }
}

fn readback(gpu: &Device, texture: &ID3D11Texture2D) -> Result<Vec<u16>> {
    // SAFETY: `staging` is a CPU-readable copy of `texture`, and each row read lies within the
    // mapped `RowPitch * Height` bytes, which stay valid until Unmap below.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        gpu.device
            .CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.unwrap();
        gpu.context.CopyResource(&staging, texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let mut values = Vec::with_capacity((desc.Width * desc.Height) as usize);
        let sixteen = desc.Format == DXGI_FORMAT_R16_UNORM;
        for y in 0..desc.Height as usize {
            let row = std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                desc.Width as usize * if sixteen { 2 } else { 1 },
            );
            if sixteen {
                values.extend(
                    row.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|v| u16::from_le_bytes(*v)),
                );
            } else {
                values.extend(row.iter().map(|&v| u16::from(v)));
            }
        }
        gpu.context.Unmap(&staging, 0);
        Ok(values)
    }
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

// A lossless client receive: order data shards, remove the short frame header,
// and trim the final shard. This deliberately does not use the host's layout parser.
/// One picture encoded and waited for: with two frames in flight it would
/// otherwise come back from a later call.
fn encode_now(
    encoder: &mut Encoder,
    image: &GpuImage,
    kbps: u32,
) -> Result<Vec<crate::encoder::Encoded>> {
    let mut frames = encoder.encode_gpu(image, true, kbps)?;
    frames.extend(encoder.finish()?);
    Ok(frames)
}

fn receive(packets: &[Vec<u8>]) -> Result<Vec<u8>> {
    let mut data = BTreeMap::new();
    let mut counts = BTreeMap::new();
    for packet in packets.iter().rev() {
        ensure!(
            packet.len() >= 40 && packet[0] == 0x90,
            "invalid RTP packet"
        );
        let block = (packet[27] >> 4) & 3;
        let info = word(packet, 28);
        let (count, index) = (info >> 22, (info >> 12) & 1023);
        counts.insert(block, count);
        if index < count {
            ensure!(
                data.insert((block, index), &packet[32..]).is_none(),
                "duplicate data shard"
            );
        }
    }
    ensure!(
        counts.len() == usize::from(packets[0][27] >> 6) + 1,
        "missing FEC block"
    );
    let mut joined = Vec::new();
    for (block, count) in counts {
        for index in 0..count {
            joined.extend_from_slice(data.get(&(block, index)).context("missing data shard")?);
        }
    }
    let last = usize::from(u16::from_le_bytes(joined[4..6].try_into().unwrap()));
    let shard_size = packets[0].len() - 32;
    ensure!(
        joined[0] == 1 && joined[3] == 2 && (1..=shard_size).contains(&last),
        "invalid frame header"
    );
    joined.truncate(joined.len() - shard_size + last);
    Ok(joined[8..].to_vec())
}

fn codec_packets(frame: &[u8], records: bool) -> Result<Vec<&[u8]>> {
    let mut packets = Vec::new();
    let mut offset = if records { 0 } else { 4 };
    while offset < frame.len() {
        ensure!(frame.len() - offset >= 8, "short codec header");
        let header = word(frame, offset);
        let size = if records {
            if header == u32::MAX {
                8 + word(frame, offset + 4) as usize * 4
            } else if header & 0x80000000 != 0 {
                8
            } else {
                ((header >> 16) & 0xfff) as usize * 4
            }
        } else {
            offset += 4;
            header as usize
        };
        ensure!(
            size >= 8 && size <= frame.len() - offset,
            "invalid codec packet length"
        );
        if !records || header != u32::MAX {
            packets.push(&frame[offset..offset + size]);
        }
        offset += size;
    }
    ensure!(
        records || packets.len() == word(frame, 0) as usize,
        "packet count mismatch"
    );
    Ok(packets)
}

mod e2e_tests;
mod recovery_tests;
mod scenes;
use e2e_tests::stream_status;
use scenes::{Scene, compare, picture, ssim};
