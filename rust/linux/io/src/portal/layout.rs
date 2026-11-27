//! Where the desktop's monitors sit, for placing absolute input and listing displays. The
//! portal tells the monitors it shares; the rest of the layout comes from KDE's
//! `kscreen-doctor -j` when it is installed, and from the kernel's connectors otherwise (sizes
//! only, side by side, which is right for one monitor and a guess for more).

use super::Monitor;
use std::{
    process::Command,
    sync::{Mutex, PoisonError},
};

/// A rectangle of the desktop in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

static NOTED: Mutex<Vec<Monitor>> = Mutex::new(Vec::new());

/// Remembers the monitors a portal session shares, for [`monitors`].
pub fn note(monitors: &[Monitor]) {
    *NOTED.lock().unwrap_or_else(PoisonError::into_inner) = monitors.to_vec();
}

/// The union of `rects`.
pub fn bounds(rects: &[Rect]) -> Option<Rect> {
    let left = rects.iter().map(|r| r.x).min()?;
    let top = rects.iter().map(|r| r.y).min()?;
    let right = rects.iter().map(|r| r.x + r.width).max()?;
    let bottom = rects.iter().map(|r| r.y + r.height).max()?;
    Some(Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

/// The monitors in the order the host numbers them: primary first, then by position.
pub fn monitors() -> Vec<(String, Rect)> {
    if let Some(found) = from_kscreen().filter(|found| !found.is_empty()) {
        return found;
    }
    let noted: Vec<(String, Rect)> = NOTED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|m| m.width > 0 && m.height > 0)
        .map(|m| {
            (
                format!("node {}", m.node),
                Rect {
                    x: m.x,
                    y: m.y,
                    width: m.width,
                    height: m.height,
                },
            )
        })
        .collect();
    if !noted.is_empty() {
        return noted;
    }
    from_sysfs()
}

/// KDE's own list of outputs: `priority` 1 is the primary.
fn from_kscreen() -> Option<Vec<(String, Rect)>> {
    // With no display to ask (the login screen, before anyone signs in), kscreen-doctor aborts
    // and leaves a core dump each time.
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        return None;
    }
    let output = Command::new("kscreen-doctor").arg("-j").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let mut found: Vec<(i64, String, Rect)> = json
        .get("outputs")?
        .as_array()?
        .iter()
        .filter(|o| o["enabled"].as_bool() == Some(true) && o["connected"].as_bool() != Some(false))
        .filter_map(|o| {
            let scale = o["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0);
            // The layout is in logical pixels; a turned output swaps its sides.
            let turned = matches!(o["rotation"].as_i64(), Some(2 | 8));
            let (mut w, mut h) = (o["size"]["width"].as_f64()?, o["size"]["height"].as_f64()?);
            if turned {
                std::mem::swap(&mut w, &mut h);
            }
            Some((
                o["priority"].as_i64().unwrap_or(i64::MAX),
                o["name"].as_str().unwrap_or("").to_owned(),
                Rect {
                    x: o["pos"]["x"].as_i64()? as i32,
                    y: o["pos"]["y"].as_i64()? as i32,
                    width: (w / scale).round() as i32,
                    height: (h / scale).round() as i32,
                },
            ))
        })
        .collect();
    found.sort_by_key(|(priority, _, rect)| (*priority, rect.x, rect.y));
    Some(
        found
            .into_iter()
            .map(|(_, name, rect)| (name, rect))
            .collect(),
    )
}

/// The connected, enabled connectors with their first (preferred) mode, side by side.
fn from_sysfs() -> Vec<(String, Rect)> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut names: Vec<_> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with("card") && name.contains('-'))
        .collect();
    names.sort();
    let mut found = Vec::new();
    let mut x = 0;
    for name in names {
        let dir = std::path::Path::new("/sys/class/drm").join(&name);
        let read = |file: &str| std::fs::read_to_string(dir.join(file)).unwrap_or_default();
        if read("status").trim() != "connected" || read("enabled").trim() != "enabled" {
            continue;
        }
        let mode = read("modes");
        let Some((w, h)) = mode
            .lines()
            .next()
            .and_then(|line| line.split('i').next()?.split_once('x'))
            .and_then(|(w, h)| Some((w.parse::<i32>().ok()?, h.parse::<i32>().ok()?)))
        else {
            continue;
        };
        let label = name.split_once('-').map_or(name.as_str(), |(_, rest)| rest);
        found.push((
            label.to_owned(),
            Rect {
                x,
                y: 0,
                width: w,
                height: h,
            },
        ));
        x += w;
    }
    found
}
