//! Wi-Fi streaming mode on Linux: no adapter is put in a streaming mode yet, so entering it
//! reports that nothing was changed.

/// Keeps connected Wi-Fi adapters in media streaming mode while it lives. On Linux it is
/// never handed out.
pub struct WifiStreamingGuard(());

/// Enable media streaming mode on every connected Wi-Fi adapter; returns `None` when it is
/// unavailable, which is always so on Linux.
pub fn enter_streaming_mode() -> Option<WifiStreamingGuard> {
    None
}
