//! HDR requests and restoration on the currently connected display.

use super::*;

pub fn set_hdr(m: &Monitor, enabled: bool) -> Result<()> {
    if enabled && !m.hdr_supported {
        bail!("selected display does not support HDR");
    }
    color_state::set(m.adapter, m.target, enabled)
}

/// Target `(adapter, target)` for reading SDR white level on `output`, only
/// while Windows reports HDR enabled on that monitor.
fn hdr_white_target<'a>(monitors: &'a [Monitor], output: &str) -> Option<&'a Monitor> {
    let monitor = if output.is_empty() {
        monitors.iter().find(|m| m.primary)
    } else {
        monitors.iter().find(|m| m.matches(output))
    }?;
    monitor.hdr_enabled.then_some(monitor)
}

/// Windows' "SDR content brightness" in nits for `output` (or the primary when
/// `output` is empty), when the display is currently running in HDR.
pub fn sdr_white_nits(output: &str) -> Option<f32> {
    let topology = Topology::query().ok()?;
    let monitors = topology.monitors();
    let monitor = hdr_white_target(&monitors, output)?;
    color_state::sdr_white_nits(monitor.adapter, monitor.target)
}
/// What a stream's HDR request needs from a display. One without HDR is
/// left alone and the stream continues in SDR, as libdisplaydevice leaves
/// a display without an HDR state: the launch must not fail on it.
#[derive(Debug, PartialEq)]
pub(super) enum HdrAction {
    Keep,
    Skip,
    Set(bool),
}
pub(super) fn hdr_action(supported: bool, current: bool, requested: Option<bool>) -> HdrAction {
    match requested {
        Some(enabled) if enabled != current => {
            if enabled && !supported {
                HdrAction::Skip
            } else {
                HdrAction::Set(enabled)
            }
        }
        _ => HdrAction::Keep,
    }
}
/// Undo a stream's HDR change on the display as it is connected now, if it
/// still has the value the stream applied. Its adapter and target ids can
/// change while streaming (a driver reset, a TV that reconnects), so the
/// monitor recorded at stream start only names the device.
pub(super) fn restore_hdr(
    recorded: &Monitor,
    previous: bool,
    applied: bool,
    now: &[Monitor],
    set: impl FnOnce(&Monitor, bool) -> Result<()>,
) -> Result<()> {
    match now
        .iter()
        .find(|m| m.device_id == recorded.device_id && m.hdr_enabled == applied)
    {
        Some(monitor) => {
            tracing::info!(
                output = %monitor.display_name,
                previous,
                applied,
                "restoring HDR after a stream"
            );
            set(monitor, previous)
        }
        None => {
            let current = now
                .iter()
                .find(|m| m.device_id == recorded.device_id)
                .map(|m| m.hdr_enabled);
            tracing::info!(
                output = %recorded.display_name,
                previous,
                applied,
                current = ?current,
                "leaving HDR as it is; the display is gone or no longer has the value the stream applied"
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::monitor;
    use super::*;
    #[test]
    fn hdr_on_a_display_without_hdr_is_skipped_rather_than_failing_the_launch() {
        assert_eq!(hdr_action(false, false, Some(true)), HdrAction::Skip);
        assert_eq!(hdr_action(true, false, Some(true)), HdrAction::Set(true));
        assert_eq!(hdr_action(true, true, Some(false)), HdrAction::Set(false));
        assert_eq!(hdr_action(false, true, Some(false)), HdrAction::Set(false));
        assert_eq!(hdr_action(false, false, Some(false)), HdrAction::Keep);
        assert_eq!(hdr_action(true, true, Some(true)), HdrAction::Keep);
        assert_eq!(hdr_action(true, false, None), HdrAction::Keep);
    }
    #[test]
    fn hdr_is_restored_on_the_display_as_it_is_connected_now() {
        // Recorded at stream start; the adapter's LUID changed since.
        let recorded = monitor("tv", 1, false);
        let mut set = None;
        restore_hdr(
            &recorded,
            false,
            true,
            &[monitor("tv", 2, true)],
            |m, enabled| {
                set = Some((m.adapter.LowPart, enabled));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(set, Some((2, false)));
        // Changed back by the user, or gone: left alone.
        let untouched = |_: &Monitor, _| -> Result<()> { panic!("must not change HDR") };
        restore_hdr(
            &recorded,
            false,
            true,
            &[monitor("tv", 2, false)],
            untouched,
        )
        .unwrap();
        restore_hdr(&recorded, false, true, &[monitor("pc", 2, true)], untouched).unwrap();
        restore_hdr(&recorded, false, true, &[], untouched).unwrap();
    }

    #[test]
    fn hdr_white_target_matches_named_or_primary_hdr_monitor_only() {
        let mut primary_hdr = monitor("pc", 1, true);
        primary_hdr.primary = true;
        let mut secondary_sdr = monitor("tv", 2, false);
        secondary_sdr.primary = false;
        let monitors = [primary_hdr.clone(), secondary_sdr];

        assert_eq!(
            hdr_white_target(&monitors, "").map(|m| m.device_id.as_str()),
            Some("pc")
        );
        assert_eq!(
            hdr_white_target(&monitors, "pc").map(|m| m.device_id.as_str()),
            Some("pc")
        );
        assert_eq!(
            hdr_white_target(&monitors, r"\\.\DISPLAY1").map(|m| m.device_id.as_str()),
            Some("pc")
        );
        // Monitor without HDR active or missing named output must return None, not primary.
        assert!(hdr_white_target(&monitors, "tv").is_none());
        assert!(hdr_white_target(&monitors, "missing").is_none());
    }
}
