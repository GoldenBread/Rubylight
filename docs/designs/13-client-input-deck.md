# Visual Console-Grade Controller Editor & Client Input Deck on First-Party Virtual Gamepad (7.0.0 — Wave 3)

[Protocol reservations](00-protocol-reservations.md) · [Code map](../code-map.md) · [Nightjar code map](../../client/docs/code-map.md)

Status: engineering design (Phase 1). Written against `main` `b3d0c26b9b` (`6.2.0`) and `00-protocol-reservations.md`.

## 1. Summary & User Experience

Item 13 replaces dozens of scattered flat controller preference keys with **ONE flagship Visual Console-Grade Controller Editor** (Steam Input Configurator × Xbox Accessories × DualSense Edge Profile Studio caliber) paired with **Client Input Deck** — across Nightjar Android (Phone, Handheld, Android TV / Fire TV) and Desktop (Windows, Linux / Steam Deck, macOS) at full parity — backed exclusively on the host by Rubylight's first-party UMDF 2 VHF virtual gamepad driver (`rust/drivers/rubylight-vgamepad`, hardware ID `Root\RubylightVirtualGamepad`) and existing USB/IP virtual HID mouse/touchpad (`rust/core/src/virtual_hid.rs`) plus `SendInput` keyboard injection. It ships **zero unsigned KMDF drivers** and **zero `mouse_mapper`** — preserving the deliberate exclusion in `rust/drivers/rubylight-vgamepad/README.md`.

### Replacing scattered controller settings with ONE Visual Controller Editor

Instead of fragmenting controller configuration across dozens of flat rows (dead zones, stick inversion, button swaps, back paddles, gyro axes/sensitivity, trigger thresholds, rumble feel, per-model mappings, touch layouts), all controller customization lives inside a single cohesive **Controller Editor** reachable from **Settings → Controls**, any **Game Page**, and the in-stream **Console Control Center**:

1. **Live Interactive Controller Diagram:** Adapts visually to the connected controller (`DualSense`, `Xbox Series / Elite`, `Switch Pro / Switch 2 Pro`, `Steam Controller 2026`, `Steam Deck / Handheld`, `Generic`, plus `Modern Touch Layout` mode for `#151`). Live real-time input telemetry highlights sticks, triggers, face/shoulder buttons, back paddles (`L4/R4/L5/R5`), trackpads, and gyro orientation directly on the interactive diagram.
2. **Direct Manipulation & 100% Controller / TV-Remote / Keyboard / Touch / Screen-Reader Navigable:** Focusing any element on the visual diagram opens its dedicated inline studio:
   - **Buttons, Back Paddles (`L4/R4/L5/R5`), Share/Capture & Chords:** Bind to gamepad buttons, keyboard keys/chords, mouse clicks, Nightjar actions (*Console Control Center*, *Save Clip*, *Screenshot*, *Mic Mute*, *Switch Aspect*, *Radial Menu*), or Action Layer hold/toggle. Multi-button chords evaluate deterministically with optional short suppression (`<= 25 ms`, default `0 ms` for non-conflicting chords).
   - **Sticks & Triggers Studio:** Interactive live circular/axial **Dead Zone & Outer Ring** visualizer, **Response Curve** preview (*Linear*, *Aggressive*, *Relaxed*, *Wide*, *Custom*), trigger actuation / hair-trigger thresholds, and DualSense adaptive trigger feel.
   - **Gyroscope Studio:** Live 3D/2D orientation visualizer, activation trigger (*Always*, *Hold L2*, *Stick Touch*, *Toggle*), output mode (*Gyro-to-Mouse*, *Gyro-to-Stick*, *Native Motion*, *DSU*), per-axis sensitivity, invert, smoothing, flick-stick, ratcheting pause, and 1-tap **Calibrate Zero**. Gyro-to-stick routes through the VHF virtual gamepad right stick (`0x5500_0081`); gyro-to-mouse routes relative deltas through Rubylight's USB/IP virtual mouse (`0x1209:0x52b1`, never flagged injected) or `SendInput` fallback (reported visibly).
   - **Trackpads & Radial Menu Builder:** Visual 4-to-12-slice **Radial / Touch Menu** editor (icons, labels, bindings, deadzone) + trackpad mode selector; in-stream radial wheel renders strictly blur-free using the 7.0 HUD token set (`blur: 0`, `shadowBlur: 0`).
   - **Haptics & Lightbar Studio:** Live test pulse for rumble, trigger motors, and DualSense/Steam waveform haptics, audio-to-haptics toggle/gain, and LED/lightbar color picker.
   - **On-Screen Touch Controller Studio (`#151`, Android):** Seamless **Touch** tab/mode in the same Controller Editor — freely resizable/reshapable controls across the full screen, separable ABXY & D-pad clusters, movable floating micro-toolbar, and `0%`-opacity stealth mode.
3. **Layered Profiles (`Global → Per-Controller-Model → Per-Game`) + Hub Sync:** Profiles cascade cleanly from Global defaults to Per-Controller-Model overrides to Per-Game customizations, support portable JSON import/export, sync automatically across the user's paired Nightjar devices via `09-host-hub-sync`, and work across every game launcher on top of `rust/drivers/rubylight-vgamepad`'s 6 first-party VHF profiles (`XboxOne`, `XboxSeries`, `DualShock4`, `DualSense`, `SwitchPro`, `SteamController`).
4. **Lossless First-Launch Preference Migration:** Existing flat controller preference keys (`deadzone`, stick/button swaps, gyro settings, trigger toggles, rumble switches, legacy touch layout preferences) are migrated automatically into the user's initial `Global` controller profile on first 7.0 launch and cleanly retired from the flat Settings list (`SettingsKeysTest` + `config.rs::migrate`, never reusing retired key names and never losing a player's calibration).

### Zero-overhead guarantee when unused

When a game uses the stock default profile with no custom chords, radials, or gyro-to-mouse overrides, Nightjar neither sends `rlInputDeck=1` nor mounts any HUD layer, and the host allocates zero deck routing state. Standard Moonlight input packets (`0x0206` controller, keyboard, mouse) execute byte-for-byte identically to `6.2.0`.

## 2. Protocol, Native API & Capability Contract

### Compatibility invariants

- **`PROTOCOL_VERSION` stays unchanged.** Stock Moonlight clients and pre-7.0.0 Nightjar builds never send `rlInputDeck=1`, `0x554D`, or `0x5500_0081` and observe zero wire changes.
- **`rubylight_native_api::API_VERSION` stays `1`.** All Native API additions are additive (`#[serde(default)]` on responses, `#[serde(deny_unknown_fields)]` on requests, `#[serde(other)] Unknown` catch-all enum variants).
- **Capability-gated, never brand-gated.** Nightjar gates all Input Deck UI and wire sends strictly on `HostCapabilities::has(Feature::InputDeck)` (`HostCapabilities.Feature.INPUT_DECK` on Java), never on `Family::Rubylight` or version strings.

