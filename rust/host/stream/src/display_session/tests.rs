//! Unit and lifecycle tests for display session preparation, roaming, pause/resume,
//! scaling, layout learning, and crash-restore recovery.
use super::apply::*;
use super::baseline::*;
use super::lifecycle::*;
use super::mode_report::*;
use super::policy::*;
use super::restore::*;
use super::scale::*;
use super::turn::*;
use super::window_target::*;
use super::*;
use crate::state::Launch;
use anyhow::{Context, Result};
use butterpollo_config::config::Config;
use butterpollo_media_policy::{
    display_policy::Arrangement,
    framegen::{Policy, Rate},
    rtsp::Negotiated,
};
use butterpollo_session::session::{Role, Warnings};
use butterpollo_win_io::limiter;
use std::{
    cell::Cell,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub(crate) use super::test_support::{retained_display, retained_on, test_prepared};

thread_local! {
    /// How often this thread held off the watch over a woken display.
    pub(crate) static WAKE_WATCH_HOLDS: Cell<usize> = const { Cell::new(0) };
    /// Makes this thread's streams ones whose layout took up what a stopped watch left.
    pub(crate) static BEGAN_AS_A_MONITOR_WOKE: Cell<bool> = const { Cell::new(false) };
}

fn codes(warnings: &Warnings) -> Vec<String> {
    warnings.snapshot().into_iter().map(|w| w.code).collect()
}

mod join_roam;
mod layouts;
mod mode_selection;
mod recovery_reports;
mod remembered;
mod scale_turn;
