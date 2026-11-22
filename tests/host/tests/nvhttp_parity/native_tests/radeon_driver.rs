//! Radeon fake driver state struct extracted to keep `radeon.rs` within its line ratchet.
use super::Driven;
use butterpollo_win_graphics::radeon::GpuInfo;

#[derive(Default)]
pub(super) struct Driver {
    pub(super) settings: Vec<Driven>,
    pub(super) gpus: Vec<GpuInfo>,
    pub(super) writes: Vec<String>,
    pub(super) reads: usize,
    /// A write of this id fails.
    pub(super) refuse: Option<&'static str>,
    /// A write of this id succeeds and changes nothing.
    pub(super) ignore: Option<&'static str>,
    /// Every write fails.
    pub(super) refuse_all: bool,
    pub(super) unreadable: bool,
    /// Reads wait until this clears (a driver call that takes its time).
    pub(super) hold: bool,
}
