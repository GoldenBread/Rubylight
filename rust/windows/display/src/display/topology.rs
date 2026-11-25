//! Display identities, topology queries, and desktop route changes.

use super::modes::{apply_mode_with, timing_matches};
pub use super::monitor::Monitor;
use super::*;

mod identity;
mod routes;

pub use identity::edid_refresh;
use identity::monitor_id;
pub(super) use identity::{Route, is_physical_monitor, is_rubylight_monitor, is_virtual_monitor};
pub(super) use routes::activate_target;
#[cfg(test)]
use routes::{
    activation_paths, extended_paths, extended_paths_preferred, keep_active_modes, moving_targets,
};

pub struct Topology {
    pub(super) paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    pub(super) modes: Vec<DISPLAYCONFIG_MODE_INFO>,
}
impl Topology {
    pub(super) fn rotations(&self) -> std::collections::BTreeMap<String, u32> {
        self.rotations_of(&self.basic_monitors())
    }
    /// [`Self::rotations`] for monitors already listed, which cost a DXGI enumeration.
    pub(super) fn rotations_of(
        &self,
        monitors: &[Monitor],
    ) -> std::collections::BTreeMap<String, u32> {
        monitors
            .iter()
            .filter_map(|m| {
                self.paths
                    .iter()
                    .find(|p| p.targetInfo.adapterId == m.adapter && p.targetInfo.id == m.target)
                    .map(|p| (m.device_id.clone(), p.targetInfo.rotation.0 as u32))
            })
            .collect()
    }
    pub(super) fn set_rotations(
        &mut self,
        rotations: &std::collections::BTreeMap<String, u32>,
    ) -> Result<()> {
        let monitors = self.basic_monitors();
        let mut changed = false;
        for monitor in monitors {
            if let Some(rotation) = rotations.get(&monitor.device_id) {
                if !(1..=4).contains(rotation) {
                    bail!("invalid saved display rotation");
                }
                if let Some(path) = self.paths.iter_mut().find(|p| {
                    p.targetInfo.adapterId == monitor.adapter && p.targetInfo.id == monitor.target
                }) && path.targetInfo.rotation.0 != *rotation as i32
                {
                    let swap = (path.targetInfo.rotation.0 - *rotation as i32).abs() % 2 != 0;
                    if swap
                        && let Some(mode) = self
                            .modes
                            // SAFETY: The path was queried without virtual-mode awareness, so modeInfoIdx is the active union field.
                            .get_mut(unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize)
                        && mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE
                    {
                        // SAFETY: The mode type was checked as SOURCE before accessing its sourceMode fields.
                        unsafe {
                            std::mem::swap(
                                &mut mode.Anonymous.sourceMode.width,
                                &mut mode.Anonymous.sourceMode.height,
                            );
                        }
                    }
                    path.targetInfo.rotation = DISPLAYCONFIG_ROTATION(*rotation as i32);
                    path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
                    changed = true;
                }
            }
        }
        if changed {
            self.restore()?;
            super::end_trace::committed("rotation", false);
        }
        Ok(())
    }
    pub fn nodes(&self) -> Result<Vec<butterpollo_media_policy::topology::Node>> {
        self.nodes_of(self.basic_monitors())
    }
    /// [`Self::nodes`] for monitors already listed, which cost a DXGI enumeration.
    pub(super) fn nodes_of(
        &self,
        monitors: Vec<Monitor>,
    ) -> Result<Vec<butterpollo_media_policy::topology::Node>> {
        use butterpollo_media_policy::topology::{Kind, Mode, Node, Position};
        let listed = monitors.clone();
        monitors
            .into_iter()
            .map(|m| {
                let mode = mode(&m.display_name)?;
                let refresh = self.refresh_in(&listed, &m.device_id)?;
                // SAFETY: mode() returns a display DEVMODEW, whose display position union field is initialized.
                let position = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
                Ok(Node {
                    id: m.device_id.clone(),
                    label: m.friendly_name,
                    kind: Kind::Physical,
                    active: true,
                    primary: m.primary,
                    device_id: m.device_id,
                    desired_position: Position {
                        x: position.x,
                        y: position.y,
                    },
                    mode: Mode {
                        width: mode.dmPelsWidth,
                        height: mode.dmPelsHeight,
                        refresh_hz: f64::from(refresh.0) / 1000.0,
                    },
                })
            })
            .collect()
    }
    pub fn set_positions(
        &mut self,
        positions: &std::collections::BTreeMap<
            String,
            butterpollo_media_policy::topology::Position,
        >,
    ) -> Result<()> {
        let monitors = self.basic_monitors();
        let mut moved = false;
        let mut placed = Vec::new();
        for monitor in monitors {
            let Some(position) = positions.get(&monitor.device_id) else {
                continue;
            };
            for path in &self.paths {
                if path.targetInfo.adapterId == monitor.adapter
                    && path.targetInfo.id == monitor.target
                {
                    // SAFETY: The path was queried without virtual-mode awareness, so modeInfoIdx is the active union field.
                    let index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx };
                    let source = self
                        .modes
                        .get_mut(index as usize)
                        .context("display source mode unavailable")?;
                    if source.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
                        bail!("invalid display source mode");
                    }
                    let next = POINTL {
                        x: position.x,
                        y: position.y,
                    };
                    // SAFETY: The mode type was checked as SOURCE before reading its sourceMode position.
                    let current = unsafe { source.Anonymous.sourceMode.position };
                    placed.push((monitor.device_id.clone(), next.x, next.y));
                    if (current.x, current.y) != (next.x, next.y) {
                        source.Anonymous.sourceMode.position = next;
                        moved = true;
                    }
                }
            }
        }
        // Re-applying even an unchanged configuration lets Windows renegotiate
        // display timings: a TV at 1080p120 HDR came back at 60 Hz.
        if !moved {
            return Ok(());
        }
        self.restore()
            .with_context(|| format!("placing {placed:?}"))?;
        super::end_trace::committed("positions", false);
        Ok(())
    }
    pub fn query() -> Result<Self> {
        Self::query_with_flags(QDC_ONLY_ACTIVE_PATHS)
    }
    pub fn query_all() -> Result<Self> {
        Self::query_with_flags(QDC_ALL_PATHS)
    }
    fn query_with_flags(flags: QUERY_DISPLAY_CONFIG_FLAGS) -> Result<Self> {
        Self::query_with(
            // SAFETY: Both counts are live writable references for this call.
            |np, nm| unsafe { GetDisplayConfigBufferSizes(flags, np, nm) },
            // SAFETY: The initialized buffers match the supplied counts and
            // remain live through this synchronous call.
            |np, paths, nm, modes| unsafe {
                QueryDisplayConfig(flags, np, paths.as_mut_ptr(), nm, modes.as_mut_ptr(), None)
            },
        )
    }
    fn query_with(
        mut sizes: impl FnMut(&mut u32, &mut u32) -> WIN32_ERROR,
        mut query: impl FnMut(
            &mut u32,
            &mut [DISPLAYCONFIG_PATH_INFO],
            &mut u32,
            &mut [DISPLAYCONFIG_MODE_INFO],
        ) -> WIN32_ERROR,
    ) -> Result<Self> {
        for _ in 0..8 {
            let (mut np, mut nm) = (0, 0);
            sizes(&mut np, &mut nm).ok()?;
            // QDC_ALL_PATHS includes every source/target combination, not
            // just attached monitors. VDDs can legitimately exceed 256.
            if np > 16384 || nm > 32768 {
                bail!("display configuration exceeds the limit: {np} paths, {nm} modes");
            }
            // A headless console has no paths before its virtual display is
            // created. QueryDisplayConfig requires real array buffers.
            if np == 0 {
                return Ok(Self {
                    paths: Vec::new(),
                    modes: Vec::new(),
                });
            }
            nm = nm.max(1);
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
            let result = query(&mut np, &mut paths, &mut nm, &mut modes);
            if result == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            result.ok()?;
            paths.truncate(np as usize);
            modes.truncate(nm as usize);
            return Ok(Self { paths, modes });
        }
        bail!("display topology is changing")
    }
    pub fn restore(&self) -> Result<()> {
        let (paths, modes) = super::hotplug::referenced_modes(&self.paths, &self.modes)?;
        let modes = (!modes.is_empty()).then_some(modes.as_slice());
        // SAFETY: The owned path and compacted mode slices remain valid for this synchronous display configuration call.
        unsafe {
            check(SetDisplayConfig(
                Some(&paths),
                modes,
                SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
            ))
        }
    }
    /// Save the active layout to the Windows display database for the
    /// connected displays, as the layout Windows recalls for them. Windows
    /// recalls it when an exclusive-fullscreen game loses focus; a layout that
    /// is only applied then loses to the saved one. The modes are the current
    /// ones, so nothing is switched.
    pub fn save_current() -> Result<()> {
        let current = Self::query()?;
        // SAFETY: The owned path and mode slices remain valid for this synchronous display configuration call.
        unsafe {
            check(SetDisplayConfig(
                Some(&current.paths),
                Some(&current.modes),
                SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_SAVE_TO_DATABASE,
            ))
        }
    }
    pub fn refresh(&self, id: &str) -> Result<butterpollo_media_policy::framegen::Rate> {
        self.refresh_in(&self.basic_monitors(), id)
    }
    /// [`Self::refresh`] among monitors already listed, which cost a DXGI enumeration.
    fn refresh_in(
        &self,
        monitors: &[Monitor],
        id: &str,
    ) -> Result<butterpollo_media_policy::framegen::Rate> {
        let monitor = monitors
            .iter()
            .find(|m| m.device_id == id)
            .context("display unavailable")?;
        let path = self
            .paths
            .iter()
            .find(|p| {
                p.targetInfo.adapterId == monitor.adapter && p.targetInfo.id == monitor.target
            })
            .context("display path unavailable")?;
        let rate = path.targetInfo.refreshRate;
        if rate.Denominator == 0 {
            bail!("display refresh has no denominator");
        }
        Ok(butterpollo_media_policy::framegen::Rate(u32::try_from(
            (u64::from(rate.Numerator) * 1000 + u64::from(rate.Denominator) / 2)
                / u64::from(rate.Denominator),
        )?))
    }
    pub fn set_mode_rate(
        name: &str,
        width: u32,
        height: u32,
        rate: butterpollo_media_policy::framegen::Rate,
    ) -> Result<()> {
        if width == 0 || height == 0 || rate.0 == 0 {
            bail!("invalid display mode");
        }
        let mut topology = Self::query()?;
        let monitor = topology
            .basic_monitors()
            .into_iter()
            .find(|m| m.matches(name))
            .context("display unavailable")?;
        // Windows chooses the target timing itself, even for the mode a display
        // already has, so a display already in this mode is left alone.
        let applied = || -> Result<bool> {
            let current = mode(&monitor.display_name)?;
            let actual = Self::query()?.refresh(&monitor.device_id)?;
            Ok(timing_matches(
                (width, height, rate.0),
                (current.dmPelsWidth, current.dmPelsHeight, actual.0),
            ))
        };
        if applied()? {
            return Ok(());
        }
        let (num, den) = rate.rational();
        for path in &mut topology.paths {
            if path.targetInfo.adapterId == monitor.adapter && path.targetInfo.id == monitor.target
            {
                // SAFETY: The path was queried without virtual-mode awareness, so modeInfoIdx is the active union field.
                let index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
                let source = topology
                    .modes
                    .get_mut(index)
                    .context("display source mode unavailable")?;
                if source.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
                    bail!("invalid source mode");
                }
                source.Anonymous.sourceMode.width = width;
                source.Anonymous.sourceMode.height = height;
                path.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL {
                    Numerator: num,
                    Denominator: den,
                };
                path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            }
        }
        topology.restore()?;
        super::end_trace::committed("mode", false);
        if applied()? {
            return Ok(());
        }
        // Windows reported success with another refresh rate: a TV whose native
        // timing is 4K60 turns 1080p120 into 60 Hz, and a 240 Hz monitor can
        // fall back to its 120 Hz default. The mode list applies the rate.
        let hz = rate.0.saturating_add(500) / 1000;
        tracing::debug!(output = %monitor.display_name, hz, "display rate applied through the mode list");
        let mut next = mode(&monitor.display_name)?;
        next.dmPelsWidth = width;
        next.dmPelsHeight = height;
        next.dmDisplayFrequency = hz;
        next.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
        apply_mode_with(&monitor.display_name, &next, CDS_TYPE(0))?;
        super::end_trace::committed("mode from the mode list", false);
        if !applied()? {
            let current = mode(&monitor.display_name)?;
            bail!(
                "display kept {}x{}@{} instead of {width}x{height}@{:.3}",
                current.dmPelsWidth,
                current.dmPelsHeight,
                current.dmDisplayFrequency,
                f64::from(rate.0) / 1000.
            );
        }
        Ok(())
    }
    pub fn monitors(&self) -> Vec<Monitor> {
        self.monitors_filtered(false, true)
    }
    /// Monitors without per-monitor HDR/WCG colour queries, for identity/layout callers.
    pub(crate) fn basic_monitors(&self) -> Vec<Monitor> {
        self.monitors_filtered(false, false)
    }
    /// Connected monitors only (`targetAvailable`), filtering out disconnected
    /// cached routes returned by [`Self::query_all`].
    pub fn available_monitors(&self) -> Vec<Monitor> {
        self.monitors_filtered(true, true)
    }
    /// Connected monitors without colour queries.
    pub(crate) fn available_basic_monitors(&self) -> Vec<Monitor> {
        self.monitors_filtered(true, false)
    }
    fn target_paths(&self, only_available: bool) -> Vec<&DISPLAYCONFIG_PATH_INFO> {
        let mut paths: Vec<_> = self
            .paths
            .iter()
            .filter(|p| !only_available || p.targetInfo.targetAvailable.as_bool())
            .collect();
        paths.sort_by_key(|p| p.flags & DISPLAYCONFIG_PATH_ACTIVE == 0);
        let mut targets = std::collections::BTreeSet::new();
        paths
            .into_iter()
            .filter(|p| {
                targets.insert((
                    p.targetInfo.adapterId.LowPart,
                    p.targetInfo.adapterId.HighPart,
                    p.targetInfo.id,
                ))
            })
            .collect()
    }
    fn monitors_filtered(&self, only_available: bool, with_color: bool) -> Vec<Monitor> {
        let primary = crate::desktop::primary_display_name();
        self.target_paths(only_available)
            .into_iter()
            // SAFETY: Both name structures have matching header types and sizes and remain live throughout the queries.
            .filter_map(|p| unsafe {
                let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                    header: header(
                        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                        size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
                        p.sourceInfo.adapterId,
                        p.sourceInfo.id,
                    ),
                    ..Default::default()
                };
                let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                    header: header(
                        DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                        size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
                        p.targetInfo.adapterId,
                        p.targetInfo.id,
                    ),
                    ..Default::default()
                };
                if DisplayConfigGetDeviceInfo(&mut source.header) != 0
                    || DisplayConfigGetDeviceInfo(&mut target.header) != 0
                {
                    return None;
                }
                let color = if with_color {
                    color_state::query(p.targetInfo.adapterId, p.targetInfo.id).unwrap_or_default()
                } else {
                    Default::default()
                };
                let display_name = wide(&source.viewGdiDeviceName);
                let monitor_device_path = wide(&target.monitorDevicePath);
                Some(Monitor {
                    device_id: monitor_id(&monitor_device_path),
                    monitor_device_path,
                    friendly_name: wide(&target.monitorFriendlyDeviceName),
                    primary: primary.as_ref() == Some(&display_name),
                    display_name,
                    hdr_supported: color.supported,
                    hdr_enabled: color.enabled,
                    hdr_requested: color.requested,
                    adapter: p.targetInfo.adapterId,
                    target: p.targetInfo.id,
                    source: p.sourceInfo.id,
                })
            })
            .collect()
    }
    /// Whether a path still leads to this monitor, as `monitors` would list
    /// it: the lease heartbeat's check each second, without the DXGI and
    /// colour queries that take nearly all of the half millisecond. `routes`
    /// are the (adapter, target) pairs the display goes by: Windows can list
    /// it under another pair while it is off than while it is on.
    pub(super) fn shows(
        &self,
        routes: &[Route],
        monitor: &Monitor,
        mut device_info: impl FnMut(&mut DISPLAYCONFIG_DEVICE_INFO_HEADER) -> i32,
    ) -> Result<bool> {
        for p in self.paths.iter().filter(|p| {
            routes.iter().any(|(adapter, target)| {
                p.targetInfo.adapterId == *adapter && p.targetInfo.id == *target
            })
        }) {
            // Only the target's name says which monitor this is; the source's GDI name
            // is not used here, so it is not asked for (one query less each second).
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                header: header(
                    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                    size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
                    p.targetInfo.adapterId,
                    p.targetInfo.id,
                ),
                ..Default::default()
            };
            check(device_info(&mut target.header))?;
            if wide(&target.monitorDevicePath).eq_ignore_ascii_case(&monitor.monitor_device_path) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
pub fn monitors() -> Result<Vec<Monitor>> {
    Ok(Topology::query()?.monitors())
}
/// Active monitors without HDR/WCG colour queries.
pub(crate) fn basic_monitors() -> Result<Vec<Monitor>> {
    Ok(Topology::query()?.basic_monitors())
}
/// How many displays Windows shows now, from the display configuration alone (no DXGI or
/// colour query, which take nearly all of `monitors`' half a millisecond). A display that
/// arrived or left changes it, which is when Windows rearranges the desktop and can move the
/// primary by itself; the display heartbeat asks for a stream whose layout promised the
/// primary.
pub fn active_displays() -> Result<usize> {
    Ok(Topology::query()?.paths.len())
}
/// Whether `hint` names the display Windows shows as the primary one: the primary's desktop
/// sits at the origin, whichever display that is now. Read from the display configuration
/// alone (no DXGI or colour query, which take nearly all of `monitors`' half a millisecond),
/// so the display heartbeat can ask four times a second for a stream whose layout promised
/// the primary. `None` when no active display matches `hint`: it is off, or Windows is still
/// laying it out, and neither says where the primary went.
pub fn shown_primary(hint: &str) -> Result<Option<bool>> {
    let topology = Topology::query()?;
    let mut named = false;
    for path in &topology.paths {
        // SAFETY: Both name structures have the matching header type and size and remain
        // live through the query.
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: header(
                DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
                path.sourceInfo.adapterId,
                path.sourceInfo.id,
            ),
            ..Default::default()
        };
        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: header(
                DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
                path.targetInfo.adapterId,
                path.targetInfo.id,
            ),
            ..Default::default()
        };
        // SAFETY: The structures are live locals with matching headers for these synchronous
        // queries, which only fill them in.
        let read = unsafe {
            DisplayConfigGetDeviceInfo(&mut source.header) == 0
                && DisplayConfigGetDeviceInfo(&mut target.header) == 0
        };
        if !read {
            continue;
        }
        let display_name = wide(&source.viewGdiDeviceName);
        let monitor_device_path = wide(&target.monitorDevicePath);
        let device_id = monitor_id(&monitor_device_path);
        if ![
            display_name.as_str(),
            monitor_device_path.as_str(),
            device_id.as_str(),
        ]
        .iter()
        .any(|value| value.eq_ignore_ascii_case(hint))
        {
            continue;
        }
        named = true;
        // SAFETY: The path was queried without virtual-mode awareness, so modeInfoIdx is
        // the active union field.
        let index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx };
        let Some(mode) = topology.modes.get(index as usize) else {
            continue;
        };
        if mode.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
            continue;
        }
        // SAFETY: The mode type was checked as SOURCE before reading its sourceMode position.
        let position = unsafe { mode.Anonymous.sourceMode.position };
        if position.x == 0 && position.y == 0 {
            return Ok(Some(true));
        }
    }
    Ok(named.then_some(false))
}
impl Topology {
    /// The device ids `monitors` gives, from the source and target names alone, and whether
    /// every name could be read: `monitors` leaves out a path whose name it cannot read.
    pub(super) fn device_ids(
        &self,
        mut device_info: impl FnMut(&mut DISPLAYCONFIG_DEVICE_INFO_HEADER) -> i32,
    ) -> (Vec<String>, bool) {
        let mut targets = std::collections::BTreeSet::new();
        let mut complete = true;
        let ids = self
            .paths
            .iter()
            .filter(|p| {
                targets.insert((
                    p.targetInfo.adapterId.LowPart,
                    p.targetInfo.adapterId.HighPart,
                    p.targetInfo.id,
                ))
            })
            .filter_map(|p| {
                let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                    header: header(
                        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                        size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
                        p.sourceInfo.adapterId,
                        p.sourceInfo.id,
                    ),
                    ..Default::default()
                };
                let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                    header: header(
                        DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                        size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
                        p.targetInfo.adapterId,
                        p.targetInfo.id,
                    ),
                    ..Default::default()
                };
                let read =
                    device_info(&mut source.header) == 0 && device_info(&mut target.header) == 0;
                complete &= read;
                read.then(|| monitor_id(&wide(&target.monitorDevicePath)))
            })
            .collect();
        (ids, complete)
    }
}
/// A lookup kept while what it was made from stays the same.
pub(super) struct Kept<K, V> {
    key: K,
    at: Instant,
    value: V,
}
/// The value for `key`: the one `kept` while its key is the same and it is younger than
/// `max_age`, else what `lookup` gives, kept in its place when it says it is whole. A failed
/// or partial lookup keeps nothing: the next call looks again.
pub(super) fn kept_or<K: PartialEq, V: Clone>(
    kept: &mut Option<Kept<K, V>>,
    key: K,
    now: Instant,
    max_age: Duration,
    lookup: impl FnOnce() -> Result<(V, bool)>,
) -> Result<V> {
    if let Some(kept) = kept
        .as_ref()
        .filter(|kept| kept.key == key && now.saturating_duration_since(kept.at) < max_age)
    {
        return Ok(kept.value.clone());
    }
    let (value, whole) = lookup()?;
    *kept = whole.then(|| Kept {
        key,
        at: now,
        value: value.clone(),
    });
    Ok(value)
}
/// How long [`active_device_ids`] goes on with the names it read while the active paths stay
/// the same.
const ACTIVE_IDS_KEPT: Duration = Duration::from_secs(5);
/// The device ids of the monitors Windows shows now, as [`monitors`] gives them, for the
/// display heartbeat, which asks four times a second for each stream. Only the active paths
/// are read each time; their names are read again when the paths change, and at least every
/// [`ACTIVE_IDS_KEPT`]. No DXGI or colour query, which take nearly all of `monitors`' half
/// millisecond.
pub fn active_device_ids() -> Result<Vec<String>> {
    type Routes = Vec<(u32, i32, u32, u32)>;
    static KEPT: std::sync::Mutex<Option<Kept<Routes, Vec<String>>>> = std::sync::Mutex::new(None);
    let topology = Topology::query()?;
    let routes: Routes = topology
        .paths
        .iter()
        .map(|p| {
            (
                p.targetInfo.adapterId.LowPart,
                p.targetInfo.adapterId.HighPart,
                p.targetInfo.id,
                p.sourceInfo.id,
            )
        })
        .collect();
    kept_or(
        &mut KEPT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        routes,
        Instant::now(),
        ACTIVE_IDS_KEPT,
        || {
            // SAFETY: Topology::device_ids supplies a live source- or target-name structure with a matching header type and size.
            Ok(topology.device_ids(|header| unsafe { DisplayConfigGetDeviceInfo(header) }))
        },
    )
}

#[cfg(test)]
mod tests;
