---
name: add-control-message
description: Add a new Rubylight control-stream protocol message end to end across rubylight-protocol, host RTSP/control dispatch, Nightjar client, docs, and tests.
---

# Add a Control Message

Use this skill when adding a client<->host control-stream message on ENet (or WebRTC control channel). Protocol changes MUST land with their Nightjar client side and preserve `PROTOCOL_VERSION` and stock Moonlight compatibility.

## Wire conventions

- Message IDs are `u16` registered once in `client/protocol/core/src/ids.rs` (`CONTROL_IDS`). Never reuse an existing or reserved ID.
- Payloads are little-endian, start with `version: u8 = 1`, ignore trailing bytes for forward compatibility, and reject truncated/bad-version/implausible payloads cleanly (log at debug, never panic).
- Stock clients that do not opt in MUST see no change in default RTSP `DESCRIBE` (verified by `tests/host/tests/reconnect_tests/extension_tests.rs`).
- Never block the ENet control loop (`control` in `rust/host/stream/src/stream/control/event_loop.rs`); offload stateful or slow work to a dedicated worker beside `control.rs` (see `rust/host/stream/src/stream/clipboard_sync.rs`).

## Step-by-step checklist

1. **Scaffold the `no_std` wire module** (checks `ids.rs` for collisions):
   ```powershell
   python tools/scaffold.py control-message <name> --id 0x55XX --dry-run
   python tools/scaffold.py control-message <name> --id 0x55XX
   ```
2. **Register in `rubylight-protocol` (`client/protocol/core/`):**
   - Export `pub mod <name>;` in `client/protocol/core/src/lib.rs`.
   - Reserve the constant in `CONTROL_IDS` inside `client/protocol/core/src/ids.rs`.
   - Add round-trip / no-panic property coverage in `client/protocol/core/src/fuzz.rs`.
3. **Host negotiation:**
   - Add an opt-in / capability flag to `Offered` in `rust/core/media-policy/src/extensions.rs` (`serverinfo_fields`, `describe_lines` emitting `a=x-rl-control:...`).
   - Populate it per stream in `stream_offered` (`rust/host/stream/src/extensions.rs`).
4. **Host dispatch:**
   - Dispatch the message ID in the matching family handler under `rust/host/stream/src/stream/control/`: `handle_qos_packet` (`qos.rs`), `handle_clipboard_and_focus_packet` (`clipboard.rs`) or `handle_extension_packet` (`extensions.rs`); `handle_control_packet` in `event_loop.rs` chains them. Register a new submodule in `rust/host/stream/src/stream/control.rs`.
   - Add a companion worker module beside `control.rs` if it holds state or does I/O.
5. **Nightjar client (same change):**
   - Engine: add send variant in `client/protocol/client/src/control/messages.rs`, receive event in `client/protocol/client/src/control/events.rs`, state handling in `client/protocol/client/src/control/session.rs`. Only send when the host announced the ID in `a=x-rl-control`: return it from `announcement` in `messages.rs` (parsed in `client/protocol/client/src/control/negotiation.rs`).
   - Android: handle in `client/protocol/android/src/engine.rs`, add the JNI entry in `client/protocol/android/src/jni_stream.rs`, declare it in `client/app/src/main/java/com/limelight/nvstream/jni/NightjarJni.java` and forward it from `client/app/src/main/java/com/limelight/nvstream/jni/RustBridge.java`. Host events reach Java through `client/protocol/android/src/listener.rs` and `client/protocol/android/src/bridge.rs`.
   - Desktop: handle in `client/protocol/desktop-media/src/stream/`.
6. **Docs & contract tests:**
   - Document framing and negotiation in `docs/control-messages.md`.
   - Add contract coverage in `client/protocol/client/tests/control_messages_contract.rs` and Android `client/app/src/test/java/com/limelight/nvstream/jni/MoonBridgeControlMessagesTest.java`.
   - Add host loopback tests under `tests/host/tests/reconnect_tests/` (use `python tools/scaffold.py test-scenario <name> --suite reconnect`) and check `extension_tests.rs`.
   - Add cross-process/in-process interop coverage in `tests/interop/` if host+Nightjar state interacts.

## Verification

```powershell
$env:TEMP="D:\t"; $env:TMP="D:\t"
python tools/dev.py cargo -- test --manifest-path client/protocol/Cargo.toml -p rubylight-protocol -p nightjar-core --locked
python tools/dev.py cargo -- test -p butterpollo-host-tests --test reconnect_tests --locked
python tools/dev.py cargo -- test -p rubylight-interop --locked
python tools/arch_check.py
python tools/code_map.py check
tools\dev.ps1 check
```
