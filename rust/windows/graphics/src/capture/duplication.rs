//! DXGI Desktop Duplication capture, colour-space refresh on open, and HDR transition detection.
use super::*;

pub struct Duplication {
    pub(super) gpu: Device,
    duplicate: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
    pub(super) owned: GpuPool,
    /// The newest desktop publication, shared with the capture worker: a
    /// pointer-only update re-publishes it with fresh stamps instead of
    /// copying the desktop again.
    last_desktop: Option<std::sync::Arc<GpuImage>>,
    pub(super) cursor: crate::cursor::State,
    frame_owned: bool,
    hdr: bool,
    supports_fp16: bool,
    surface_format: DXGI_FORMAT,
    color_space: Option<DXGI_COLOR_SPACE_TYPE>,
    color_check: Instant,
}

/// Whether Desktop Duplication should force a fresh DXGI/DisplayConfig colour
/// read right after `DuplicateOutput1`: either the DXGI factory went stale
/// while opening duplication, or the returned desktop surface format disagrees
/// with the pre-open `in_hdr` reading (common during HDR/fullscreen transitions
/// because the pre-open query otherwise stays cached for a full second).
fn duplication_needs_color_refresh(
    format: DXGI_FORMAT,
    pre_open_in_hdr: Option<bool>,
    factory_current: bool,
) -> bool {
    !factory_current
        || (format == DXGI_FORMAT_R16G16B16A16_FLOAT && pre_open_in_hdr != Some(true))
        || (format == DXGI_FORMAT_B8G8R8A8_UNORM && pre_open_in_hdr == Some(true))
}

/// Whether an open Desktop Duplication session must reopen to switch from
/// 8-bit BGRA8 to FP16 because the output entered HDR10 without throwing
/// `DXGI_ERROR_ACCESS_LOST`. An FP16 duplication never needs reopening for a
/// colour-space update, and a legacy `DuplicateOutput` or unchanged colour
/// space never loops.
fn duplication_reopen_for_hdr(
    hdr: bool,
    supports_fp16: bool,
    surface_format: DXGI_FORMAT,
    previous: Option<DXGI_COLOR_SPACE_TYPE>,
    current: Option<output::Description>,
) -> bool {
    hdr && supports_fp16
        && surface_format == DXGI_FORMAT_B8G8R8A8_UNORM
        && output::mode_changed(previous, current)
        && output::in_hdr(current) == Some(true)
}

/// Desktop Duplication hands a turned display's frames over as the display
/// scans them out, sideways or upside down for the stream, so it refuses one
/// rather than stream it so. Windows Graphics Capture captures it upright.
pub(super) fn duplication_upright(rotation: DXGI_MODE_ROTATION, output: &str) -> Result<()> {
    if matches!(
        rotation,
        DXGI_MODE_ROTATION_UNSPECIFIED | DXGI_MODE_ROTATION_IDENTITY
    ) {
        return Ok(());
    }
    bail!(
        "{output} is turned in Windows, and Desktop Duplication would stream it sideways. Use capture = auto or wgc, or turn the display upright"
    )
}