### Capability advertisement & launch opt-in

| Surface | Field / Line | Condition |
| --- | --- | --- |
| `/serverinfo` | `<InputDeckVersion>1</InputDeckVersion>` | `Offered::input_deck == true` (host setting `input_deck` on AND at least one of `controller`, `keyboard`, or `mouse` enabled) |
| RTSP `DESCRIBE` | `a=x-rl-control:0x554d` (merged ascending into `a=x-rl-control:...`) | Launch query carried `rlInputDeck=1` AND `Offered::input_deck == true` |
| Launch query | `rlInputDeck=1` | Client opts in when host advertises `InputDeckVersion >= 1` and active game has an enabled deck (or user enabled live deck editing) |
| Native API `Capabilities::features` | `"input.deck"` (`feature::INPUT_DECK`) | Host supports Input Deck routes and caller holds `launch` permission |

`HostCapabilities::Feature::InputDeck` (`INPUT_DECK` in Java) is proven when `InputDeckVersion >= 1` **and** native `"input.deck"` is present (merged via `HostCapabilities::with_native` / `withNativeCapabilities`).

### Control message `0x554D` (`input_deck::INPUT_DECK_ACTION_MESSAGE_TYPE`)

Defined in `client/protocol/core/src/input_deck.rs` (`no_std + alloc`), registered in `client/protocol/core/src/ids.rs` inside `RUBYLIGHT_7_CONTROL_IDS = 0x5548..=0x554F`. Delivery: **Bidirectional, Reliable** on the encrypted ENet control stream. All multi-byte integers are **little-endian**. Maximum message size: **256 bytes**.

```text
Offset  Len  Field
0       1    version = 1
1       1    opcode (u8):
               0x01 = LayerSync       (Client -> Host: active layer stack changed)
               0x02 = ActionTrigger   (Client -> Host: discrete chord/radial/macro action)
               0x03 = ProfileSwitch   (Client -> Host: request VHF profile on slot)
               0x04 = ReleaseAll      (Client -> Host: immediate panic release of deck holds)
               0x05 = HostAck         (Host -> Client: ack sequence + effective slot/profile)
               0x06 = HostError       (Host -> Client: structured refusal/error)
2       2    flags (u16 LE; bit 0 = press/down, bit 1 = release/up, bit 2 = repeat_suppress)
4       4    seq (u32 LE; strictly increasing per direction)
8       1    slot (u8; 0..15 virtual gamepad slot index, 0xFF = keyboard/mouse only)
9       1    reserved (0)
10      2    layer_id (u16 LE; 0 = base layer)
12      ..   opcode-specific body:
               0x01 LayerSync:     active_depth: u8, stack: [u16; active_depth <= 8]
               0x02 ActionTrigger: target_kind: u8 (1=VhfButton, 2=KeyScan, 3=KeyVk,
                                   4=MouseButton, 5=MouseWheel, 6=KeyChord<=4),
                                   count: u8 (1..=4), entries: [u32; count]
               0x03 ProfileSwitch: vhf_profile: u16 LE (rubylight_vgamepad_proto::Profile
                                   discriminant 3..=7 or 9; 0 = host default)
               0x04 ReleaseAll:    scope: u8 (0 = all deck holds, 1 = slot only, 2 = kbd/mouse)
               0x05 HostAck:       ack_seq: u32, effective_profile: u16, active_holds: u16
               0x06 HostError:     ref_seq: u32, code: u8 (1=DriverUnavailable,
                                   2=ProfileUnsupported, 3=PermissionDenied,
                                   4=InvalidSlot, 5=SecureDesktopBlocked,
                                   6=GuestSlotReadOnly, 7=UnknownOpcode),
                                   detail_len: u8 (<= 160), detail_utf8: [u8; detail_len]
```

Compatibility rule: a receiver ignores trailing bytes beyond the expected length for `version == 1`; an unknown `version` returns `None` from `decode()` and is ignored safely by older peers.

### Input packet magic `0x5500_0081` (`input_deck::INPUT_DECK_VIRTUAL_AXIS_MAGIC`)

Carried inside standard Moonlight input packets (`0x0206`: 4-byte BE length, 4-byte LE magic `0x5500_0081`), Client → Host, fixed **36 bytes total** (including 4-byte length + 4-byte magic; 28-byte payload). Unreliable/sequenced input channel for high-rate (up to 500 Hz) gyro-to-mouse, gyro-to-stick, and continuous radial dial streaming without blocking the reliable ENet control queue:

```text
Offset  Len  Field (after 4B BE length + 4B LE magic 0x5500_0081)
0       1    version = 1
1       1    flags:
               bit 0 (0x01) = apply_pad_axes (lx, ly, rx, ry, lt, rt)
               bit 1 (0x02) = apply_pad_buttons (buttons_mask ORed into slot deck hold mask)
               bit 2 (0x04) = apply_rel_mouse (mouse_dx, mouse_dy)
               bit 3 (0x08) = apply_wheel (wheel_x, wheel_y; 120 = 1 notch)
               bit 4 (0x10) = gyro_active (diagnostic & ratcheting telemetry bit)
2       1    slot (u8; 0..15)
3       1    reserved (0)
4       2    layer_id (u16 LE)
6       2    lx (i16 LE; -32768..32767)
8       2    ly (i16 LE)
10      2    rx (i16 LE)
12      2    ry (i16 LE)
14      1    lt (u8; 0..255)
15      1    rt (u8; 0..255)
16      2    mouse_dx (i16 LE; raw counts)
18      2    mouse_dy (i16 LE; raw counts)
20      2    wheel_y (i16 LE; signed high-res delta)
22      2    wheel_x (i16 LE; signed horizontal pan delta)
24      4    buttons_mask (u32 LE; rubylight_vgamepad_proto::protocol::button_mask)
```

Host validation: packets arriving on a session that did not negotiate `rlInputDeck=1` are dropped silently (counted in debug metrics) so stock or misbehaving clients cannot inject unauthorized deck state.

### Native API v1 routes (`rust/core/native-api/src/input_deck.rs`)

All routes live under `/api/native/v1`, authenticate the paired TLS client certificate, require `launch` permission (`butterpollo_core::native::LAUNCH`), and scope mutations strictly to the caller's own device UUID (`device_uuid(&client.uuid)`).

