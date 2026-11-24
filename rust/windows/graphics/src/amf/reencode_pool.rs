//! Reusable single-slice D3D11 texture pool for AMF VPE scene-cut IDR re-encodes.
use super::*;

/// Small reusable slot pool keyed by `(width, height, format)`: up to 2 idle
/// entries (`Arc::strong_count == 1`) are kept so scene-cut re-encodes avoid
/// per-frame `CreateTexture2D` allocations.
struct TextureSlots<K, T> {
    key: Option<K>,
    slots: Vec<std::sync::Arc<T>>,
}
impl<K, T> Default for TextureSlots<K, T> {
    fn default() -> Self {
        Self {
            key: None,
            slots: Vec::new(),
        }
    }
}
impl<K: PartialEq, T> TextureSlots<K, T> {
    const CAP: usize = 2;

    fn clear(&mut self) {
        self.key = None;
        self.slots.clear();
    }
    fn acquire<E>(
        &mut self,
        key: K,
        create: impl FnOnce() -> std::result::Result<T, E>,
    ) -> std::result::Result<std::sync::Arc<T>, E> {
        if self.key.as_ref() != Some(&key) {
            self.slots.clear();
            self.key = Some(key);
        }
        if let Some(reused) = self
            .slots
            .iter()
            .find(|slot| std::sync::Arc::strong_count(slot) == 1)
        {
            return Ok(reused.clone());
        }
        let created = std::sync::Arc::new(create()?);
        if self.slots.len() < Self::CAP {
            self.slots.push(created.clone());
        }
        Ok(created)
    }
}

/// Cached 2-texture single-slice D3D11 pool for VPE scene-cut IDR re-encodes.
#[derive(Default)]
pub(super) struct ReencodePool {
    slots: TextureSlots<
        (
            u32,
            u32,
            windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
        ),
        windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    >,
}
impl ReencodePool {
    pub(super) fn clear(&mut self) {
        self.slots.clear();
    }
    /// Copy `subresource` of `texture` into a reused exclusive single-slice
    /// D3D11 texture suitable for wrapping as an AMF input surface.
    pub(super) fn copy(
        &mut self,
        gpu: &Device,
        texture: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
        subresource: u32,
    ) -> Result<std::sync::Arc<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>> {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_DEFAULT, ID3D11Multithread, ID3D11Texture2D,
        };
        // SAFETY: `texture` and `gpu.device` are live on the same D3D11 device;
        // `desc` is filled by `GetDesc`, and the copy runs under the context's
        // multithread lock.
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let key = (desc.Width, desc.Height, desc.Format);
            let copy = self.slots.acquire(key, || {
                let copy_desc = D3D11_TEXTURE2D_DESC {
                    Width: desc.Width,
                    Height: desc.Height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: desc.Format,
                    SampleDesc: desc.SampleDesc,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut created: Option<ID3D11Texture2D> = None;
                gpu.device
                    .CreateTexture2D(&copy_desc, None, Some(&mut created))
                    .context("allocate D3D11 texture copy for AMF re-encode")?;
                created.context("D3D11 returned no texture copy")
            })?;
            let lock: ID3D11Multithread = gpu.context.cast()?;
            lock.Enter();
            gpu.context
                .CopySubresourceRegion(&*copy, 0, 0, 0, 0, texture, subresource, None);
            lock.Leave();
            Ok(copy)
        }
    }
}

#[cfg(test)]
mod pool_tests {
    use super::*;

    #[test]
    fn reencode_texture_slots_reuse_up_to_two_and_reset_on_key_change() {
        let mut pool = TextureSlots::<(u32, u32), u32>::default();
        let next = std::cell::Cell::new(0u32);
        let make = || {
            let value = next.get() + 1;
            next.set(value);
            Ok::<_, ()>(value)
        };
        let a = pool.acquire((1920, 1080), make).unwrap();
        assert_eq!(*a, 1);
        // While `a` is held, a second acquisition allocates slot 2.
        let b = pool.acquire((1920, 1080), make).unwrap();
        assert_eq!(*b, 2);
        assert_eq!(pool.slots.len(), 2);
        // Dropping `a` returns slot 1 for reuse without creating slot 3.
        drop(a);
        let reused = pool.acquire((1920, 1080), make).unwrap();
        assert_eq!(*reused, 1);
        assert_eq!(next.get(), 2);
        // Changing key clears the cached slots and allocates fresh.
        drop(b);
        drop(reused);
        let resized = pool.acquire((2560, 1440), make).unwrap();
        assert_eq!(*resized, 3);
        assert_eq!(pool.slots.len(), 1);
    }
}
