# Item 26: Physical Steam Controller (2026) Direct HID Driver — Desktop & Android + Host `vhf_steam` Polish (Wave 3 — PR 5: Track 6)

Status: engineering design (Phase 1). Written against `origin/main` `995f2540c8` (`6.3.0`), [`00-protocol-reservations.md`](00-protocol-reservations.md), [`13-client-input-deck.md`](13-client-input-deck.md), and [`24-usb-forwarding-convenience.md`](24-usb-forwarding-convenience.md).

## 1. Summary & User Experience

Item 26 delivers **PR 5 (Track 6)** — native, zero-translation direct HID support for the physical **Steam Controller (2026)** over USB (`VID 0x28DE : PID 0x1302`) and its wireless puck (`VID 0x28DE : PID 0x1304`) across **Nightjar Desktop (Windows, Linux, macOS)** and **Nightjar Android**, paired seamlessly with Rubylight's first-party UMDF2 `vhf_steam` virtual Steam Controller profile (`rust/drivers/rubylight-vgamepad`), Item `13`'s Visual Controller Editor, and Item `24`'s Track 8C USB-forwarding arbitration:

1. **Direct Physical HID Drivers (Desktop `SDL_hid_*`/hidapi & Android `UsbManager`):**
   - Binds directly to `28de:1302` (USB) and `28de:1304` (wireless puck) on Desktop (`client/protocol/desktop/src/steam_controller_2026.rs`) and Android (`client/app/src/main/java/com/limelight/binding/input/driver/SteamController2026Driver.java`).
   - Advertises `LI_CTYPE_STEAM` (`0x04`) along with the full Steam Controller 2026 capability mask: `STEAM_HAPTIC`, `STEAM_SETTINGS`, `STEAM_ATTRIBUTES` (`0x4000` on `type == 0x04` — strictly distinct from PlayStation `0x4000` in Item `25`!), `DUAL_TOUCHPAD`, `GRIP_SENSE`, `STICK_TOUCH`, and `SC26_BUTTONS`.
   - Sends **`0x5500_0008`** (`steam_controller::STEAM_ATTRIBUTES_MAGIC`) firmware attributes immediately on controller connect so the host's `vhf_steam` virtual device mirrors exact physical hardware attributes.
   - Routes host-to-client **`0x5504`** (`ControlEvent::SteamHaptic` / `MoonBridge`) and **`0x5505`** (`ControlEvent::SteamSettings` / `MoonBridge`) directly back to the physical Steam Controller (2026) via HID output/feature reports!
2. **Switch 2 C-Button vs. `LEFT_GRIP_TOUCH_FLAG` Guard (`0x40_0000`):**
   - Explicitly preserves and unit-tests the `0x40_0000` button-flag disambiguation guard so Switch 2 Pro's C-button and Steam Controller 2026's `LEFT_GRIP_TOUCH_FLAG` never cross-trigger across controller types!
3. **Track 8C USB-Forwarding Arbitration Integration (Depends on Item `24`):**
   - Registers with Desktop `usb_arbitration.rs` and Android `UsbDriverService` / `UsbForwarding` precedence so if a user chooses to USB-forward their `28de:1302`/`28de:1304` dongle directly to the host, the local HID driver yields cleanly on attach and resumes automatically on detach/stream end.
4. **Host `vhf_steam` Polish (`rust/windows/src/input/gamepads.rs` & `gamepad_thread.rs`):**
   - Raises `vhf_steam` motion reporting rate from **`200 Hz` -> `250 Hz`** for crisper Steam Input gyro response.
   - Logs a compact **12-byte per-slot feedback counter summary** at controller unplug (zero hot-path logging during play!) to aid haptic/settings diagnostics.
5. **Visual Controller Editor Parity (Depends on Item `13`):**
   - Exposes `28de:1302`/`28de:1304` live telemetry (dual trackpads, grip sense `L4/R4/L5/R5`, stick touch, gyro) seamlessly inside Item `13`'s interactive `Steam Controller 2026` diagram.