| Method & Path | Permission | Purpose |
| --- | --- | --- |
| `GET /api/native/v1/input/decks` | `launch` | Lists all deck profiles for this device (and host-synced per-`app_id` defaults from `09-host-hub-sync`), plus host VHF driver capabilities (`available_profiles`, `virtual_mouse_ready`, `active_deck_id`, `revision`). |
| `PUT /api/native/v1/input/decks/{id}` | `launch` | Creates or atomically updates deck `{id}` (standardized route matching `00-protocol-reservations.md`). Attaches an explicit per-route `.layer(DefaultBodyLimit::disable())` + `axum::body::to_bytes(req.into_body(), 64 * 1024)` override in `rust/host/src/native_api.rs` (overriding the router-wide 16 KiB `DefaultBodyLimit::max(16 * 1024)`), accepting up to 64 KiB JSON and rejecting larger bodies with `ErrorCode::TooLarge` (`"too_large"`, HTTP `413`); schema-validated (max 16 layers, max 12 slices per radial, max 64 chords, max 4 keys per chord). Updates `09-host-hub-sync` profile revision if `sync_across_devices = true`. |
| `DELETE /api/native/v1/input/decks/{id}` | `launch` | Removes deck `{id}` for this device (and clears it if currently active, triggering `ReleaseAll` on any active stream owned by caller). |
| `POST /api/native/v1/input/decks/active` | `launch` | Binds `{ "app_id": "...", "deck_id": "..." \| null }` for live switch during an owned stream or default for next launch; returns effective status & revision. |

Error codes reuse existing `ErrorCode` variants in `rust/core/native-api/src/error.rs`: `ErrorCode::NotPaired` (`"not_paired"`, HTTP `403`), `ErrorCode::PermissionRequired` (`"permission_required"`, HTTP `403`), `ErrorCode::InvalidValue` (`"invalid_value"`, HTTP `422`, schema limit exceeded or invalid key/profile), `ErrorCode::NotFound` (`"not_found"`, HTTP `404`), `ErrorCode::NotSessionOwner` (`"not_session_owner"`, HTTP `409`), and `ErrorCode::TooLarge` (`"too_large"`, HTTP `413`, body above 64 KiB).

## 3. Architecture & <=800-LOC Module Plan

All repo paths below carry `[verify post-sprint code-map.md]` so Phase 2 verifies exact post-sprint layout before Phase 3 coding begins. Every new Rust module opens with a `//!` header, every new Java class opens with a `/**` header, and zero raw `Config` reads are added under `rust/host/src/stream/**` outside `stream/config.rs` (`tools/arch_check.py`).

### Grandfathered files in `tools/size_ratchet.json` (DO NOT GROW)

| File | Ratchet ceiling (LOC) | Strategy to stay at or below ceiling |
| --- | --- | --- |
| `rust/host/src/stream/control.rs` `[verify post-sprint code-map.md]` | 2675 | Add only a 3-line dispatch arm for `0x554D` and `Input::InputDeckAxis` forwarding to `super::input_deck::handle_*`. Extract any touched helper if net lines would increase. |
| `rust/windows/src/input.rs` `[verify post-sprint code-map.md]` | 1342 | Add only `mod deck_router;` and a 2-line forward in `Injector::apply` + `release_all`. Zero inline deck logic. |
| `rust/windows/src/input/gamepad_thread.rs` `[verify post-sprint code-map.md]` | 1359 | Add 1 enum variant (`DeckAxis` / `DeckProfileSwitch`) delegating to `deck_router.rs`. |
| `rust/windows/src/input/gamepads.rs` `[verify post-sprint code-map.md]` | 1788 | Reuse existing `Backend::Vhf` slot open/reprofile/report path; add a <=8-line helper hook calling `deck_router`. |
| `rust/core/src/virtual_hid.rs` `[verify post-sprint code-map.md]` | 1123 | Untouched — `deck_router.rs` feeds existing `HidMouse` relative/wheel/button API via `virtual_hid::MOUSE`. |
| `rust/host/src/native_api.rs` `[verify post-sprint code-map.md]` | 853 | Add only `mod input_deck;` + 4 route registrations in `router()` (with explicit per-route `.layer(DefaultBodyLimit::disable())` + `axum::body::to_bytes(req.into_body(), 64 * 1024)` on `PUT /api/native/v1/input/decks/{id}` overriding the router-wide 16 KiB `DefaultBodyLimit::max(16 * 1024)`) + 1 capability check line (offset by moving existing route grouping helper if needed to stay `<= 853`). |
| `client/protocol/client/src/control/driver.rs` `[verify post-sprint code-map.md]` | 1288 | Delegate `0x554D` encode/decode to `control/input_deck.rs` (3-line hook). |
| `client/protocol/client/src/input/queue.rs` `[verify post-sprint code-map.md]` | 1285 | Add 1-line `enqueue_deck_axis` delegating to `input/deck.rs`. |
| `client/protocol/client/src/http/capabilities.rs` `[verify post-sprint code-map.md]` | 1257 | Keep addition minimal (enum variant + 1 match arm; split evidence table into `capabilities/evidence.rs` if approaching ceiling). |
| `client/protocol/desktop/src/gamepad.rs` `[verify post-sprint code-map.md]` | 1375 | Add 3-line hook passing raw SDL pad/gyro events into new `stream/input_deck.rs`. |
| `client/protocol/desktop/src/stream/mod.rs` `[verify post-sprint code-map.md]` | 2914 | Delegate all deck HUD & axis dispatch to new `stream/input_deck_hud.rs`. |
| `client/protocol/desktop/src/strings/*.rs` `[verify post-sprint code-map.md]` | 1750 | **Zero growth!** Route all new 7.0.0 Input Deck Desktop translations to `client/protocol/desktop/src/strings_70/<code>.rs` (14 non-English locale tables; English literals live at call sites). |
| `client/app/src/main/java/com/limelight/binding/input/ControllerHandler.java` `[verify post-sprint code-map.md]` | 5735 | **Zero feature logic inside `ControllerHandler.java`.** Add only a 4-line interceptor callback (`InputDeckHook`) forwarding pad/gyro events and live editor telemetry to `binding/input/deck/InputDeckEngine.java`. |
| `client/app/src/main/java/com/limelight/preferences/StreamSettings.java` `[verify post-sprint code-map.md]` | 3425 | **Shrinks, never grows!** Replace scattered flat controller rows with one flagship **Controller Editor** launcher row under Controls; keep clean key migrations in `ControllerProfileMigrator.java` + `SettingsKeysTest.java`. |
| `client/app/src/main/java/com/limelight/binding/input/pad/PadEditor.java` `[verify post-sprint code-map.md]` | 1157 | **Do not grow.** Wrap via `binding/input/editor/TouchStudioTabBridge.java` (`<= 300` LOC) so `#151` Touch Studio shares the same Controller Editor shell without bloating `PadEditor.java`. |
| `client/protocol/desktop/src/ui/settings.rs` `[verify post-sprint code-map.md]` | 4087 | **Shrinks, never grows!** Replace scattered flat controller preference rows with one **Open Controller Editor** card under Controls. |
| `client/app/src/main/java/com/limelight/Game.java` `[verify post-sprint code-map.md]` | 1883 | Mount `InputDeckRadialView` via 3-line delegate in `GameOverlays.java` / `InputDeckCoordinator.java`. |
| `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java` `[verify post-sprint code-map.md]` | 1347 | Add only `INPUT_DECK(0)` enum constant + 2-line parse/native merge; trim redundant comment/blank lines if needed so LOC `<= 1347`. |

