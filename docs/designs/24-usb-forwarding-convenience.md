# Item 24: USB Forwarding Convenience — Windows `usbipd-win` Backend, Linux `EACCES` Fallback & Controller-Driver Arbitration (Wave 2 — PR 3: Track 8, Client-Only)

Status: engineering design (Phase 1). Written against `origin/main` `995f2540c8` (`6.3.0`) and [`00-protocol-reservations.md`](00-protocol-reservations.md).

## 1. Summary & User Experience

Item 24 delivers **PR 3 (Track 8)** — making Nightjar's USB forwarding effortless across **Windows, Linux, and Android** with **ZERO host protocol changes** (100% client-side, feeding into the existing `0x553A` TLS USB forwarding tunnel):

1. **Track 8A — Windows `usbipd-win` Backend (`client/protocol/desktop/src/usb_windows.rs`):**
   - Sits cleanly alongside existing `usb_winusb.rs` without removing WinUSB fallback.
   - Detects an installed `usbipd-win` service (`usbipd.exe` / loopback port `3240`) so Windows users can forward **non-WinUSB and isochronous devices** (`HidUsb`, `USBSTOR`, webcams, USB audio interfaces, Bluetooth dongles, fightsticks, flight sticks) **without running Zadig**!
   - **Loopback-Only Security Invariant:** Connects strictly to **`127.0.0.1:3240`** (never a remote or non-loopback address), executes USB/IP `OP_REQ_IMPORT` (`0x8003`) -> `OP_REP_IMPORT` (`0x0003`), verifies exact **`vid:pid` AND `busid` match** against the user's selected device, and hands the post-import TCP stream straight into Nightjar's existing `UsbDevice::relay()` + `0x553A` TLS tunnel.
   - **Crash-Safe Ownership Journal (`usbipd-journal.json`):** Records **ONLY** devices that Nightjar itself bound via `usbipd bind`. On **Detach**, **stream end**, or **next-launch crash recovery**, Nightjar unbinds ONLY journaled busids to restore their original Windows drivers — **never** unbinding devices the user had already shared via `usbipd-win`!
   - When `usbipd-win` is absent and WinUSB cannot open a device, falls back to `usb_winusb.rs` and shows crisp, actionable guidance in the USB device picker.

2. **Track 8B — Linux `EACCES` Automatic Fallback (`client/protocol/desktop/src/usb.rs`):**
   - When `libusb` returns `rusb::Error::Access` (`EACCES`) on a non-isochronous device AND `usb_linux::System::real().missing().is_none()`, Nightjar automatically falls back cleanly to `usb_linux` (`nightjar-usb-helper` + `pkexec` + `usbip-host`) instead of telling the user to hand-author a udev rule!

3. **Track 8C — Android & Desktop Controller-Driver vs. USB-Forwarding Arbitration:**
   - **Android (`UsbForwarding.java`, `UsbDriverService.java`):** Devices opted into USB forwarding (including the Automatic list) take precedence over `UsbDriverService.shouldClaimDevice()`. On attach, Nightjar cleanly pauses/releases the local USB controller driver so it doesn't fight `UsbForwarding` for the interface, and reclaims local control automatically on detach or stream end.
   - **Desktop (`usb.rs` + `gamepad.rs`, coordinated cleanly with Item `26` Steam Controller 2026):** Prevents local SDL/HID gamepad opening for any currently forwarded USB gamepad `(vid, pid, bus/port)` during attach, and resumes local gamepad control automatically on detach or stream end.
   - Synchronizes user guides in `docs/usb-forwarding.md` and `client/docs/sharing.md`.

---

## 2. Protocol, Native API & Capability Contract

Per [`00-protocol-reservations.md`](00-protocol-reservations.md) §7, **Item 24 is 100% client-side and requires ZERO host wire, control-stream, or Native API changes.**
- Reuses existing `0x553A` (`usb::USB_MESSAGE_TYPE`) TLS USB forwarding tunnel unchanged.
- Strictly gated on existing `HostCapabilities::Feature::UsbForwarding` (never `Family::Rubylight`).
- All Linux-specific code uses `#[cfg(target_os = "linux")]` (never bare `cfg(unix)` breaking macOS CI!).

---

## 3. Architecture & <=800-LOC Module Plan

All target modules below are conceptual anchors — **resolve exact post-sprint file paths, split submodules, and `tools/size_ratchet.json` ceilings live via `docs/code-map.md` and `client/docs/code-map.md` before assigning implementer slices** (`[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]`). Every new `.rs`/`.java` file stays strictly `<= 800` LOC with `//!`/`/**` headers; grandfathered files MUST NOT grow.