---

## 2. Protocol, Native API & Capability Contract

Per [`00-protocol-reservations.md`](00-protocol-reservations.md) and `client/protocol/core/src/ids.rs`:
- Uses existing registered IDs: `0x5504` (`STEAM_HAPTIC_MESSAGE_TYPE`), `0x5505` (`STEAM_SETTINGS_MESSAGE_TYPE`), and `0x5500_0008` (`STEAM_ATTRIBUTES_MAGIC`). **Zero new wire IDs, zero `PROTOCOL_VERSION` or `API_VERSION` changes.**
- Strictly capability-gated on host Steam Controller support (never `Family::Rubylight`).
- Preserves `0x4000` on `type == 0x04` (`LI_CCAP_STEAM_ATTRIBUTES`) vs `0x4000` on `type == 0x02` (PlayStation speaker in Item `25`) and `0x40_0000` Switch 2 C-button vs `LEFT_GRIP_TOUCH_FLAG`.

---

## 3. Architecture & <=800-LOC Module Plan

All target modules below are conceptual anchors — **resolve exact post-sprint file paths, split submodules, and `tools/size_ratchet.json` ceilings live via `docs/code-map.md` and `client/docs/code-map.md` before assigning implementer slices** (`[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]`). Every new `.rs`/`.java` module stays strictly `<= 800` LOC with `//!` / `/**` headers; grandfathered files MUST NOT grow past their live post-sprint ceilings.

| Path | Status | Est. LOC | Responsibility |
| --- | --- | ---: | --- |
| `client/protocol/policy/src/sc26_hid.rs` | New | ~340 | `//!` Pure platform-independent Steam Controller (2026) (`28de:1302`/`28de:1304`) HID input report decoder, `0x5500_0008` attribute builder, `0x5504`/`0x5505` HID output/feature report encoder, and `0x40_0000` Switch-2-vs-SC26 guard + unit tests. |
| `client/protocol/desktop/src/steam_controller_2026.rs` | New | ~420 | `//!` Desktop `SDL_hid_*` / hidapi driver for `28de:1302` & `28de:1304` (Win/Linux/macOS), routing `0x5504`/`0x5505` back to hardware and honoring `usb_arbitration.rs` yield/resume. |
| `client/protocol/desktop/src/gamepad.rs`, `stream/mod.rs` | Modify | `<= 1375` / `<= 2917` | Thin registration hook into `steam_controller_2026.rs` and `ControlEvent::{SteamHaptic, SteamSettings}` forwarding. |
| `client/app/src/main/java/com/limelight/binding/input/driver/SteamController2026Driver.java` | New | ~440 | `/**` Android `UsbManager` direct HID driver for `28de:1302` & `28de:1304`, honoring Track 8C `UsbForwarding` arbitration and sending `0x5500_0008` on connect. |
| `client/app/src/main/java/com/limelight/binding/input/driver/UsbDriverService.java`, `nvstream/jni/MoonBridge.java`, `client/protocol/android/src/bridge.rs`, `SteamController2026DriverTest.java` (new) | Modify / New | `<= ceiling` / ~240 | Route `0x5504`/`0x5505` from `nightjar-android` through `MoonBridge` to `SteamController2026Driver` + unit tests verifying `0x40_0000` guard & arbitration. |
| `rust/windows/src/input/gamepads.rs`, `rust/windows/src/input/gamepad_thread.rs` | Modify | `<= 1788` / `<= 1377` | Raise `vhf_steam` motion rate `200 Hz -> 250 Hz` and log compact 12-byte per-slot feedback counters on unplug (zero hot-path log noise). |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

