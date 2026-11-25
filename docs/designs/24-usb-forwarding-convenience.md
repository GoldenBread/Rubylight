# Item 24: Desktop USB Forwarding Convenience — Windows `usbipd-win` Backend, Linux `EACCES` Fallback & Desktop Controller Arbitration (Wave 2 — PR 3: Track 8, Desktop-Only)

Status: engineering design (Phase 1), **rebased 2026-11-25 onto post-refactoring `origin/main` (`955ef39598`, `v6.4.1`)** and [`00-protocol-reservations.md`](00-protocol-reservations.md).

> **Post-6.4.1 Narrowing Note (Do NOT Redo Already-Shipped Work!):**
> - **Already shipped in `6.4.1` (`aa7eea0bd5`, `7633eec58c`, `c358b3fa50`):** Windows inbox `winusb.dll` (`LOAD_LIBRARY_SEARCH_SYSTEM32`) + `libusbK` isochronous USB forwarding (`client/protocol/desktop-media/src/usb/winusb.rs`) and async `libusb` isochronous forwarding (`client/protocol/desktop-media/src/usb/libusb.rs`).
> - **Already shipped in `6.4.1` (`5bab93f402`):** Android Track 8C USB-forwarding vs. USB-controller-driver arbitration (`UsbDriverService.shouldClaimDevice()`, `releaseActiveDevice()`, `awaitDeviceRelease()`, `UsbForwarding.isAutoForwarded()`). **Original Slice `24-C` (Android Java arbitration) is DONE and retired** — keep only a quick verification that reclaim-on-detach works cleanly and synchronize user docs!

## 1. Summary & User Experience

Item 24 delivers the remaining **Desktop** half of **Track 8** — making Nightjar Desktop USB forwarding effortless across **Windows and Linux** with **ZERO host protocol changes** (100% client-side, feeding into the existing `0x553A` TLS USB forwarding tunnel):

1. **Track 8A — Windows `usbipd-win` Backend (`client/protocol/desktop-media/src/usb_windows.rs`):**
   - Sits cleanly alongside existing `usb_winusb.rs` and `usb/winusb.rs` without removing inbox WinUSB / `libusbK` support.
   - Detects an installed `usbipd-win` service (`usbipd.exe` / loopback port `3240`) so Windows users can forward **non-WinUSB devices** (`HidUsb`, `USBSTOR`, Bluetooth dongles, fightsticks, flight sticks, composite devices bound to class drivers) **without running Zadig**!
   - **Loopback-Only Security Invariant:** Connects strictly to **`127.0.0.1:3240`** (never a remote or non-loopback address), executes USB/IP `OP_REQ_IMPORT` (`0x8003`) -> `OP_REP_IMPORT` (`0x0003`), verifies exact **`vid:pid` AND `busid` match** against the user's selected device, and hands the post-import TCP stream straight into Nightjar's existing `UsbDevice::relay()` + `0x553A` TLS tunnel.
   - **Crash-Safe Ownership Journal (`usbipd-journal.json`):** Records **ONLY** devices that Nightjar itself bound via `usbipd bind`. On **Detach**, **stream end**, or **next-launch crash recovery**, Nightjar unbinds ONLY journaled busids to restore their original Windows drivers — **never** unbinding devices the user had already shared via `usbipd-win`!
   - When `usbipd-win` is absent and WinUSB cannot open a device, falls back to `usb_winusb.rs` and shows crisp, actionable guidance in the USB device picker.

2. **Track 8B — Linux `EACCES` Automatic Fallback (`client/protocol/desktop-media/src/usb.rs`):**
   - Today `client/protocol/desktop-media/src/usb.rs:192` still returns `"No permission to open the device. Add a udev rule that gives your user access to it."` on `rusb::Error::Access` (`EACCES`).
   - When `libusb` returns `rusb::Error::Access` (`EACCES`) AND `usb_linux::System::real().missing().is_none()`, Nightjar automatically falls back cleanly to `usb_linux` (`nightjar-usb-helper` + `pkexec` + `usbip-host`) instead of telling the user to hand-author a udev rule!

3. **Track 8C (Desktop) — Desktop Controller-Driver vs. USB-Forwarding Arbitration:**
   - **Desktop (`client/protocol/desktop-media/src/usb_arbitration.rs` + `usb.rs` + `gamepad.rs`, coordinated cleanly with Item `26` Steam Controller 2026):** Prevents local SDL/HID gamepad opening for any currently forwarded USB gamepad `(vid, pid, bus/port)` during attach, and resumes local gamepad control automatically on detach or stream end.
   - Synchronizes user guides in `docs/usb-forwarding.md` and `client/docs/sharing.md`.

---

## 2. Protocol, Native API & Capability Contract