| Path | Status | Est. LOC | Responsibility |
| --- | --- | ---: | --- |
| `client/protocol/desktop/src/usb_windows.rs` | New | ~460 | `//!` `#[cfg(windows)]` `usbipd-win` detection, loopback-only (`127.0.0.1:3240`) `OP_REQ_IMPORT` (`0x8003`) / `OP_REP_IMPORT` (`0x0003`) with exact `vid:pid` + `busid` verification, `usbipd-journal.json` ownership-scoped bind/unbind + crash recovery, and handoff into `UsbDevice::relay()`. |
| `client/protocol/desktop/src/usb_windows/tests.rs` | New | ~260 | `//!` Mock loopback TCP transport tests for `OP_REQ_IMPORT`/`OP_REP_IMPORT`, `vid:pid` mismatch rejection, non-loopback rejection, and crash-safe journal unbinding ONLY Nightjar-bound devices. |
| `client/protocol/desktop/src/usb_arbitration.rs` | New | ~180 | `//!` Shared Desktop registry of currently forwarded USB devices (`(vid, pid, serial/port)`) preventing `gamepad.rs` (and Item `26` HID drivers) from double-opening forwarded controllers, with clean resume on detach. |
| `client/protocol/desktop/src/usb.rs` & `gamepad.rs` | Modify | `<= 1108` / `<= 1375` | Wire `usb_windows.rs` fallback, Track 8B Linux `rusb::Error::Access` -> `usb_linux` fallback (`#[cfg(target_os = "linux")]`), and Track 8C `usb_arbitration.rs` pause/resume hooks (net LOC `<= ceiling`). |
| `client/app/src/main/java/com/limelight/usb/UsbForwarding.java`, `binding/input/driver/UsbDriverService.java`, `UsbArbitrationTest.java` (new) | Modify / New | `<= ceiling` / ~180 | Track 8C Android precedence: USB-forwarding-opted & auto-forwarded devices override `shouldClaimDevice()`, pause/release local driver on attach, reclaim on detach/stream end. |
| `docs/usb-forwarding.md`, `client/docs/sharing.md`, `client/protocol/desktop/src/strings_70/*.rs`, `client/app/src/main/res/values*/strings.xml` | Modify | existing | Updated picker guidance (`usbipd-win` & Linux helper) across all 15 Nightjar locales (`strings_70/`, zero edits to grandfathered `strings/*.rs`) & docs. |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

```text
[User Attaches USB Device on Windows]
  ├── WinUSB eligible & opens cleanly ──▶ Existing usb_winusb -> UsbDevice::relay() (0x553A)
  └── Non-WinUSB / Isochronous / Access refused:
        ├── usbipd-win detected:
        │     1. Query busid state; if NOT already shared by user -> write busid to usbipd-journal.json -> `usbipd bind`
        │     2. Connect strictly to 127.0.0.1:3240 -> OP_REQ_IMPORT (0x8003)
        │     3. Verify OP_REP_IMPORT (0x0003) status==0 & vid:pid == expected & busid == expected
        │     4. Pause local gamepad via usb_arbitration -> hand socket to UsbDevice::relay()
        │     ──(Detach / Stream End / Next-Launch Crash Recovery)──▶
        │        Release usb_arbitration -> unbind ONLY journaled busids -> remove journal entry
        └── usbipd-win absent ──▶ Actionable picker guidance (never cryptic WinUSB error)
```

---

## 5. Settings & 10/10 Craft Plan

- **Zero new user-facing settings** (automatic backend selection + existing USB picker & auto-forward list).
- **Zero `"Experimental"` & zero mod names** across all 15 Nightjar locales (`strings_70/` + `values*/strings.xml`).

---

## 6. Disjoint Implementer Slices (Phase 3 `parallel()`)

| Slice | Stage | Exclusive Owned Files (Zero Overlap) |
| --- | --- | --- |
| **24-A: Windows `usbipd-win` Backend, Journal & Mock Tests (Track 8A)** | **Stage 1** (parallel with 24-B, 24-C) | `client/protocol/desktop/src/usb_windows.rs` (new), `client/protocol/desktop/src/usb_windows/tests.rs` (new) `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |
| **24-B: Desktop Arbitration & Linux `EACCES` Fallback (Tracks 8B & 8C Desktop)** | **Stage 2** (after 24-A) | `client/protocol/desktop/src/usb_arbitration.rs` (new), `client/protocol/desktop/src/usb.rs`, `client/protocol/desktop/src/gamepad.rs`, `client/protocol/desktop/src/strings_70/*.rs`, `client/protocol/desktop/src/strings/tests.rs` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |
| **24-C: Android Controller vs USB-Forwarding Arbitration & Docs (Track 8C Android)** | **Stage 1** (parallel with 24-A) | `client/app/src/main/java/com/limelight/usb/UsbForwarding.java`, `client/app/src/main/java/com/limelight/binding/input/driver/UsbDriverService.java`, `client/app/src/test/java/com/limelight/usb/UsbArbitrationTest.java` (new), `client/app/src/main/res/values*/strings.xml`, `docs/usb-forwarding.md`, `client/docs/sharing.md` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |

---

## 7. Verification & Measurement Plan

1. **Automated Unit & Mock-Transport Tests (No Hardware Required):**
   - `cargo test -p nightjar-desktop --locked usb_windows usb_arbitration usb` — tests mock loopback `127.0.0.1:3240` `OP_REQ_IMPORT`/`OP_REP_IMPORT` handshake, `vid:pid` + `busid` mismatch rejection, non-loopback refusal, crash-safe journal unbinding ONLY Nightjar-bound devices (leaving pre-shared user devices untouched!), Linux `EACCES` -> `usb_linux` fallback decision, and local gamepad pause/resume arbitration.
   - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.usb.UsbArbitrationTest` — verifies Android USB-forwarding precedence over `UsbDriverService.shouldClaimDevice()` and clean reclaim on detach/stream end.
   - `python tools/arch_check.py`, `python tools/code_map.py check`, `node client/scripts/check-doc-links.mjs`, and `cargo fmt -- --check` (without `--all`).