### Planned new <=800-LOC modules

| New module path | Crate / App | Target LOC | Responsibility |
| --- | --- | --- | --- |
| `client/protocol/core/src/input_deck.rs` `[verify post-sprint code-map.md]` | `rubylight-protocol` | ~320 | `no_std` wire codecs for `0x554D` (`DeckMessage`) and `0x5500_0081` (`DeckVirtualAxis`), bounds checks, round-trip unit tests. |
| `rust/core/native-api/src/input_deck.rs` `[verify post-sprint code-map.md]` | `rubylight-native-api` | ~320 | Additive v1 serde types (`DeckList`, `DeckDocument`, `ControllerProfileScope`, `StickCurve`, `TriggerConfig`, `GyroConfig`, `HapticsLightbarConfig`, `DeckLayer`, `RadialMenu`, `ChordBinding`, `ActiveDeckRequest`), fixtures. |
| `rust/core/src/input_deck.rs` `[verify post-sprint code-map.md]` | `butterpollo-core` | ~380 | Pure host deck validator (max layers/slices/chords), per-session held-button/key state tracker (`DeckHoldTracker`), safe release-all planner, profile compatibility matrix (`VHF` vs `ViGEmBus`). |
| `rust/windows/src/input/deck_router.rs` `[verify post-sprint code-map.md]` | `butterpollo-windows` | ~360 | Routes validated deck actions to `GamepadThread` (`rubylight-vgamepad` VHF slots), `virtual_hid::MOUSE` (`usbip-win2` raw HID mouse, visible notice if falling back to `SendInput`), and `Injector` keyboard chords sharing `SEND_ORDER` + `HELD`. Zero KMDF / zero `mouse_mapper`. |
| `rust/host/src/stream/input_deck.rs` `[verify post-sprint code-map.md]` | `butterpollo` | ~340 | Session deck controller: processes `0x554D` + `0x5500_0081`, enforces `rlInputDeck=1` opt-in & guest slot isolation, drives 15 s hold watchdog, emits `HostAck`/`HostError`, guarantees `release_all` on teardown. |
| `rust/host/src/native_api/input_deck.rs` `[verify post-sprint code-map.md]` | `butterpollo` | ~310 | HTTP handlers for `/api/native/v1/input/decks`, `/api/native/v1/input/decks/{id}` (`.layer(DefaultBodyLimit::disable())` + `to_bytes(..., 64 * 1024)` returning JSON `ErrorCode::TooLarge` HTTP `413`), and `/api/native/v1/input/decks/active`; atomic disk persistence under `config_dir/input-decks/`, `09-host-hub-sync` profile bridge. |
| `client/protocol/policy/src/controller_profile.rs` `[verify post-sprint code-map.md]` | `nightjar-policy` | ~520 | Shared sans-IO layered controller profile engine (`Global → Per-Controller-Model → Per-Game`), stick deadzone/outer-ring/response-curve math (Linear/Aggressive/Relaxed/Wide/Custom), trigger thresholds, gyro/flick-stick/calibration math, JSON import/export schema, and flat-key legacy migration mapping. |
| `client/protocol/policy/src/input_deck.rs` `[verify post-sprint code-map.md]` | `nightjar-policy` | ~480 | Shared sans-IO deck evaluator used by BOTH Android and Desktop: chord matcher, back-paddle (`L4/R4/L5/R5`) & Share/Capture bindings, hold/toggle layer stack, radial sector geometry (deadzone + hysteresis + wedge index), safe release-all state emitter. |
| `client/protocol/client/src/control/input_deck.rs` `[verify post-sprint code-map.md]` | `nightjar-core` | ~180 | Control stream send/receive glue + `0x5500_0081` input queue emitter. |
| `client/protocol/desktop/src/stream/input_deck_hud.rs` `[verify post-sprint code-map.md]` | `nightjar-desktop` | ~360 | Blur-free HUD radial overlay + layer pill drawn in egui only while open; SDL gyro/gamepad bridge (gates Linux-only code with `target_os = "linux"`). |
| `client/protocol/desktop/src/ui/controller_editor/mod.rs` `[verify post-sprint code-map.md]` | `nightjar-desktop` | ~280 | Flagship Desktop Visual Controller Editor shell: profile layer switcher (`Global / Model / Game`), import/export JSON, Hub Sync indicator, studio tab routing. |
| `client/protocol/desktop/src/ui/controller_editor/diagram.rs` `[verify post-sprint code-map.md]` | `nightjar-desktop` | ~420 | Live interactive controller diagram (DualSense, Xbox Series/Elite, Switch Pro / Switch 2 Pro, Steam Controller 2026, Steam Deck/Handheld, Generic) with real-time stick/trigger/button/paddle/trackpad/gyro highlights & direct-focus button/paddle/chord binder. |
| `client/protocol/desktop/src/ui/controller_editor/sticks_triggers.rs` `[verify post-sprint code-map.md]` | `nightjar-desktop` | ~360 | Sticks & Triggers Studio: live circular/axial deadzone & outer-ring canvas, response curve preview, hair-trigger actuation, DualSense adaptive trigger feel. |
| `client/protocol/desktop/src/ui/controller_editor/gyro_studio.rs` `[verify post-sprint code-map.md]` | `nightjar-desktop` | ~340 | Gyroscope Studio: live orientation visualizer, activation trigger, output mode (Gyro-to-Mouse / Gyro-to-Stick / Native Motion / DSU), per-axis sensitivity/invert/smoothing/flick-stick, 1-tap Calibrate Zero. |
| `client/protocol/desktop/src/ui/controller_editor/radials_haptics.rs` `[verify post-sprint code-map.md]` | `nightjar-desktop` | ~360 | Trackpads & 4–12-slice Radial Menu Builder + Haptics & Lightbar Studio (live test pulse, audio-to-haptics toggle/gain, LED/lightbar color). |
| `client/protocol/desktop/src/strings_70/<code>.rs` (14 locale files) `[verify post-sprint code-map.md]` | `nightjar-desktop` | ~85/locale | 7.0.0 Desktop translations for Visual Controller Editor & Input Deck across all 14 non-English locales, keeping grandfathered `strings/*.rs` <= 1,750 LOC. |
| `client/protocol/android/src/input_deck.rs` `[verify post-sprint code-map.md]` | `nightjar-android` | ~260 | JNI bridge exposing `nightjar-policy::controller_profile` + `input_deck` evaluators + `0x554D` / `0x5500_0081` senders so Android & Desktop share one engine. |
| `client/app/src/main/java/com/limelight/binding/input/deck/InputDeckEngine.java` `[verify post-sprint code-map.md]` | Android `app` | ~300 | Wraps JNI `controller_profile` + `input_deck` policy, listens to `ControllerHandler` hook, Android IMU/SensorManager gyro, and focus/lifecycle release-all. |
| `client/app/src/main/java/com/limelight/binding/input/deck/InputDeckRadialView.java` `[verify post-sprint code-map.md]` | Android `app` | ~340 | Custom `View` rendering radial wheel strictly blur-free (`blur: 0`, `shadowBlur: 0`, `GONE` with zero invalidations when closed). |
| `client/app/src/main/java/com/limelight/binding/input/editor/ControllerEditorActivity.java` `[verify post-sprint code-map.md]` | Android `app` | ~340 | Flagship Android Visual Controller Editor host (reachable from Settings → Controls, Game Page, and Console Control Center; 100% controller, TV-remote, keyboard, touch, TalkBack navigable). |
| `client/app/src/main/java/com/limelight/binding/input/editor/ControllerDiagramView.java` `[verify post-sprint code-map.md]` | Android `app` | ~450 | Interactive live-telemetry diagram adapting to DualSense, Xbox Series/Elite, Switch Pro / Switch 2 Pro, Steam Controller 2026, Handheld, Generic, and Touch Studio mode. |
| `client/app/src/main/java/com/limelight/binding/input/editor/SticksTriggersStudioFragment.java` `[verify post-sprint code-map.md]` | Android `app` | ~380 | Interactive circular/axial deadzone & outer-ring canvas, response curve preview, trigger actuation, DualSense adaptive trigger feel. |
| `client/app/src/main/java/com/limelight/binding/input/editor/GyroStudioFragment.java` `[verify post-sprint code-map.md]` | Android `app` | ~360 | Live gyro visualizer, activation trigger, output modes, per-axis sensitivity/invert/smoothing/flick-stick, 1-tap Calibrate Zero. |
| `client/app/src/main/java/com/limelight/binding/input/editor/RadialsHapticsStudioFragment.java` `[verify post-sprint code-map.md]` | Android `app` | ~360 | Trackpads, 4–12-slice Radial Menu Builder, Haptics live test pulse, audio-to-haptics gain, lightbar color. |
| `client/app/src/main/java/com/limelight/binding/input/editor/TouchStudioTabBridge.java` `[verify post-sprint code-map.md]` | Android `app` | ~280 | Seamless Touch Controller Studio (`#151`) tab inside Controller Editor (freely resizable/reshapable controls, separable ABXY/D-pad, movable floating micro-toolbar, 0%-opacity stealth mode). |
| `client/app/src/main/java/com/limelight/binding/input/editor/ControllerProfileMigrator.java` `[verify post-sprint code-map.md]` | Android `app` | ~260 | One-shot lossless migration of legacy flat controller `SharedPreferences` keys into the initial `Global` controller profile, verified by `SettingsKeysTest`. |