Per [`00-protocol-reservations.md`](00-protocol-reservations.md) §7, **Item 24 is 100% client-side and requires ZERO host wire, control-stream, or Native API changes.**
- Reuses existing `0x553A` (`usb::USB_MESSAGE_TYPE`) TLS USB forwarding tunnel unchanged.
- Strictly gated on existing `HostCapabilities::Feature::UsbForwarding` (`client/protocol/policy/src/http/capabilities.rs`, never `Family::Rubylight`).
- All Linux-specific code uses `#[cfg(target_os = "linux")]` (never bare `cfg(unix)` breaking macOS CI!).
- All unit-test mock TCP listeners bind strictly to loopback `127.0.0.1:0` (`AGENTS.md` Firewall hygiene).

---

## 3. Architecture & <=800-LOC Module Plan (Post-Refactoring Split Crates)

Every new/modified `.rs` file stays strictly `<= 800` LOC with `//!` headers (`tools/arch_check.py`). All new Desktop translations go exclusively in `client/protocol/desktop-config/src/strings_70/<code>.rs` (zero lines added to grandfathered `client/protocol/desktop-config/src/strings/*.rs`!).

| Path | Status | Est. LOC | Responsibility |
| --- | --- | ---: | --- |
| `client/protocol/desktop-media/src/usb_windows.rs` | New | ~460 | `//!` `#[cfg(windows)]` `usbipd-win` detection, loopback-only (`127.0.0.1:3240`) `OP_REQ_IMPORT` (`0x8003`) / `OP_REP_IMPORT` (`0x0003`) with exact `vid:pid` + `busid` verification, `usbipd-journal.json` ownership-scoped bind/unbind + crash recovery, and handoff into `UsbDevice::relay()`. |
| `client/protocol/desktop-media/src/usb_windows/tests.rs` | New | ~260 | `//!` Mock loopback (`127.0.0.1:0`) TCP transport tests for `OP_REQ_IMPORT`/`OP_REP_IMPORT`, `vid:pid` mismatch rejection, non-loopback rejection, and crash-safe journal unbinding ONLY Nightjar-bound devices. |
| `client/protocol/desktop-media/src/usb_arbitration.rs` | New | ~180 | `//!` Shared Desktop registry of currently forwarded USB devices (`(vid, pid, serial/port)`) preventing `gamepad.rs` (and Item `26` HID drivers) from double-opening forwarded controllers, with clean resume on detach. |
| `client/protocol/desktop-media/src/usb.rs` & `gamepad.rs` | Modify | `<= 800` | Wire `usb_windows.rs` fallback, Track 8B Linux `rusb::Error::Access` -> `usb_linux` fallback (`#[cfg(target_os = "linux")]`), and Track 8C `usb_arbitration.rs` pause/resume hooks. |
| `docs/usb-forwarding.md`, `client/docs/sharing.md`, `client/protocol/desktop-config/src/strings_70/*.rs` | Modify | existing | Updated picker guidance (`usbipd-win` & Linux helper) across all 15 Nightjar locales (`desktop-config/src/strings_70/`, via `tools/i18n.py`, zero edits to grandfathered `strings/*.rs`) & docs. |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

```text
[User Attaches USB Device on Windows]
  ├── WinUSB / libusbK eligible & opens cleanly ──▶ Existing usb/winusb.rs / libusb -> UsbDevice::relay() (0x553A)
  └── Non-WinUSB / Access refused:
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
- **Zero `"Experimental"` & zero mod names** across all 15 Nightjar locales (`desktop-config/src/strings_70/`).

---

## 6. Disjoint Implementer Slices (Phase 3 `parallel()`)

| Slice | Stage | Exclusive Owned Files (Zero Overlap) |
| --- | --- | --- |
| **24-A: Windows `usbipd-win` Backend, Journal & Mock Tests (Track 8A)** | **Stage 1** | `client/protocol/desktop-media/src/usb_windows.rs` (new), `client/protocol/desktop-media/src/usb_windows/tests.rs` (new) |
| **24-B: Desktop Arbitration, Linux `EACCES` Fallback & Docs (Tracks 8B & 8C Desktop)** | **Stage 2** (after 24-A) | `client/protocol/desktop-media/src/usb_arbitration.rs` (new), `client/protocol/desktop-media/src/usb.rs`, `client/protocol/desktop-media/src/gamepad.rs`, `client/protocol/desktop-config/src/strings_70/*.rs`, `client/protocol/desktop-config/src/strings/tests/sources.rs`, `docs/usb-forwarding.md`, `client/docs/sharing.md` |

---

## 7. Verification & Measurement Plan

1. **Automated Unit & Mock-Transport Tests (No Hardware Required):**
   - `cargo test --manifest-path client/protocol/Cargo.toml -p nightjar-desktop-media --locked usb_windows usb_arbitration usb` — tests mock loopback `127.0.0.1:0` `OP_REQ_IMPORT`/`OP_REP_IMPORT` handshake, `vid:pid` + `busid` mismatch rejection, non-loopback refusal, crash-safe journal unbinding ONLY Nightjar-bound devices (leaving pre-shared user devices untouched!), Linux `EACCES` -> `usb_linux` fallback decision, and local gamepad pause/resume arbitration.
   - `python tools/i18n.py check`, `python tools/arch_check.py`, `python tools/code_map.py check`, `node client/scripts/check-doc-links.mjs`, and `cargo fmt -- --check` (without `--all`).