impl Duplication {
    pub fn new(name: &str) -> Result<Self> {
        Self::new_format(name, false)
    }
    pub fn new_format(name: &str, hdr: bool) -> Result<Self> {
        Self::new_device(Device::new(name)?, hdr)
    }
    pub(super) fn new_device(gpu: Device, hdr: bool) -> Result<Self> {
        let output = gpu.output()?.clone();
        // Desktop Duplication works only on the GPU the display is connected
        // to; WGC also captures it for a device on another GPU.
        if let Some(mismatch) = GpuMismatch::of(&gpu) {
            bail!("{}", mismatch.refusal());
        }
        let build = crate::process_info::windows_build();
        use butterpollo_media_policy::os_gate::{Feature, missing};
        if hdr && let Some(reason) = missing(build, Feature::OutputDesc1) {
            tracing::warn!("{reason} An HDR stream will be captured as SDR.");
        }
        let pre_open_desc = gpu.color_description()?;
        let pre_open_color_space = pre_open_desc.map(|desc| desc.color_space);
        let pre_open_in_hdr = output::in_hdr(pre_open_desc);
        let native_hdr = hdr && pre_open_in_hdr == Some(true);
        // Always name the formats this host accepts. The legacy call loses
        // access in a loop whenever Windows composes the output in FP16, as it
        // does for advanced color or after another virtual display arrives.
        let formats: &[DXGI_FORMAT] = if native_hdr {
            &[DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_B8G8R8A8_UNORM]
        } else {
            &[DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT]
        };
        // SAFETY: `output` and `gpu.device` are live, and `formats` outlives the DuplicateOutput1
        // call that reads it.
        let (duplicate, api) = unsafe {
            // DuplicateOutput1 needs a per-monitor DPI-aware process; fall back
            // to the legacy call rather than fail where it is refused.
            let unsupported = missing(build, Feature::DuplicateOutput1);
            match unsupported
                .as_ref()
                .map(|reason| {
                    Err(windows::core::Error::new(
                        windows::Win32::Foundation::E_NOTIMPL,
                        reason.as_str(),
                    ))
                })
                .unwrap_or_else(|| {
                    output
                        .cast::<IDXGIOutput5>()
                        .and_then(|output| output.DuplicateOutput1(&gpu.device, 0, formats))
                }) {
                Ok(duplicate) => (duplicate, "DuplicateOutput1"),
                Err(error) => {
                    tracing::warn!(%error, "DuplicateOutput1 unavailable; trying legacy Desktop Duplication, which may lose HDR or fail after display changes. Update Windows and the graphics driver or select WGC");
                    (
                        output.DuplicateOutput(&gpu.device).with_context(|| {
                            format!("opening legacy Desktop Duplication after DuplicateOutput1 failed: {error}")
                        })?,
                        "DuplicateOutput",
                    )
                }
            }
        };
        // SAFETY: `duplicate` is a live output duplication, and GetDesc returns an owned struct.
        let desc = unsafe { duplicate.GetDesc() };
        if duplication_needs_color_refresh(
            desc.ModeDesc.Format,
            pre_open_in_hdr,
            gpu.color_is_current(),
        ) {
            gpu.refresh_color();
        }
        let supports_fp16 = api == "DuplicateOutput1";
        let current_desc = gpu.color_description().ok().flatten();
        tracing::info!(
            api,
            output = %gpu.display.display_name,
            width = desc.ModeDesc.Width,
            height = desc.ModeDesc.Height,
            x = gpu.display.x,
            y = gpu.display.y,
            rotation = desc.Rotation.0,
            format = desc.ModeDesc.Format.0,
            pixel = ?Pixel::try_from_dxgi(desc.ModeDesc.Format),
            color_space = current_desc.map(|d| d.color_space.0),
            in_hdr = output::in_hdr(current_desc),
            sdr_white_nits = gpu.sdr_white_nits(),
            "Desktop Duplication opened"
        );
        duplication_upright(desc.Rotation, &gpu.display.display_name)?;
        // If Windows switched into HDR while `DuplicateOutput1` opened BGRA8,
        // re-check immediately so the next acquire reopens into FP16 without
        // waiting a full check interval.
        let reopen_now = duplication_reopen_for_hdr(
            hdr,
            supports_fp16,
            desc.ModeDesc.Format,
            pre_open_color_space,
            current_desc,
        );
        Ok(Self {
            gpu,
            duplicate,
            staging: None,
            owned: GpuPool::default(),
            last_desktop: None,
            cursor: Default::default(),
            frame_owned: false,
            hdr,
            supports_fp16,
            surface_format: desc.ModeDesc.Format,
            color_space: pre_open_color_space,
            color_check: if reopen_now {
                Instant::now()
            } else {
                Instant::now() + Duration::from_secs(1)
            },
        })
    }
    fn check_color_space(&mut self) -> Result<()> {
        if Instant::now() < self.color_check {
            return Ok(());
        }
        self.color_check = Instant::now() + Duration::from_secs(1);
        let current = self.gpu.color_description()?;
        if duplication_reopen_for_hdr(
            self.hdr,
            self.supports_fp16,
            self.surface_format,
            self.color_space,
            current,
        ) {
            bail!("capture output color space changed to HDR");
        }
        Ok(())
    }
    pub fn next(&mut self, timeout: Duration) -> Result<Option<Image>> {
        self.check_color_space()?;
        let Some((_, resource)) = self.acquire_frame(timeout.as_millis().min(100) as u32)? else {
            return Ok(None);
        };
        // CPU and GPU callers may alternate on the same duplication object.
        // A CPU acquisition cannot leave an older GPU desktop cache reusable.
        self.last_desktop = None;
        let result = (|| -> Result<Image> {
            let texture: ID3D11Texture2D = resource.context("empty captured texture")?.cast()?;
            read_texture(&self.gpu, &texture, &mut self.staging)
        })();
        if result.is_err() {
            let _ = self.release_frame();
        }
        result.map(Some)
    }
    pub fn next_gpu(&mut self, origin: u64) -> Result<Option<std::sync::Arc<GpuImage>>> {
        self.check_color_space()?;
        // Keep acquisition nonblocking. A waiting acquisition holds the device's
        // lock, so another thread's D3D11 call took up to 42 ms meanwhile, and
        // beside a game it delivered fewer pictures, later, than polling
        // (`examples/ddx_arrival_probe.rs`).
        if !self.owned.has_capacity()
            && self.last_desktop.as_ref().is_some_and(|image| {
                std::sync::Arc::strong_count(image) == 1
                    && std::sync::Arc::strong_count(&image.texture) == 2
            })
        {
            // The cache can be the last reader of a snapshot. Let that slot
            // return before deciding whether a new desktop can be acquired.
            self.last_desktop = None;
        }
        let available = self.owned.has_capacity();
        if !available {
            // Acquisition normally reports loss. Backpressure must not hide a
            // removed device/output while consumers still own its snapshots.
            if let Some(loss) = crate::device_loss::DeviceLost::d3d11(&self.gpu.device) {
                return Err(loss.into());
            }
            let _ = self.gpu.color_description()?;
        }
        let Some((info, resource)) = acquire_snapshot_frame(available, || self.acquire_frame(0))?
        else {
            return Ok(None);
        };
        let shown = self.cursor.shown();
        let desktop_updated = info.LastPresentTime != 0;
        // The picture is a fresh copy, and so the new cache, unless a
        // pointer-only update re-publishes the cached desktop.
        let fresh = desktop_updated || self.last_desktop.is_none();
        let result: Result<_> = (|| {
            self.cursor.update(&self.gpu, &self.duplicate, &info)?;
            // A pointer-only update while the pointer stays hidden (mouse-look
            // in a game) changes nothing on screen. Publishing the picture
            // again spent the pacing slot of the game's next frame. The frame
            // is released at the next acquisition.
            if info.LastPresentTime == 0
                && self.last_desktop.is_some()
                && !shown
                && !self.cursor.shown()
            {
                return Ok(None);
            }
            let texture: ID3D11Texture2D = resource.context("empty captured texture")?.cast()?;
            let mut image = self.owned.desktop_snapshot(
                &self.gpu,
                &texture,
                &mut self.last_desktop,
                desktop_updated,
            )?;
            if let Some(image) = image.as_mut() {
                image.captured =
                    qpc_timestamps(info.LastPresentTime.max(info.LastMouseUpdateTime)).0;
                image.acquired = Instant::now();
                image.cursor = self.cursor.snapshot();
                image.published = Instant::now();
                image.origin = origin;
            }
            Ok(image)
        })();
        if result.is_err() {
            let _ = self.release_frame();
        }
        // An updated desktop is wrapped once and shared: the publication and
        // the cache a later pointer-only update re-publishes are the same
        // picture, not a clone of it made each frame.
        Ok(result?.map(|image| {
            let image = std::sync::Arc::new(image);
            if fresh {
                self.last_desktop = Some(image.clone());
            }
            image
        }))
    }
    fn release_frame(&mut self) -> Result<()> {
        if std::mem::replace(&mut self.frame_owned, false) {
            // SAFETY: `frame_owned` was set, so this duplication holds an acquired frame that has
            // not been released.
            unsafe {
                self.duplicate.ReleaseFrame()?;
            }
        }
        Ok(())
    }
    fn acquire_frame(
        &mut self,
        timeout_ms: u32,
    ) -> Result<Option<(DXGI_OUTDUPL_FRAME_INFO, Option<IDXGIResource>)>> {
        // Release immediately before acquisition, as recommended by DXGI.
        // Between polls, Windows tracks dirty regions instead of repeatedly
        // copying desktop updates into a surface we have already snapshotted.
        self.release_frame()?;
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        // SAFETY: `duplicate` is live, the previous frame was released above, and both
        // out-parameters are live locals.
        match unsafe {
            self.duplicate
                .AcquireNextFrame(timeout_ms, &mut info, &mut resource)
        } {
            Ok(()) => {
                self.frame_owned = true;
                Ok(Some((info, resource)))
            }
            Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for Duplication {
    fn drop(&mut self) {
        let _ = self.release_frame();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplication_refreshes_colour_when_surface_format_or_factory_disagrees() {
        // Steady-state matches skip extra enumeration.
        assert!(!duplication_needs_color_refresh(
            DXGI_FORMAT_B8G8R8A8_UNORM,
            Some(false),
            true
        ));
        assert!(!duplication_needs_color_refresh(
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            Some(true),
            true
        ));
        assert!(!duplication_needs_color_refresh(
            DXGI_FORMAT_B8G8R8A8_UNORM,
            None,
            true
        ));
        // Format/HDR disagreements or a stale factory force an immediate refresh.
        assert!(duplication_needs_color_refresh(
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            Some(false),
            true
        ));
        assert!(duplication_needs_color_refresh(
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            None,
            true
        ));
        assert!(duplication_needs_color_refresh(
            DXGI_FORMAT_B8G8R8A8_UNORM,
            Some(true),
            true
        ));
        assert!(duplication_needs_color_refresh(
            DXGI_FORMAT_B8G8R8A8_UNORM,
            Some(false),
            false
        ));
    }

    #[test]
    fn duplication_reopens_only_when_bgra8_capture_enters_hdr_on_duplicate_output1() {
        let hdr_desc = Some(output::Description {
            color_space: DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020,
            metadata: butterpollo_media_policy::hdr::Metadata::default(),
        });
        let sdr_desc = Some(output::Description {
            color_space: DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            metadata: butterpollo_media_policy::hdr::Metadata::default(),
        });
        // BGRA8 opened in SDR (or unknown colour space) reopens once the output enters HDR.
        assert!(duplication_reopen_for_hdr(
            true,
            true,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            Some(DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709),
            hdr_desc,
        ));
        assert!(duplication_reopen_for_hdr(
            true,
            true,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            None,
            hdr_desc,
        ));
        // Already FP16 never needs reopening (avoids freeze when HDR settles on an FP16 surface).
        assert!(!duplication_reopen_for_hdr(
            true,
            true,
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            Some(DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709),
            hdr_desc,
        ));
        // Unchanged HDR colour space, legacy DuplicateOutput, SDR streams, and SDR transitions never loop.
        assert!(!duplication_reopen_for_hdr(
            true,
            true,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            Some(DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020),
            hdr_desc,
        ));
        assert!(!duplication_reopen_for_hdr(
            true,
            false,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            Some(DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709),
            hdr_desc,
        ));
        assert!(!duplication_reopen_for_hdr(
            false,
            true,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            Some(DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709),
            hdr_desc,
        ));
        assert!(!duplication_reopen_for_hdr(
            true,
            true,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            Some(DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020),
            sdr_desc,
        ));
    }
}
