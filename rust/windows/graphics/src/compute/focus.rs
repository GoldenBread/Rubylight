//! D3D12 compute encoder focus still-age detection and ROI importance map readback.

use super::*;

// Encoder focus (docs/encoder-focus-design.md): the conversion also keeps a
// still-age per 16x16 block and writes the codec blocks' ROI importances,
// read back without a CPU wait. Off, `convert` records exactly what it did.

/// What the focus passes use for the next conversions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusSettings {
    pub mode: butterpollo_media_policy::encoder_focus::Mode,
    pub levels: butterpollo_media_policy::encoder_focus::Levels,
    /// The cursor in target pixels, while it is in the picture.
    pub cursor: Option<(i32, i32)>,
}
/// A finished conversion's ROI map, one u32 importance per codec block.
pub struct MapView<'a> {
    /// The compute fence value of the conversion that wrote it.
    pub value: u64,
    /// Every block has the same importance, so the map would change nothing.
    pub uniform: bool,
    pub rows: &'a [u32],
    pub width: u32,
    pub height: u32,
}
/// The still-age detector's and map's GPU objects.
pub(super) struct Focus {
    pub(super) root: ID3D12RootSignature,
    pub(super) convert: ID3D12PipelineState,
    pub(super) map: ID3D12PipelineState,
    /// `{signature, age}` per 16x16 block.
    pub(super) ages: ID3D12Resource,
    /// `{min, max}`, then the importances.
    pub(super) importances: ID3D12Resource,
    /// One persistently mapped READBACK copy of `importances` per slot.
    pub(super) readback: Vec<(ID3D12Resource, *const u32)>,
    pub(super) block: u32,
    pub(super) age_grid: (u32, u32),
    pub(super) grid: (u32, u32),
    pub(super) settings: Option<FocusSettings>,
    /// Whether each slot's last conversion wrote a map.
    pub(super) written: [bool; SLOTS],
    /// The next focus conversion starts every age at 0.
    pub(super) reset: bool,
}
/// Root parameters 3 to 5 of the focus root signature: the ages (u5), the
/// map (u6) and the `Focus` constants (b1).
pub(super) fn focus_parameters() -> [D3D12_ROOT_PARAMETER; 3] {
    let uav = |register| D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR {
                ShaderRegister: register,
                RegisterSpace: 0,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    };
    [
        uav(5),
        uav(6),
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 1,
                    RegisterSpace: 0,
                    Num32BitValues: 16,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
    ]
}
pub(crate) fn focus_buffer(
    device: &ID3D12Device,
    heap: D3D12_HEAP_TYPE,
    size: u64,
    state: D3D12_RESOURCE_STATES,
) -> Result<ID3D12Resource> {
    let mut desc = buffer_desc(size);
    if heap == D3D12_HEAP_TYPE_DEFAULT {
        desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    }
    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: `device` is live and the descriptions and out-parameter outlive the call.
    unsafe {
        device.CreateCommittedResource(
            &D3D12_HEAP_PROPERTIES {
                Type: heap,
                ..Default::default()
            },
            D3D12_HEAP_FLAG_NONE,
            &desc,
            state,
            None,
            &mut buffer,
        )?;
    }
    buffer.context("no encoder focus buffer")
}
pub(super) fn uav_barrier() -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                pResource: std::mem::ManuallyDrop::new(None),
            }),
        },
    }
}
impl Focus {
    fn map_bytes(&self) -> u64 {
        4 * (2 + u64::from(self.grid.0) * u64::from(self.grid.1))
    }
    pub(super) fn constants(&self) -> [u32; 16] {
        let settings = self.settings.expect("focus constants without settings");
        let levels = settings.levels;
        let cursor = settings
            .cursor
            .map(butterpollo_media_policy::encoder_focus::clamp_cursor);
        let (x, y) = cursor.unwrap_or_default();
        [
            settings.mode.shader(),
            levels.moving,
            levels.still,
            levels.ramp,
            levels.centre,
            levels.ctb_still_256(),
            levels.noise_bits.min(8),
            self.block,
            self.age_grid.0,
            self.age_grid.1,
            self.grid.0,
            self.grid.1,
            x as u32,
            y as u32,
            u32::from(cursor.is_some()),
            u32::from(self.reset),
        ]
    }
    /// Bind the ages, the map and the constants for `yuv420_focus_cs`.
    ///
    /// # Safety
    /// `list` is open, with the focus root signature set.
    pub(super) unsafe fn bind(&self, list: &ID3D12GraphicsCommandList) {
        let constants = self.constants();
        // SAFETY: Both buffers are live and in COMMON between conversions; the constants
        // outlive the call that copies them into the list.
        unsafe {
            let mut barriers = [
                transition(
                    &self.ages,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ),
                transition(
                    &self.importances,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ),
            ];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.SetComputeRootUnorderedAccessView(3, self.ages.GetGPUVirtualAddress());
            list.SetComputeRootUnorderedAccessView(4, self.importances.GetGPUVirtualAddress());
            list.SetComputeRoot32BitConstants(5, 16, constants.as_ptr().cast(), 0);
        }
    }
    /// After the conversion's dispatch: the map pass and its copy into the
    /// slot's readback buffer, so the slot's fence covers both.
    ///
    /// # Safety
    /// `list` is open and [`Focus::bind`] recorded into it.
    pub(super) unsafe fn record(&self, list: &ID3D12GraphicsCommandList, slot: usize) {
        // SAFETY: The buffers are live, in the states named, and the copy fits both.
        unsafe {
            list.ResourceBarrier(&[uav_barrier()]);
            list.SetPipelineState(&self.map);
            list.Dispatch(self.grid.0.div_ceil(8), self.grid.1.div_ceil(8), 1);
            let mut barriers = [
                transition(
                    &self.ages,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COMMON,
                ),
                transition(
                    &self.importances,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                ),
            ];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.CopyBufferRegion(
                &self.readback[slot].0,
                0,
                &self.importances,
                0,
                self.map_bytes(),
            );
            let mut barriers = [transition(
                &self.importances,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_COMMON,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
        }
    }
    /// `slot`'s list is recorded and about to run.
    pub(super) fn recorded(&mut self, slot: usize) {
        self.written[slot] = self.settings.is_some();
        self.reset &= !self.written[slot];
    }
    pub(super) fn retain(&self, work: &mut RetainedWork) {
        work.keep(&self.root);
        work.keep(&self.convert);
        work.keep(&self.map);
        work.keep(&self.ages);
        work.keep(&self.importances);
        for (buffer, _) in &self.readback {
            work.keep(buffer);
        }
    }
    pub(super) fn unmap(&self) {
        for (buffer, _) in &self.readback {
            // SAFETY: Each buffer was mapped once in `enable_focus`; nothing reads it after this.
            unsafe { buffer.Unmap(0, None) };
        }
    }
}
impl Converter {
    /// Make the encoder focus passes for codec blocks of `block` (16 or 64)
    /// pixels. Off until [`Converter::set_focus`].
    pub fn enable_focus(&mut self, block: u32) -> Result<()> {
        anyhow::ensure!(
            block == 16 || block == 64,
            "encoder focus block {block} is not 16 or 64"
        );
        anyhow::ensure!(
            !self.packs_chroma,
            "encoder focus does not support packed 4:4:4"
        );
        if let Some(focus) = &self.focus {
            anyhow::ensure!(
                focus.block == block,
                "encoder focus already uses {} pixel blocks",
                focus.block
            );
            return Ok(());
        }
        self.compute.ensure_active()?;
        let age_grid = butterpollo_media_policy::encoder_focus::age_grid(self.width, self.height);
        let grid = (self.width.div_ceil(block), self.height.div_ceil(block));
        let device = &self.compute.device;
        let root = root_signature_with(device, 0, self.layout.uav_count(), &focus_parameters())?;
        let convert = pipeline(device, &root, self.layout.focus_entry())?;
        let map = pipeline(device, &root, b"focus_map_cs\0")?;
        let ages = focus_buffer(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            8 * u64::from(age_grid.0) * u64::from(age_grid.1),
            D3D12_RESOURCE_STATE_COMMON,
        )?;
        let map_bytes = 4 * (2 + u64::from(grid.0) * u64::from(grid.1));
        let importances = focus_buffer(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            map_bytes,
            D3D12_RESOURCE_STATE_COMMON,
        )?;
        let mut readback = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            let buffer = focus_buffer(
                device,
                D3D12_HEAP_TYPE_READBACK,
                map_bytes,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )?;
            let mut mapped = std::ptr::null_mut();
            // SAFETY: A READBACK buffer stays mapped for its life; `unmap` ends it in Drop.
            unsafe { buffer.Map(0, None, Some(&mut mapped))? };
            readback.push((buffer, mapped.cast_const().cast::<u32>()));
        }
        self.focus = Some(Box::new(Focus {
            root,
            convert,
            map,
            ages,
            importances,
            readback,
            block,
            age_grid,
            grid,
            settings: None,
            written: [false; SLOTS],
            reset: true,
        }));
        Ok(())
    }
    /// Switch the focus passes on with `settings`, or off with `None`. Ages
    /// restart when focus comes back on, and maps from before a switch are
    /// never returned.
    pub fn set_focus(&mut self, settings: Option<FocusSettings>) -> Result<()> {
        let focus = match (&mut self.focus, settings) {
            (Some(focus), _) => focus,
            (None, None) => return Ok(()),
            (None, Some(_)) => bail!("encoder focus was not enabled on this converter"),
        };
        if focus.settings.is_some() != settings.is_some() {
            focus.written = [false; SLOTS];
            focus.reset = true;
        }
        focus.settings = settings;
        Ok(())
    }
    /// The newest map whose conversion has finished and whose fence value is
    /// above `newer_than`. Only asks the fence: never waits.
    pub fn latest_map(&self, newer_than: u64) -> Option<MapView<'_>> {
        let focus = self.focus.as_deref()?;
        let (slot, value) = (0..SLOTS)
            .filter(|&slot| focus.written[slot])
            .map(|slot| (slot, self.slots[slot].fence))
            .filter(|&(_, value)| {
                value != u64::MAX && value > newer_than && self.compute.completed(value)
            })
            .max_by_key(|&(_, value)| value)?;
        let (width, height) = focus.grid;
        // SAFETY: The slot's copy has completed and the next one needs `&mut self`
        // (`convert`), so the mapped words stay unchanged while this borrow lives.
        let words = unsafe {
            std::slice::from_raw_parts(focus.readback[slot].1, 2 + width as usize * height as usize)
        };
        Some(MapView {
            value,
            uniform: words[0] >= words[1],
            rows: &words[2..],
            width,
            height,
        })
    }
}
