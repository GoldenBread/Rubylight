//! Non-blocking Windows V-Sync present (`DXGI_PRESENT_DO_NOT_WAIT`) with targeted VBlank sleeps.

use sdl3_sys::everything::*;

/// What a present that never waits behind the last picture did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))] // Only Windows has this present.
pub enum NoWait {
    Shown,
    /// The last picture has not reached the display: this one was not presented, and the back
    /// buffer still holds it.
    Refused,
    /// Something else went wrong. SDL's own present recovers from what can be recovered from.
    Failed,
}

/// How long [`present_without_waiting`] sleeps (ns) after `DXGI_ERROR_WAS_STILL_DRAWING`, given
/// the remaining hold budget, the display's refresh rate, the swap chain's last `SyncQPCTime`
/// and the current QPC counter/frequency. Sleeping straight to `next_vblank - 500 us` turns
/// 16-34 half-millisecond wakeups per frame into 1-2 wakeups without adding latency past the
/// vblank that frees the swap chain.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn no_wait_sleep_ns(
    remaining: std::time::Duration,
    refresh_hz: f32,
    sync_qpc: Option<i64>,
    now_qpc: u64,
    qpc_freq: u64,
) -> u64 {
    /// Fallback retry interval when DXGI frame statistics are unavailable (1 ms instead of 0.5 ms).
    const FALLBACK_NS: u64 = 1_000_000;
    /// Wake this far before the estimated next vblank so the retry catches the release cleanly.
    const LEAD_NS: u64 = 500_000;
    /// Floor for a calculated sleep so retries never tight-spin on sub-millisecond remainders.
    const MIN_NS: u64 = 500_000;

    let rem_ns = remaining.as_nanos().min(u64::MAX as u128) as u64;
    if rem_ns == 0 {
        return 0;
    }
    let fallback = FALLBACK_NS.min(rem_ns);
    let Some(sync) =
        sync_qpc.filter(|&t| t > 0 && qpc_freq > 0 && refresh_hz.is_finite() && refresh_hz > 1.0)
    else {
        return fallback;
    };
    let period_ns = (1_000_000_000.0 / f64::from(refresh_hz)) as u64;
    if period_ns == 0 {
        return fallback;
    }
    let elapsed_ticks = now_qpc.saturating_sub(sync as u64);
    let elapsed_ns = u128::from(elapsed_ticks).saturating_mul(1_000_000_000) / u128::from(qpc_freq);
    // Ignore stale frame statistics (older than 4 refreshes).
    if elapsed_ns > u128::from(period_ns) * 4 {
        return fallback;
    }
    let into_refresh = (elapsed_ns % u128::from(period_ns)) as u64;
    let to_vblank = period_ns.saturating_sub(into_refresh);
    if to_vblank <= LEAD_NS + MIN_NS {
        MIN_NS.min(rem_ns)
    } else {
        (to_vblank - LEAD_NS).min(rem_ns)
    }
}

/// Presents at the next refresh unless the last picture still waits for one, trying again for
/// `hold`.
///
/// # Safety
/// `renderer` is the live D3D11 renderer of this thread.
#[cfg(windows)]
pub unsafe fn present_without_waiting(
    renderer: *mut SDL_Renderer,
    hold: std::time::Duration,
    refresh_hz: f32,
) -> NoWait {
    use windows::Win32::Graphics::Dxgi::{
        DXGI_ERROR_WAS_STILL_DRAWING, DXGI_FRAME_STATISTICS, DXGI_PRESENT_DO_NOT_WAIT,
        IDXGISwapChain,
    };
    use windows::core::Interface;
    // SAFETY: caller guarantees `renderer` is the live D3D11 renderer of this thread.
    unsafe {
        let chain = crate::render_device::live_renderer_pointer(
            renderer,
            SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER,
        );
        let Some(chain) = IDXGISwapChain::from_raw_borrowed(&chain) else {
            return NoWait::Failed;
        };
        if !SDL_FlushRenderer(renderer) {
            return NoWait::Failed;
        }
        let begun = std::time::Instant::now();
        let freq = SDL_GetPerformanceFrequency();
        let result = loop {
            let result = chain.Present(1, DXGI_PRESENT_DO_NOT_WAIT);
            let remaining = hold.saturating_sub(begun.elapsed());
            if result != DXGI_ERROR_WAS_STILL_DRAWING || remaining.is_zero() {
                break result;
            }
            // Sleep once until just before the next vblank that frees the swap chain.
            let mut stats = DXGI_FRAME_STATISTICS::default();
            let sync_qpc = chain
                .GetFrameStatistics(&mut stats)
                .ok()
                .map(|_| stats.SyncQPCTime);
            let sleep_ns = no_wait_sleep_ns(
                remaining,
                refresh_hz,
                sync_qpc,
                SDL_GetPerformanceCounter(),
                freq,
            );
            if sleep_ns > 0 {
                SDL_DelayNS(sleep_ns);
            }
        };
        // A present unbinds the back buffer's view: this makes SDL's next draw bind it again.
        SDL_FlushRenderer(renderer);
        if result == DXGI_ERROR_WAS_STILL_DRAWING {
            NoWait::Refused
        } else if result.is_ok() {
            NoWait::Shown
        } else {
            NoWait::Failed
        }
    }
}

/// Only Windows has this present: SDL's own takes over elsewhere.
///
/// # Safety
/// Nothing to uphold here; the signature is the Windows one's.
#[cfg(not(windows))]
pub unsafe fn present_without_waiting(
    _renderer: *mut SDL_Renderer,
    _hold: std::time::Duration,
    _refresh_hz: f32,
) -> NoWait {
    NoWait::Failed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn no_wait_present_sleeps_once_until_just_before_next_vblank() {
        // 10 MHz QPC (Windows standard), 100 Hz display (10 ms period).
        let freq = 10_000_000u64;
        let sync = 1_000_000i64;
        // 2 ms after last vblank -> 8 ms left -> sleeps 7.5 ms (500 us lead before vblank).
        assert_eq!(
            no_wait_sleep_ns(Duration::from_millis(10), 100.0, Some(sync), 1_020_000, freq),
            7_500_000
        );
        // Clamped to remaining hold budget.
        assert_eq!(
            no_wait_sleep_ns(Duration::from_millis(3), 100.0, Some(sync), 1_020_000, freq),
            3_000_000
        );
        // Within 1 ms of vblank -> 500 us floor so we never spin.
        assert_eq!(
            no_wait_sleep_ns(Duration::from_millis(10), 100.0, Some(sync), 1_093_000, freq),
            500_000
        );
        // Missing or stale stats (>4 refreshes old) fall back to 1 ms instead of 0.5 ms polling.
        assert_eq!(
            no_wait_sleep_ns(Duration::from_millis(10), 100.0, None, 1_020_000, freq),
            1_000_000
        );
        assert_eq!(
            no_wait_sleep_ns(Duration::from_millis(10), 100.0, Some(sync), 1_500_000, freq),
            1_000_000
        );
        // Zero remaining budget sleeps nothing.
        assert_eq!(
            no_wait_sleep_ns(Duration::ZERO, 100.0, Some(sync), 1_020_000, freq),
            0
        );
    }
}
