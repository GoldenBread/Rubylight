//! Ring caches for D3D11 4:4:4 surface descriptions and slice shader-resource views.

use std::collections::HashMap;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::D3D11_SRV_DIMENSION_TEXTURE2DARRAY;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

use super::surface_view;

const MAX_CACHED_TEXTURES: usize = 32;

struct SurfaceFacts {
    key: usize,
    _texture: ID3D11Texture2D,
    desc: D3D11_TEXTURE2D_DESC,
}

type SliceViews = HashMap<(u32, i32), ID3D11ShaderResourceView>;

/// Caches `D3D11_TEXTURE2D_DESC` and slice SRVs across a shared texture ring without per-frame
/// queries or view recreation.
#[derive(Default)]
pub(super) struct SurfaceCache {
    facts: Vec<SurfaceFacts>,
    views: Vec<(usize, ID3D11Texture2D, SliceViews)>,
}

impl SurfaceCache {
    pub(super) fn clear(&mut self) {
        self.facts.clear();
        self.views.clear();
    }

    pub(super) fn desc(
        &mut self,
        device: &ID3D11Device,
        source: &ID3D11Texture2D,
        #[cfg(test)] queries: &mut u32,
    ) -> Result<D3D11_TEXTURE2D_DESC, String> {
        let key = source.as_raw() as usize;
        if let Some(facts) = self.facts.iter().find(|f| f.key == key) {
            return Ok(facts.desc);
        }
        #[cfg(test)]
        {
            *queries += 1;
        }
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: A live texture.
        unsafe { source.GetDesc(&mut desc) };
        // SAFETY: A live texture's device.
        let owner = unsafe { source.GetDevice() }
            .map_err(|e| format!("the decoder's texture has no device: {e}"))?;
        if owner.as_raw() != device.as_raw() {
            return Err(
                "the decoder's pictures are on another Direct3D 11 device than the renderer's"
                    .into(),
            );
        }
        if self.facts.len() >= MAX_CACHED_TEXTURES {
            self.facts.remove(0);
        }
        self.facts.push(SurfaceFacts {
            key,
            _texture: source.clone(),
            desc,
        });
        Ok(desc)
    }

    /// Creates or retrieves a cached SRV for `source[slice]`, running `CreateShaderResourceView`
    /// under `lock` so shared-device decoding never races with view creation.
    pub(super) fn slice_view(
        &mut self,
        device: &ID3D11Device,
        source: &ID3D11Texture2D,
        key: usize,
        slice: u32,
        format: DXGI_FORMAT,
        lock: &mut dyn FnMut(&mut dyn FnMut()),
    ) -> Result<ID3D11ShaderResourceView, String> {
        let (view_format, _) = surface_view(format).ok_or("not a 4:4:4 format")?;
        let pos = match self.views.iter().position(|(texture, _, _)| *texture == key) {
            Some(pos) => pos,
            None => {
                if self.views.len() >= MAX_CACHED_TEXTURES {
                    self.views.remove(0);
                }
                self.views.push((key, source.clone(), HashMap::new()));
                self.views.len() - 1
            }
        };
        let (_, _, views) = &mut self.views[pos];
        if let Some(view) = views.get(&(slice, format.0)) {
            return Ok(view.clone());
        }
        let mut made: Result<ID3D11ShaderResourceView, String> = Err("no view".into());
        // SAFETY: View of a live shader-readable texture, serialized with the decoder lock.
        lock(&mut || {
            let mut view = None;
            made = unsafe {
                device
                    .CreateShaderResourceView(
                        source,
                        Some(&D3D11_SHADER_RESOURCE_VIEW_DESC {
                            Format: view_format,
                            ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2DARRAY,
                            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                                Texture2DArray: D3D11_TEX2D_ARRAY_SRV {
                                    MostDetailedMip: 0,
                                    MipLevels: 1,
                                    FirstArraySlice: slice,
                                    ArraySize: 1,
                                },
                            },
                        }),
                        Some(&mut view),
                    )
                    .map_err(|e| format!("a view of slice {slice}: {e}"))
                    .and_then(|()| view.ok_or_else(|| "no view".into()))
            };
        });
        let view = made?;
        views.insert((slice, format.0), view.clone());
        Ok(view)
    }
}
