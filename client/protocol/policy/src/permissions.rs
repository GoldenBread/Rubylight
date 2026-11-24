//! Pure stream permission, action gating, and server-command throttle policy.
//!
//! Unifies `NvConnection` and `ComputerDetails` permission checks against
//! `capabilities::PERMISSION_*` and `launch::Role`.

use crate::http::{
    PERMISSION_CLIPBOARD_READ, PERMISSION_CLIPBOARD_SET, PERMISSION_CONTROLLER, PERMISSION_INPUT,
    PERMISSION_LAUNCH, PERMISSION_SERVER_COMMAND, PERMISSION_VIEW,
};
use crate::launch::Role;

/// Minimum interval between host server commands in milliseconds.
pub const SERVER_COMMAND_THROTTLE_MS: u64 = 1_000;

/// Permission snapshot for the active stream and host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamPermissionContext {
    pub stop_requested: bool,
    pub is_monkey: bool,
    pub role: Role,
    pub paired: bool,
    /// Host permission bitmask, or `-1` when the host reports no permissions.
    pub permission: i64,
}

/// Whether `permission` grants at least one bit in `mask` (or `-1` = legacy/all allowed).
pub const fn has_permission(permission: i64, mask: u32) -> bool {
    permission == -1 || ((permission as u32) & mask) != 0
}

/// Whether ordinary mouse/keyboard/touch input may be sent (`NvConnection.canSendInput`).
pub fn can_send_input(ctx: &StreamPermissionContext) -> bool {
    !ctx.stop_requested
        && !ctx.is_monkey
        && ctx.role != Role::RemoteMonitor
        && has_permission(ctx.permission, PERMISSION_INPUT)
}

/// Whether controller input may be sent (`NvConnection.canSendControllerInput`).
pub fn can_send_controller_input(ctx: &StreamPermissionContext) -> bool {
    can_send_input(ctx) && has_permission(ctx.permission, PERMISSION_CONTROLLER)
}

/// Whether the device may change bitrate mid-stream (`NvConnection.setBitrate`).
pub fn can_change_bitrate(ctx: &StreamPermissionContext, supports_runtime_bitrate: bool) -> bool {
    !ctx.is_monkey
        && ctx.role != Role::InputOnly
        && supports_runtime_bitrate
        && has_permission(ctx.permission, PERMISSION_VIEW | PERMISSION_LAUNCH)
}

/// Whether the device may write text or images to the host clipboard (`ComputerDetails.canWriteClipboard`).
pub fn can_write_clipboard(ctx: &StreamPermissionContext) -> bool {
    ctx.role != Role::RemoteMonitor
        && ctx.permission != -1
        && has_permission(ctx.permission, PERMISSION_CLIPBOARD_SET)
        && has_permission(ctx.permission, PERMISSION_VIEW | PERMISSION_LAUNCH)
}

/// Whether the device may read text or images from the host clipboard (`ComputerDetails.canReadClipboard`).
pub fn can_read_clipboard(ctx: &StreamPermissionContext) -> bool {
    ctx.permission != -1
        && has_permission(ctx.permission, PERMISSION_CLIPBOARD_READ)
        && has_permission(ctx.permission, PERMISSION_VIEW | PERMISSION_LAUNCH)
}

/// Whether the device may run server command `command_index` (`NvConnection.sendServerCommand`).
pub fn can_run_server_command(
    ctx: &StreamPermissionContext,
    command_index: usize,
    command_count: usize,
    command_matches_name: bool,
) -> bool {
    ctx.role != Role::RemoteMonitor
        && command_index < command_count.min(256)
        && command_matches_name
        && has_permission(ctx.permission, PERMISSION_SERVER_COMMAND)
}

/// Whether the device may quit the running game or issue a native session action.
pub fn can_quit_or_control_session(ctx: &StreamPermissionContext) -> bool {
    !ctx.is_monkey && ctx.role == Role::Stream && has_permission(ctx.permission, PERMISSION_LAUNCH)
}

/// Whether the device may disconnect a remote-role session (`disconnectRemoteSession`).
pub fn can_disconnect_remote_session(ctx: &StreamPermissionContext) -> bool {
    has_permission(ctx.permission, PERMISSION_LAUNCH)
}

/// Whether the 1-second server command rate limit allows sending at `now_ms`.
pub const fn server_command_throttle_allows(last_sent_ms: Option<u64>, now_ms: u64) -> bool {
    match last_sent_ms {
        None => true,
        Some(prev) => now_ms.saturating_sub(prev) >= SERVER_COMMAND_THROTTLE_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream_ctx(permission: i64) -> StreamPermissionContext {
        StreamPermissionContext {
            stop_requested: false,
            is_monkey: false,
            role: Role::Stream,
            paired: true,
            permission,
        }
    }

    #[test]
    fn legacy_hosts_without_permissions_allow_input_and_quit_but_not_clipboard() {
        let ctx = stream_ctx(-1);
        assert!(can_send_input(&ctx));
        assert!(can_send_controller_input(&ctx));
        assert!(can_quit_or_control_session(&ctx));
        assert!(can_change_bitrate(&ctx, true));
        assert!(!can_write_clipboard(&ctx));
        assert!(!can_read_clipboard(&ctx));
    }

    #[test]
    fn remote_monitor_denies_input_clipboard_and_server_commands() {
        let mut ctx = stream_ctx(-1);
        ctx.role = Role::RemoteMonitor;
        assert!(!can_send_input(&ctx));
        assert!(!can_send_controller_input(&ctx));
        assert!(!can_quit_or_control_session(&ctx));
        assert!(!can_run_server_command(&ctx, 0, 1, true));
        ctx.permission = i64::from(
            PERMISSION_CLIPBOARD_SET
                | PERMISSION_CLIPBOARD_READ
                | PERMISSION_VIEW
                | PERMISSION_SERVER_COMMAND,
        );
        assert!(!can_write_clipboard(&ctx));
        assert!(can_read_clipboard(&ctx));
        assert!(!can_run_server_command(&ctx, 0, 1, true));
    }

    #[test]
    fn bitrate_clipboard_and_commands_require_specific_bits() {
        let only_view = stream_ctx(i64::from(PERMISSION_VIEW));
        assert!(can_change_bitrate(&only_view, true));
        assert!(!can_change_bitrate(&only_view, false));
        assert!(!can_quit_or_control_session(&only_view));
        assert!(!can_write_clipboard(&only_view));

        let mut input_only = only_view;
        input_only.role = Role::InputOnly;
        assert!(!can_change_bitrate(&input_only, true));

        let full = stream_ctx(i64::from(
            PERMISSION_VIEW
                | PERMISSION_LAUNCH
                | PERMISSION_CLIPBOARD_SET
                | PERMISSION_CLIPBOARD_READ
                | PERMISSION_SERVER_COMMAND,
        ));
        assert!(can_write_clipboard(&full));
        assert!(can_read_clipboard(&full));
        assert!(can_quit_or_control_session(&full));
        assert!(can_run_server_command(&full, 0, 1, true));
        assert!(!can_run_server_command(&full, 0, 1, false));
        assert!(!can_run_server_command(&full, 256, 300, true));
    }

    #[test]
    fn server_command_throttle_enforces_one_second_gap() {
        assert!(server_command_throttle_allows(None, 0));
        assert!(!server_command_throttle_allows(Some(1000), 1999));
        assert!(server_command_throttle_allows(Some(1000), 2000));
    }
}