### Driver & hardware architecture boundaries

- **First-party VHF gamepad only for virtual pad slots:** `deck_router.rs` inspects `Backend` (`rust/windows/src/input/gamepad_backend.rs`). When `Backend::Vhf` (`Root\RubylightVirtualGamepad`) is active, virtual buttons, analog sticks, triggers, and profile switches (`Profile::XboxOne`..`SteamController`) execute directly on the slot's VHF handle (`16` concurrent slots). When the user explicitly forced `gamepad = vigem_x360` or `vigem_ds4`, virtual buttons/axes compatible with Xbox 360 / DS4 still work, while an incompatible `ProfileSwitch` returns `HostError::ProfileUnsupported` visibly (never silently switching drivers).
- **Virtual HID mouse & keyboard:** Relative gyro-to-mouse deltas and radial mouse buttons route through `rust/windows/src/input/virtual_hid.rs` (`MOUSE: Slot<HidMouse>` over signed `usbip-win2`) when `virtual_mouse` is active, or existing `Injector` (`SendInput`) otherwise, with a one-time stream warning if a game rejects `SendInput` and `usbip-win2` is not installed. Keyboard chords route through `Injector::key_scan` under `SEND_ORDER` + `HELD` so multi-client refcounts stay balanced.
- **No GPU / encoder / capture coupling:** Input Deck runs exclusively on the control/input/gamepad threads (`control`, `gamepads`) and never touches capture, compute shaders, AMD AMF, or NVIDIA NVENC. Both AMD AMF and NVIDIA NVENC, Windows 10, physical displays, WGC, Desktop Duplication, and D3D11 remain 100% unaffected.

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### Client & Host Input Deck state machine

```text
Client:
  Disabled ──(rlInputDeck=1 & HostAck)──▶ Armed(base_layer=0)
  Armed ──(radial_hold_down)──▶ RadialOpen(menu_id) ──(release/cancel)──▶ Armed
  Armed ──(layer_hold/toggle)──▶ LayerActive(stack) ──(release/pop)──▶ Armed
  AnyActive ──(focus_lost | pause | disconnect | menu_open)──▶ Releasing ──▶ Disarmed
     └─ emits 0x554D ReleaseAll(scope=all) + zeroed 0x5500_0081 frame locally & over wire

Host (per session, inside stream/input_deck.rs):
  Inactive ──(launch rlInputDeck=1)──▶ Active { layer_stack: [0], held_keys, held_mouse, held_pad[16], slot_profile_override }
  Active ──(0x554D / 0x5500_0081)──▶ Validate → Apply → Update DeckHoldTracker → Refresh 15s watchdog
  Active ──(ReleaseAll | watchdog_15s | ENet_disconnect | session_teardown)──▶ ReleaseAllHolds → Inactive
```

### Safety invariants (zero stuck keys/buttons)

