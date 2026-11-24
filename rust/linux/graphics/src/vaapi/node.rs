//! The GPUs of this PC as DRM render nodes: which vendor and PCI slot each is, how much video
//! memory it has and what it is called. Streams encode on AMD and NVIDIA GPUs only, so
//! only those are listed.
use super::display::Va;
use std::{path::Path, sync::OnceLock};

pub const AMD: u32 = 0x1002;
pub const NVIDIA: u32 = 0x10de;

/// One GPU with a render node.
#[derive(Clone, Debug)]
pub struct Node {
    /// `/dev/dri/renderD128` and the like.
    pub path: String,
    /// The render node's minor number, which tells two GPUs apart for as long as the system
    /// runs.
    pub minor: u32,
    pub vendor: u32,
    pub device: u32,
    /// The PCI slot, such as `0000:03:00.0`.
    pub slot: String,
    /// Video memory in bytes; 0 where the kernel driver does not say.
    pub memory: u64,
    pub name: String,
}

fn read_hex(path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

/// The GPU's marketing name from the VA driver's vendor string, which Mesa words as
/// "Mesa Gallium driver 26.3.1 for AMD Radeon RX 7800 XT (radeonsi, navi32, ...)"; the whole
/// string when it has no such part.
fn name_from(vendor_string: &str) -> String {
    let text = vendor_string.trim();
    text.split_once(" for ")
        .map(|(_, rest)| rest.split(" (").next().unwrap_or(rest).trim())
        .filter(|name| !name.is_empty())
        .unwrap_or(text)
        .to_string()
}

fn scan() -> Vec<Node> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut nodes = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let Some(minor) = file_name
            .strip_prefix("renderD")
            .and_then(|minor| minor.parse::<u32>().ok())
        else {
            continue;
        };
        let device_dir = entry.path().join("device");
        let (Some(vendor), Some(device)) = (
            read_hex(&device_dir.join("vendor")),
            read_hex(&device_dir.join("device")),
        ) else {
            continue;
        };
        if vendor != AMD && vendor != NVIDIA {
            continue;
        }
        let slot = std::fs::canonicalize(&device_dir)
            .ok()
            .and_then(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let memory = std::fs::read_to_string(device_dir.join("mem_info_vram_total"))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0);
        let path = format!("/dev/dri/{file_name}");
        let name = Va::open(&path)
            .map(|va| name_from(&va.vendor()))
            .unwrap_or_else(|_| format!("GPU {vendor:04x}:{device:04x}"));
        nodes.push(Node {
            path,
            minor,
            vendor,
            device,
            slot,
            memory,
            name,
        });
    }
    nodes.sort_by_key(|node| node.minor);
    nodes
}

/// The AMD and NVIDIA GPUs with a render node, found once per process.
pub fn nodes() -> &'static [Node] {
    static NODES: OnceLock<Vec<Node>> = OnceLock::new();
    NODES.get_or_init(scan)
}

#[cfg(test)]
mod tests {
    use super::name_from;

    #[test]
    fn names_come_out_of_the_driver_string() {
        assert_eq!(
            name_from(
                "Mesa Gallium driver 26.3.1 for AMD Radeon RX 7800 XT (radeonsi, navi32, LLVM 22.1.0, DRM 3.64)"
            ),
            "AMD Radeon RX 7800 XT"
        );
        assert_eq!(name_from("Intel iHD driver"), "Intel iHD driver");
    }
}
