//! Clipboard sync, text-input focus notices, keyboard layout reports, and client-drawn
//! cursor shape messages over the control channel.

use super::super::*;
use super::peer::ControlPeer;
use super::socket::ControlSocket;

/// Dispatches clipboard sync, text input, keyboard layout, and cursor shape packets.
/// Returns `true` if `kind` was handled.
pub(super) fn handle_clipboard_and_focus_packet(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    p: &mut ControlPeer,
    s: &Arc<Session>,
    kind: u16,
    payload: &[u8],
    encrypted: bool,
) -> bool {
    match kind {
        // Keyboard follows focus: only for a device that may type, and
        // the text around the cursor only over encrypted control.
        butterpollo_input_policy::text_input::TEXT_INPUT_MESSAGE_TYPE => {
            let Ok(config) = p.config(h, s) else {
                return true;
            };
            let offered = crate::extensions::offered(h, &config).text_input
                && s.launch.client.allows(input::KEYBOARD_PERMISSION);
            let surrounding = encrypted && config.text_input_surrounding_text();
            let watch = s.text_input.on_request(payload, offered, surrounding);
            match watch {
                Some(watch) => {
                    tracing::info!(
                        client = %s.launch.client.name,
                        ?watch,
                        offered,
                        "text input"
                    );
                    // The fixture's tests note the focus themselves.
                    #[cfg(any(test, feature = "test-support"))]
                    if let Some(fixture) = &h.reconnect_fixture {
                        fixture.text_watches.lock().unwrap().push(watch);
                        return true;
                    }
                    p.watch_focus(s, watch, control_wake);
                }
                None => tracing::debug!(len = payload.len(), "invalid text input request"),
            }
        }
        // Clipboard sync from a Nightjar that asked or a Moonlight V+
        // client, which never asks: its first frame starts the sync.
        // Encrypted only, as USB: a plain frame anyone on the network
        // could send would set the PC's clipboard and start sending it.
        rubylight_protocol::clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE if encrypted => {
            let Ok(config) = p.config(h, s) else {
                return true;
            };
            // A guest's clipboard needs its keyboard-and-mouse permission:
            // without it, neither direction.
            let allowed = if h.device_guests.full_access(s) {
                clipboard_sync::allowed(&config, &s.launch.client)
            } else {
                clipboard_sync::Allowed::default()
            };
            p.clipboard_sync.receive(s, payload, allowed, control_wake);
        }
        // The latest report wins; the injector maps keys with it.
        butterpollo_input_policy::keyboard_layout::KEYBOARD_LAYOUT_MESSAGE_TYPE => {
            let Ok(config) = p.config(h, s) else {
                return true;
            };
            if !crate::extensions::offered(h, &config).keyboard_layout {
                tracing::debug!("keyboard layout report not offered");
                return true;
            }
            // Only a device that may type picks the layout keys go with.
            if !s.launch.client.allows(input::KEYBOARD_PERMISSION) {
                tracing::debug!("keyboard layout report without the keyboard permission");
                return true;
            }
            match butterpollo_input_policy::keyboard_layout::KeyboardLayout::decode(payload) {
                Some(layout) => {
                    // Resolved once here; the layout is read on this
                    // loop, never with input.
                    let klid = butterpollo_input_policy::keyboard_layout::reported_klid(&layout);
                    let before = std::mem::replace(&mut *s.keyboard_layout.lock().unwrap(), klid);
                    if before != klid {
                        tracing::info!(
                            client = %s.launch.client.name,
                            source = ?layout.source,
                            reported = format_args!("{:08X}", layout.klid),
                            tag = %layout.tag,
                            klid = ?klid.map(|klid| format!("{klid:08X}")),
                            "keyboard layout"
                        );
                    }
                    p.keyboard_layout
                        .report(klid, &s.launch.warnings, Instant::now());
                }
                None => tracing::debug!(len = payload.len(), "invalid keyboard layout report"),
            }
        }
        // The device draws the cursor (0x5540): the stream leaves it
        // out of the video, and the pass below sends its shapes.
        butterpollo_input_policy::client_cursor::CURSOR_SHAPE_REQUEST_MESSAGE_TYPE => {
            let Ok(config) = p.config(h, s) else {
                return true;
            };
            let offered = crate::extensions::offered(h, &config).cursor_shape;
            match s.client_cursor.on_request(payload, offered, p.cursor_owner) {
                Some(enabled) => {
                    tracing::info!(
                        client = %s.launch.client.name,
                        enabled,
                        "client-drawn cursor"
                    );
                    let _ = control_wake.send(&[1]);
                }
                None => tracing::debug!(
                    len = payload.len(),
                    offered,
                    "invalid or unoffered cursor shape request"
                ),
            }
        }
        _ => return false,
    }
    true
}

/// Sends queued text-input focus notices, clipboard sync frames, and client-drawn cursor
/// shapes for `peer_id`.
pub(super) fn send_clipboard_and_focus_updates(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    host: &mut Host<ControlSocket>,
    peer_id: PeerID,
    p: &mut ControlPeer,
    s: &Arc<Session>,
) {
    // The text field notice for a client that asked (0x553B): reliable,
    // the newest focus at most every 50 ms.
    if s.text_input.queued() {
        let notice = s.text_input.take(Instant::now());
        if let Some(notice) = notice {
            match p.encrypt(
                s,
                butterpollo_input_policy::text_input::TEXT_INPUT_MESSAGE_TYPE,
                &notice,
            ) {
                Ok(message) => {
                    if let Err(e) = host
                        .peer_mut(peer_id)
                        .send(0, &Packet::new(message, PacketKind::Reliable))
                    {
                        tracing::warn!(error = %e, "text input notice not sent");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "text input notice not encrypted");
                }
            }
        }
    }
    // The PC's newest copy for a device that syncs the clipboard (0x5508):
    // reliable.
    // A guest's permission is asked per copy: one made view only gets no more.
    if p.clipboard_sync.queued()
        && let Some(frame) = p
            .clipboard_sync
            .take(Instant::now(), || h.device_guests.full_access(s))
    {
        match p.encrypt(
            s,
            rubylight_protocol::clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE,
            &frame,
        ) {
            Ok(message) => {
                if let Err(e) = host
                    .peer_mut(peer_id)
                    .send(0, &Packet::new(message, PacketKind::Reliable))
                {
                    tracing::warn!(error = %e, "clipboard sync frame not sent");
                }
            }
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "clipboard sync frame not encrypted");
            }
        }
    }
    // The device-drawn cursor's shapes (0x5541): reliable, the newest at
    // most every 16 ms.
    if s.client_cursor.queued() {
        let shape = s
            .client_cursor
            .take(p.cursor_owner, Instant::now(), |output| {
                let wake = control_wake.clone();
                p.cursor_scale.poll(
                    output,
                    butterpollo_win_display::display::output_scale,
                    move || {
                        let _ = wake.send(&[1]);
                    },
                )
            });
        if let Some(shape) = shape {
            match p.encrypt(
                s,
                butterpollo_input_policy::client_cursor::CURSOR_SHAPE_MESSAGE_TYPE,
                &shape,
            ) {
                Ok(message) => {
                    if let Err(e) = host
                        .peer_mut(peer_id)
                        .send(0, &Packet::new(message, PacketKind::Reliable))
                    {
                        tracing::warn!(error = %e, "cursor shape not sent");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "cursor shape not encrypted");
                }
            }
        }
    }
}