1. **Client-side focus loss & menu guard:** Opening the Nightjar Stream Menu, switching OS windows (`SDL_EVENT_WINDOW_FOCUS_LOST`), Android `onWindowFocusChanged(false)` / `onPause()`, or Bluetooth controller disconnect immediately runs `InputDeckEvaluator::panic_release()` → closes any radial HUD, resets all held layers to `[0]`, sends `0x554D ReleaseAll` + a zeroed `0x5500_0081` packet (sticks 0, triggers 0, `buttons_mask = 0`), and suppresses deck input until focus returns and held triggers are physically released.
2. **Host-side authoritative hold tracker (`DeckHoldTracker`):** Every key, mouse button, and VHF gamepad button/axis activated by Input Deck is recorded in `DeckHoldTracker` per session. On ENet peer drop, session pause, guest kick, or `session_teardown`, `DeckHoldTracker::drain_releases()` deterministically emits key-up (`Injector`), mouse-button-up (`virtual_hid::MOUSE` / `Injector`), and zeroed VHF stick/trigger/button state (`GamepadThread`) — even if the client crashed without sending `ReleaseAll`.
3. **Hold watchdog:** If a client holds a deck key/button/axis continuously for 15 seconds with zero incoming control pings or input packets (silent half-open link), the host triggers `drain_releases()` and logs a `WARN` without waiting for full ENet timeout.
4. **Secure desktop / UAC boundary:** When `rust/windows/src/input/desktop.rs::secure_desktop_shown()` is true, `deck_router.rs` releases all active deck holds immediately and answers any new `ActionTrigger` with `HostError::SecureDesktopBlocked` so chords never queue up and misfire after UAC dismissal.

### Reconnect & multi-client matrix

| Scenario | Expected behavior |
| --- | --- |
| **Same-device resume / brief roam** | Old ENet peer teardown runs `drain_releases()` (no stuck keys during drop). Reconnecting peer re-sends `0x554D LayerSync([0])` + active `ProfileSwitch`; host restores slot profile cleanly. |
| **Continue Here (different device takes over)** | Previous owner's `DeckHoldTracker` drains on transfer. New owner loads its own device deck (or shared `09-host-hub-sync` per-game deck) and sends `LayerSync` upon stream start. |
| **Couch guest (`multiclient.rs`)** | Guests with `rlInputDeck=1` may only drive their own assigned controller slot (`slot == guest_slot`). Keyboard/mouse deck actions (`slot == 0xFF`) and other players' slots are refused with `HostError::GuestSlotReadOnly`. |
| **VHF driver handle lost (`DriverLost`, `#138`)** | Existing `gamepad_thread.rs` reopens the VHF handle; `input_deck.rs` receives the reopen callback, clears transient button holds, and re-applies any active `ProfileSwitch` override once the slot comes back. |
| **Host restart / crash mid-stream** | VHF child HID instances close automatically when the host process handle closes (`rubylight-vgamepad` owner cleanup on handle close), clearing all gamepad holds; OS releases `SendInput` keys; next host startup reads `session.json` and resets any persisted temporary slot profile override. |

### Crash-restore journal

- **Persistent deck library:** Stored in `config_dir/input-decks/<device_uuid>/<deck_id>.json` (plus `shared/<app_id>.json` synced via `09-host-hub-sync`), written atomically via temp file + `sync_all()` + rename so power loss never corrupts saved decks.
- **Transient session journal:** If a session applies a temporary per-game VHF profile override via `0x554D ProfileSwitch`, `session_control.rs` records `input_deck_profile_override: Option<(u8, u16)>` inside `session.json` (`session_control::JOURNAL`). On teardown or post-crash boot cleanup, the host clears the transient override so subsequent stock Moonlight sessions start with the configured `gamepad` profile.

## 5. Settings & 10/10 Craft Plan

### Fewer-settings compliance & flat-key migration

- **Dramatic settings simplification:** Consolidates dozens of legacy flat controller settings (dead zones, stick inversion, face-button swaps, trigger thresholds, gyro sensitivity/axes, rumble/haptics sliders, per-model overrides, touch layout flags) into **ONE Visual Controller Editor** opened from **Settings → Controls**, any **Game Page**, or the in-stream **Console Control Center**!
- **Lossless key migration (`SettingsKeysTest` + `config.rs::migrate`):** On first 7.0 launch, `ControllerProfileMigrator` (Android) and `config.rs::migrate` (Desktop) read every existing flat controller preference key into the user's initial `Global` profile, clear/retire the legacy flat preference rows in `preferences.xml` / `ui/settings.rs`, and add every retired key to `SettingsKeysTest` so retired names are never reused and no user loses calibration.
- **One folded host policy toggle:** `input_deck` (bool, default `true`, added to `SETTINGS_30` + `LIVE` + `Config::input_deck()` in `rust/core/src/config/settings30.rs`, folded under `category: 'input'` in Console `rust/web/src/lib/schema/extensions.ts` and `rust/host/src/console/settings.rs`). Updating `rust/core/src/config/settings30.rs` explicitly updates its hardcoded 12-entry test assertions (`there_are_12_unique_settings`, `answers()`, `the_defaults_are_the_documented_values`, and `the_console_schema_lists_exactly_these_settings_with_these_defaults`, which parses `rust/web/src/lib/schema/extensions.ts` via `include_str!`). Two-sentence help: *"Allows Nightjar devices to run custom radial menus, action layers, and virtual controller bindings during a stream. Turn off to accept only standard controller, keyboard, and mouse input."*

### Visual craft, zero stream-time blur & dynamic glyphs

- **Strict blur-free HUD radial rendering:** When a radial menu is open during a stream, it draws **zero blur and zero shadow** using the 7.0 zero-blur HUD token set (`blur: 0`, `shadowBlur: 0`). High-contrast mode switches automatically to opaque `solid` tokens with a crisp rim.
- **Zero overhead when closed:** The radial view is `GONE` on Android and skipped entirely in Desktop `app/overlay.rs` when no radial is active — zero extra render target, zero extra composition layer.
- **Dynamic input glyphs:** Every wedge badge, chord prompt, and Controller Editor hint reads the active input device via canonical `client/protocol/policy/src/input_glyphs.rs` — showing Xbox (`A/B/X/Y`, `LB/RB`), PlayStation (`Cross/Circle/Square/Triangle`, `L1/R1`), Nintendo (`B/A/Y/X`), Keyboard keycaps, or TV Remote (`OK` / `Hold OK`), with **no button hints for Touch or Mouse**, switching immediately on the next input with no animation.
- **Plain-English error states:**
  - VHF driver missing: *"Virtual controller driver is unavailable. Run Rubylight setup with the gamepad driver selected to use virtual gamepad bindings."*
  - ViGEmBus profile refusal: *"This PC is set to ViGEmBus, which cannot switch to a DualSense or Steam Controller layout. Switch the PC's controller driver to Automatic in Rubylight settings."*
  - Secure desktop: *"Windows security prompt is open on the PC. Input Deck bindings are paused until it closes."*
  - Zero "Experimental" anywhere in UI. Zero mod names (no mention of Nucleus Co-op or ANR).
