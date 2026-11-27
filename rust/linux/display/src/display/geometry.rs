//! Where each connected display sits on the desktop, for the console's read-only display map.
//! A Linux host cannot read a display's place or mode yet ([`super::mode`]), so every display
//! is listed without one, as Windows lists a display it cannot read; the map then draws none.
use super::Monitor;
use serde::Serialize;

/// A display's place and mode on the desktop, in desktop pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// The current refresh rate in Hz.
    pub refresh: u32,
    /// The display's scale in percent; left out when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
}

/// A display with its place on the desktop, as `/api/display-devices` lists it.
#[derive(Serialize)]
pub struct Placed<'a> {
    #[serde(flatten)]
    pub monitor: &'a Monitor,
    /// Left out while the display's place cannot be read.
    #[serde(flatten)]
    pub geometry: Option<Geometry>,
    /// A Rubylight virtual display rather than a physical one.
    #[serde(rename = "virtual")]
    pub is_virtual: bool,
}

/// The display's current place and mode: not readable on Linux yet.
pub fn geometry(_monitor: &Monitor) -> Option<Geometry> {
    None
}

/// Every display with its place on the desktop.
pub fn placed(monitors: &[Monitor]) -> Vec<Placed<'_>> {
    monitors
        .iter()
        .map(|monitor| Placed {
            monitor,
            geometry: geometry(monitor),
            is_virtual: monitor.is_rubylight(),
        })
        .collect()
}
