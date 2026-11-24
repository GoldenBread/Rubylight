//! Ignored hardware integration tests for live native virtual display recovery.

use super::super::*;
use crate::display::modes::timing_matches;
#[test]
#[ignore = "creates a virtual display and briefly switches it off"]
fn native_virtual_display_left_off_by_windows_is_switched_on_without_retiming_others() -> Result<()>
{
    let mut display = VirtualDisplay::create(
        &format!("activation-test-{}", std::process::id()),
        1280,
        720,
        60,
    )?;
    let owned = display
        .resolved_target
        .clone()
        .context("virtual display unresolved")?;
    let routes = display.routes()?;
    let ours = |m: &Monitor| routes.contains(&(m.adapter, m.target));
    // Device, width, height, refresh and desktop position.
    type Timing = (String, u32, u32, u32, i32, i32);
    let timings = || -> Result<Vec<Timing>> {
        monitors()?
            .iter()
            .filter(|m| !ours(m))
            .map(|m| {
                let mode = mode(&m.display_name)?;
                // SAFETY: mode() returns a display DEVMODEW with its display position union field initialized.
                let position = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
                Ok((
                    m.device_id.clone(),
                    mode.dmPelsWidth,
                    mode.dmPelsHeight,
                    mode.dmDisplayFrequency,
                    position.x,
                    position.y,
                ))
            })
            .collect()
    };
    let before = timings()?;
    // What Windows does when a saved layout leaves the new display off.
    let active = Topology::query()?;
    let off: Vec<_> = active
        .paths
        .iter()
        .filter(|p| !routes.contains(&(p.targetInfo.adapterId, p.targetInfo.id)))
        .copied()
        .collect();
    // SAFETY: The retained paths and queried modes remain valid for this synchronous display configuration call.
    check(unsafe {
        SetDisplayConfig(
            Some(&off),
            Some(&active.modes),
            SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
        )
    })?;
    anyhow::ensure!(
        !monitors()?.iter().any(ours),
        "virtual display stayed on after switching it off"
    );
    let recovered = display.current_monitor()?;
    assert!(display.owns_monitor(&recovered));
    assert!(display.switched_on);
    assert_eq!(display.generation, 0);
    assert_eq!(timings()?, before);
    // Exercise the established stream's heartbeat separately from startup.
    // SAFETY: The same retained paths and queried modes remain valid here.
    check(unsafe {
        SetDisplayConfig(
            Some(&off),
            Some(&active.modes),
            SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
        )
    })?;
    display.last_feed = Instant::now() - Duration::from_secs(2);
    display.feed()?;
    anyhow::ensure!(monitors()?.iter().any(ours), "virtual display stayed off");
    assert_eq!(display.generation, 1);
    assert!(display.owns_monitor(&owned));
    assert_eq!(timings()?, before);
    println!(
        "switched on {} beside {} displays",
        display.name,
        before.len()
    );
    Ok(())
}
#[test]
#[ignore = "creates and expires owned 4K virtual displays and changes their mode and HDR"]
fn native_virtual_display_recovery_restores_the_requested_mode_and_mode_list() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_ansi(false)
        .try_init();
    let before = Snapshot::capture()?;
    let result = (|| -> Result<()> {
        for rate in [116_000, 1_000_000] {
            let mut display = VirtualDisplay::create_rate(
                &format!("mode-recovery-test-{}-{rate}", std::process::id()),
                3840,
                2160,
                rate,
            )
            .with_context(|| format!("create 4K{}", rate / 1000))?;
            display
                .finish_hotplug("test startup")
                .with_context(|| format!("settle 4K{}", rate / 1000))?;
            let supported = supported_modes(&display.name);
            anyhow::ensure!(supported.contains(&(3840, 2160, rate / 1000)));
            anyhow::ensure!(supported.contains(&(1920, 1080, 60)));
            // The driver's EDID prefers 4K60; creation must set the
            // stream's rate rather than leave Windows' choice.
            let created = current_timing(&display.name)?;
            anyhow::ensure!(
                timing_matches((3840, 2160, rate), created),
                "created at {}",
                format_timing(created)
            );
            Topology::set_mode_rate(
                &display.name,
                2560,
                1440,
                butterpollo_media_policy::framegen::Rate(60_000),
            )?;
            // A healthy heartbeat must allow a game's own mode change.
            display.last_feed = Instant::now() - Duration::from_secs(2);
            display.feed()?;
            anyhow::ensure!(mode(&display.name)?.dmPelsWidth == 2560);
            anyhow::ensure!(display.generation == 0);

            // What a driver restart does: the monitor departs under a live stream.
            display.driver.remove(display.id)?;
            let deadline = Instant::now() + Duration::from_secs(20);
            while display.generation == 0 {
                let recovered = display.feed();
                if Instant::now() >= deadline {
                    recovered?;
                    bail!("owned virtual display did not recover");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let owned = display.hotplug_monitor()?.clone();
            let routes = display.routes()?;
            set_hdr(&owned, true)?;
            display
                .finish_hotplug("test recovery after HDR")
                .with_context(|| format!("finish recovered 4K{}", rate / 1000))?;
            let actual = current_timing(&display.name)?;
            anyhow::ensure!(
                timing_matches((3840, 2160, rate), actual),
                "recovered at {}",
                format_timing(actual)
            );
            let recovered = supported_modes(&display.name);
            anyhow::ensure!(
                supported.iter().all(|mode| recovered.contains(mode)),
                "recovery restricted the mode list"
            );
            println!(
                "recovered {} at 3840x2160@{} with {} modes after a 2560x1440@60 recall",
                display.name,
                rate / 1000,
                recovered.len()
            );
            drop(display);
            let deadline = Instant::now() + Duration::from_secs(10);
            while Topology::query_all()?.paths.iter().any(|path| {
                routes.contains(&(path.targetInfo.adapterId, path.targetInfo.id))
                    && path.targetInfo.targetAvailable.as_bool()
            }) {
                anyhow::ensure!(Instant::now() < deadline, "owned display did not depart");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Ok(())
    })();
    before.restore()?;
    result?;
    anyhow::ensure!(
        serde_json::to_value(Snapshot::capture()?)? == serde_json::to_value(before)?,
        "the pre-test layout was not restored"
    );
    Ok(())
}