- **15 locales:** All new strings added in lockstep to `rust/assets/locale/` (host), `client/protocol/desktop/src/strings_70/<code>.rs` (all 14 non-English Desktop tables `de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`; English literals live at call sites via `tr`/`trf`/`en`, never growing grandfathered `strings/*.rs` past 1,750 LOC), and `client/app/src/main/res/values*/strings.xml`. Radial wedge labels cap at 2 lines (`type.caption` 12 sp, Figtree 600 for Latin-1 / system font for the 9 non-Latin-1 locales) with safe ellipsis so German compound verbs never clip outside a 12-wedge ring.

## 6. Disjoint Implementer Slices (Phase 3 `parallel()`)

Every file path belongs to **exactly one** slice below (zero overlap across parallel branches):

### Stage 1 — Slice 13A: Protocol Wire, Lockstep Capabilities, Core Logic, Native API Types, Schema Lockstep & Shared Policy
- `client/protocol/core/src/ids.rs` `[verify post-sprint code-map.md]`
- `client/protocol/core/src/lib.rs` `[verify post-sprint code-map.md]`
- `client/protocol/core/src/input_deck.rs` *(new)* `[verify post-sprint code-map.md]`
- `client/protocol/core/src/fuzz.rs` `[verify post-sprint code-map.md]`
- `client/protocol/client/src/http/capabilities.rs` *(adds `Feature::InputDeck` to `Feature`, `Feature::ALL`, `Evidence`, `with_native`)* `[verify post-sprint code-map.md]`
- `client/protocol/client/tests/capabilities_contract.rs` *(adds `Feature::InputDeck` to exhaustive `feature_name` match)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java` *(adds `INPUT_DECK(0)`)* `[verify post-sprint code-map.md]`
- `client/app/src/test/resources/protocol/capabilities/*` *(recorded capability fixtures for `InputDeckVersion` + `input.deck`)* `[verify post-sprint code-map.md]`
- `rust/core/native-api/src/lib.rs` `[verify post-sprint code-map.md]`
- `rust/core/native-api/src/input_deck.rs` *(new)* `[verify post-sprint code-map.md]`
- `rust/core/native-api/tests/fixtures/input_deck.json` *(new)* `[verify post-sprint code-map.md]`
- `rust/core/native-api/tests/fixtures.rs` `[verify post-sprint code-map.md]`
- `rust/core/src/lib.rs` `[verify post-sprint code-map.md]`
- `rust/core/src/input_deck.rs` *(new)* `[verify post-sprint code-map.md]`
- `rust/core/src/input.rs` `[verify post-sprint code-map.md]`
- `rust/core/src/extensions.rs` `[verify post-sprint code-map.md]`
- `rust/core/src/config/settings30.rs` *(adds `input_deck` to `SETTINGS_30`, `LIVE`, `Config::input_deck()`, and updates the hardcoded 12-entry test assertions: `there_are_12_unique_settings`, `answers()`, `the_defaults_are_the_documented_values`, and `the_console_schema_lists_exactly_these_settings_with_these_defaults`)* `[verify post-sprint code-map.md]`
- `rust/web/src/lib/schema/extensions.ts` *(updated in lockstep with `settings30.rs`'s `include_str!` schema assertion)* `[verify post-sprint code-map.md]`
- `client/protocol/policy/src/lib.rs` `[verify post-sprint code-map.md]`
- `client/protocol/policy/src/controller_profile.rs` *(new)* `[verify post-sprint code-map.md]`
- `client/protocol/policy/src/input_deck.rs` *(new)* `[verify post-sprint code-map.md]`

### Stage 2 — Slice 13B: Host Runtime & Windows VHF/HID Router *(depends on 13A merged)*
- `rust/windows/src/input.rs` `[verify post-sprint code-map.md]`
- `rust/windows/src/input/deck_router.rs` *(new)* `[verify post-sprint code-map.md]`
- `rust/windows/src/input/gamepad_thread.rs` `[verify post-sprint code-map.md]`
- `rust/windows/src/input/gamepads.rs` `[verify post-sprint code-map.md]`
- `rust/host/src/extensions.rs` `[verify post-sprint code-map.md]`
- `rust/host/src/console/settings.rs` `[verify post-sprint code-map.md]`
- `rust/host/src/stream/control.rs` `[verify post-sprint code-map.md]`
- `rust/host/src/stream/input_deck.rs` *(new)* `[verify post-sprint code-map.md]`
- `rust/host/src/native_api.rs` *(registers `/api/native/v1/input/decks`, `/api/native/v1/input/decks/{id}` with `.layer(DefaultBodyLimit::disable())` + handler `axum::body::to_bytes(req.into_body(), 64 * 1024)` so `> 64 KiB` returns JSON `ErrorBody` `ErrorCode::TooLarge` HTTP `413` rather than plain-text `LengthLimitError`, `/api/native/v1/input/decks/active`, and `feature::INPUT_DECK`)* `[verify post-sprint code-map.md]`
- `rust/host/src/native_api/input_deck.rs` *(new)* `[verify post-sprint code-map.md]`
- `rust/host/src/stream/reconnect_tests/input_deck_tests.rs` *(new)* `[verify post-sprint code-map.md]`
- `rust/host/src/nvhttp/native_tests/input_deck.rs` *(new)* `[verify post-sprint code-map.md]`

### Stage 2 — Slice 13C: Nightjar Core Engine & Desktop Client *(depends on 13A merged, parallel with 13B/13D/13E)*
- `client/protocol/client/src/http/native.rs` `[verify post-sprint code-map.md]`
- `client/protocol/client/src/control/input_deck.rs` *(new)* `[verify post-sprint code-map.md]`
- `client/protocol/client/src/control/driver.rs` `[verify post-sprint code-map.md]`
- `client/protocol/client/src/control/messages.rs` `[verify post-sprint code-map.md]`
- `client/protocol/client/src/control/events.rs` `[verify post-sprint code-map.md]`
- `client/protocol/client/src/input/queue.rs` `[verify post-sprint code-map.md]`
- `client/protocol/client/tests/control_messages_contract.rs` `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/gamepad.rs` `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/stream/mod.rs` `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/stream/input_deck_hud.rs` *(new)* `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/ui/controller_editor/mod.rs`, `diagram.rs`, `sticks_triggers.rs`, `gyro_studio.rs`, `radials_haptics.rs` *(new, all `<= 800` LOC)* `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/ui/settings.rs` *(shrinks: replaces flat controller rows with single Controller Editor entry)* `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/config.rs` *(lossless migration of legacy flat controller keys into initial `Global` profile)* `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/strings_70/<code>.rs` *(all 14 non-English 7.0.0 Desktop locale tables; never growing grandfathered `strings/*.rs` past 1,750 LOC)* `[verify post-sprint code-map.md]`

### Stage 2 — Slice 13D: Nightjar Android Client *(depends on 13A merged, parallel with 13B/13C/13E)*
- `client/protocol/android/src/lib.rs` `[verify post-sprint code-map.md]`
- `client/protocol/android/src/input_deck.rs` *(new)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/nvstream/jni/RustBridge.java` `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/binding/input/ControllerHandler.java` *(4-line hook only; never grows)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/preferences/StreamSettings.java` *(shrinks: replaces flat controller rows with single Controller Editor entry)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/binding/input/deck/InputDeckEngine.java` *(new)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/binding/input/deck/InputDeckRadialView.java` *(new)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/binding/input/editor/ControllerEditorActivity.java`, `ControllerDiagramView.java`, `SticksTriggersStudioFragment.java`, `GyroStudioFragment.java`, `RadialsHapticsStudioFragment.java`, `TouchStudioTabBridge.java`, `ControllerProfileMigrator.java` *(new, all `<= 800` LOC)* `[verify post-sprint code-map.md]`
- `client/app/src/main/res/values*/strings.xml` `[verify post-sprint code-map.md]`
- `client/app/src/test/java/com/limelight/binding/input/deck/InputDeckEngineTest.java`, `ControllerProfileMigratorTest.java`, and `client/app/src/test/java/com/limelight/preferences/SettingsKeysTest.java` *(verifies lossless flat-key migration & retired key hygiene)* `[verify post-sprint code-map.md]`

### Slice 13E: Host Locales & Documentation *(depends on 13A merged, parallel with 13B/13C/13D)*
- `rust/assets/locale/` *(host locale entries)* `[verify post-sprint code-map.md]`
- `docs/control-messages.md` `[verify post-sprint code-map.md]`
- `docs/native-api.md` `[verify post-sprint code-map.md]`
- `docs/configuration.md` `[verify post-sprint code-map.md]`
- `rust/release/notes.d/7-0-input-deck.md` *(new)* `[verify post-sprint code-map.md]`

## 7. Verification & Measurement Plan

### Narrowest unit, property, and interop tests

1. **Wire & fuzz:** `cargo test -p rubylight-protocol ids input_deck fuzz` — verifies `0x554D` and `0x5500_0081` uniqueness, bit-15 clear (`HAPTICS_CAPABILITY`), `0x554F` still free, zero panics on random bytes, and exact encode/decode round-trips.
2. **Core & Policy:** `cargo test -p butterpollo-core input_deck settings30` and `cargo test -p nightjar-policy input_deck` — validates radial deadzone/hysteresis, chord suppression, layer push/pop, `DeckHoldTracker::drain_releases()`, ViGEmBus profile refusal, and the updated `settings30` entry count + `rust/web/src/lib/schema/extensions.ts` lockstep check.
3. **Native API & Capabilities:** `cargo test -p rubylight-native-api`, `cargo test -p nightjar-core --test capabilities_contract --test control_messages_contract`, and `cargo test -p butterpollo nvhttp::native_tests::input_deck` (explicitly verifying `PUT /api/native/v1/input/decks/{id}` accepts valid 16–64 KiB bodies under `.layer(DefaultBodyLimit::disable())` + `axum::body::to_bytes(req.into_body(), 64 * 1024)` and rejects > 64 KiB with `ErrorCode::TooLarge` / `"too_large"` / HTTP `413`).
4. **Loopback & Interop:** `cargo test -p butterpollo stream::reconnect_tests::input_deck_tests` and `interop_tests` — verifies:
   - Stock client without `rlInputDeck=1` sees byte-identical serverinfo/DESCRIBE and dropped unsolicited `0x554D`/`0x5500_0081`.
   - Disconnect mid-chord releases all held keys, mouse buttons, and VHF gamepad buttons deterministically.
   - Guest device cannot fire keyboard/mouse deck actions or hijack Player 1's slot.
5. **Android unit & Robolectric:** `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.binding.input.deck.InputDeckEngineTest --tests com.limelight.nvstream.jni.MoonBridgeControlMessagesTest`.
6. **Architecture & size ratchet:** `python tools/arch_check.py` (including grandfathered `client/protocol/desktop/src/strings/*.rs <= 1,750` LOC) and `python tools/code_map.py check`.

### Visual screenshot checks

- **Desktop (`shot.rs`):** Rasterize 8-wedge and 12-wedge radial HUDs (Night, High-Contrast, German long labels, Japanese CJK) plus the Input Deck editor page; verify zero blur pixels on the HUD backdrop, >=4.5:1 label contrast, and dynamic Xbox/PS/Keyboard glyphs.
- **Android (`emulator-smoke.py`):** Open the demo-stream radial HUD via simulated pad chord and touch hold at `393dp-font-1`, `393dp-font-2`, and `393dp-font-2-landscape`; verify safe-area clearance, zero extra blurred window, and clean removal on release.

### Serialized idle-gated hardware criteria (RX 7900 XT + Radeon 780M + Android)

- **Idle gate before any GPU/driver run:** Verify `http://127.0.0.1:47989/serverinfo` shows `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0`. Use isolated test host: `butterpollo.exe --config-dir C:\tests\input-deck --port 48124 --bind 127.0.0.1`.
- **Exit code 3 abort rule:** Exit code `3` from `rust/release/e2e.py` or `check.ps1` signals an AMD encoder hang — **abort immediately and investigate; never retry**.
- **Driver verification (`#[ignore]` hardware tests):** Run `input_deck_vhf_native_tests` against installed `Root\RubylightVirtualGamepad` confirming live profile switching (`XboxSeries` → `DualSense` → `SteamController`), 500 Hz `0x5500_0081` axis injection, and 0 stuck inputs after forced handle close / `ReleaseAll`.
- **Latency acceptance gate:** ABBA 90 s `e2e.py` run (`BUTTERPOLLO_TIMING_RECORD=1`):
  - Deck disabled (default): `present_to_sent p99` delta `<= +0.05 ms` (within noise); `gamepad timings` unchanged.
  - Deck active with 250 Hz gyro-to-stick + radial open/close: `present_to_sent p99` delta `<= +0.15 ms`, zero `control` thread stalls in `stall_watch.rs`, radial open-to-first-frame `<= 1 frame`, and post-close client GPU frame time matches pre-open baseline.
- **NVIDIA note:** NVIDIA NVENC code paths are untouched by Input Deck; any verification on NVIDIA remains header/sample-checked only and is never claimed as run on NVIDIA hardware.