- **Attach / USB-Forwarding Arbitration:** `PhysicalConnected(28de:1302|1304)` -> check `usb_arbitration` / `UsbForwarding` -> if USB-forwarded, yield handle immediately; else claim HID -> send `0x5500_0008` attributes -> send `LI_CTYPE_STEAM` arrival -> stream input & receive `0x5504`/`0x5505` -> on unplug/detach/stream-end, close HID handle cleanly & host logs 12-byte per-slot feedback counters.
- **Reconnect:** Re-sends `0x5500_0008` attributes before arrival on every reconnect so the host `vhf_steam` slot always receives fresh firmware attributes.
- **Crash-Restore:** Zero host OS state mutated.

---

## 5. Settings & 10/10 Craft Plan

- **Zero new settings** (plug-and-play detection on `28de:1302`/`28de:1304`; full customization lives in Item `13`'s Visual Controller Editor).
- **Zero `"Experimental"` & zero mod names**; preserve upstream HID/protocol credits strictly in code comments, `NOTICE`, and `rust/THIRD_PARTY.md`.

---

## 6. Disjoint Implementer Slices (Phase 3 `parallel()`)

Wave 3 dependency: `depends_on: ["13", "24"]` (Item `13` Visual Controller Editor + Item `24` Track 8C USB arbitration already landed).

| Slice | Stage | Exclusive Owned Files (Zero Overlap) |
| --- | --- | --- |
| **26-A: Shared `nightjar-policy` SC26 HID Codec & Flag Guards** | **Stage 1** | `client/protocol/policy/src/sc26_hid.rs` (new), `client/protocol/policy/src/lib.rs` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |
| **26-B: Desktop `SDL_hid_*` SC26 Driver (Win/Linux/macOS)** | **Stage 2** (parallel with 26-C, 26-D) | `client/protocol/desktop/src/steam_controller_2026.rs` (new), `client/protocol/desktop/src/gamepad.rs`, `client/protocol/desktop/src/stream/mod.rs` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |
| **26-C: Android `UsbManager` SC26 Driver & `MoonBridge` `0x5504`/`0x5505` Routing** | **Stage 2** (parallel with 26-B, 26-D) | `client/app/src/main/java/com/limelight/binding/input/driver/SteamController2026Driver.java` (new), `client/app/src/main/java/com/limelight/binding/input/driver/UsbDriverService.java`, `client/app/src/main/java/com/limelight/nvstream/jni/MoonBridge.java`, `client/protocol/android/src/bridge.rs`, `client/app/src/test/java/com/limelight/binding/input/driver/SteamController2026DriverTest.java` (new) `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |
| **26-D: Host `vhf_steam` 250 Hz Motion & Unplug Feedback Counters** | **Stage 2** (parallel with 26-B, 26-C) | `rust/windows/src/input/gamepads.rs`, `rust/windows/src/input/gamepad_thread.rs` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |

---

## 7. Verification & Measurement Plan

1. **Automated Unit & Mock-HID Tests (No Physical SC26 Hardware Required):**
   - `cargo test -p nightjar-policy --locked sc26_hid` — tests `28de:1302`/`1304` report decoding, `0x5500_0008` attribute encoding, `0x5504`/`0x5505` output/feature encoding, and `0x40_0000` Switch 2 C-button vs `LEFT_GRIP_TOUCH_FLAG` isolation.
   - `cargo test -p nightjar-desktop --locked steam_controller_2026` — verifies mock HID attach/detach, `0x5504`/`0x5505` delivery, and Track 8C `usb_arbitration` yield/reclaim.
   - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.binding.input.driver.SteamController2026DriverTest --tests com.limelight.nvstream.jni.MoonBridgeControlMessagesTest`
   - `cargo test -p butterpollo-windows --locked gamepads gamepad_thread` — verifies `vhf_steam` 250 Hz motion interval and 12-byte per-slot unplug feedback counter summary.
   - **Hardware Honesty:** Physical `28de:1302`/`28de:1304` hardware testing is recorded explicitly as mock-HID/unit-verified unless physically run on Steam Controller (2026) hardware.
   - `python tools/arch_check.py` & `cargo fmt -- --check` (without `--all`).
