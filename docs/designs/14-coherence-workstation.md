# Coherence Workstation Multi-Window Streaming (7.0.0 — Wave 3)

[Protocol reservations](00-protocol-reservations.md) · [Code map](../code-map.md) · [Nightjar code map](../../client/docs/code-map.md)

Status: engineering design (Phase 1). Rebased 2026-11-25 onto post-refactoring `origin/main` `955ef39598` (`v6.4.1`) and `00-protocol-reservations.md`.

## 1. Summary & User Experience

Coherence Workstation extends Rubylight's existing single-window capture (`rust/core/media-policy/src/app_window.rs`, `rust/windows/graphics/src/capture/app_window.rs`, `rust/host/stream/src/stream/app_window.rs`) into seamless multi-window application streaming for desktop, tablet, and phone productivity plus hybrid gaming. Instead of pinning a remote desktop monitor onto a small laptop or tablet screen — or being locked to one launched game window — a Coherence session lets the user switch cleanly among safe top-level Windows applications on the session's virtual or physical display, streaming only the active window at crisp per-window DPI scaling.

### Core user experience

1. **Instant window focus & streaming:** Starting a session with Coherence active (`rlCoherence=1`) enumerates visible, non-cloaked user windows on the host display and opens Windows Graphics Capture (`CreateForWindow`) directly on the focused window (honoring full frame or client-area crop).
2. **Quick Alt-Tab-style switcher in Nightjar:**
   - **Desktop Nightjar:** Pressing `Ctrl+Alt+Shift+W` (or clicking the minimal top-edge window switcher pill / holding `View+LB` on controller) reveals a blur-free `glass.menu.hud.*` window strip with app titles, active indicators, minimize/maximize/close actions, and instant keyboard/arrow/gamepad selection. The Nightjar window title reflects the active remote app title, and resizing the Nightjar window optionally requests a matching remote window resize + DPI scale.
   - **Android (phone, tablet, handheld, TV):** A dedicated Coherence Switcher bottom sheet (opened from the Stream Menu tile or edge gesture / controller chord) shows running windows with one-tap focus switching, quick resize-to-viewport fit, and a "Show full desktop" toggle.
3. **Per-window DPI scaling via `rlDisplayScale`:** On a virtual display, Coherence coordinates with Rubylight's existing `DisplayScaleVersion` / `rlDisplayScale` pipeline (`rust/host/stream/src/display_session/scale.rs`) to match client density and reports each window's live Win32 `GetDpiForWindow` value so text and UI stay sharp on HiDPI tablets and laptops without ever modifying a physical monitor's Windows scale.
4. **Loud, transparent fallback (zero black-screen freezes):** Whenever WGC window capture cannot capture the active target — e.g., Windows 10 older than 1903 (`os_gate::WGC_WINDOW = 18362`), a DRM/protected window (`WDA_EXCLUDEFROMCAPTURE` / `WDA_MONITOR`), a minimized window with no other open window, an exclusive-fullscreen DirectX surface that refuses window WGC, or a user-pinned whole-display capture backend (`ddx`, `amd`, `idd-push`) — Coherence falls back cleanly to streaming the session's virtual or physical display via Desktop Duplication / WGC monitor and displays a plain-English status banner naming the exact reason.
5. **Safe restore on exit or crash:** Every window move or resize Coherence performs is journaled before the change and restored on normal teardown or after an unexpected host crash — while respecting any window the user manually repositioned afterward.

## 2. Protocol, Native API & Capability Contract

### Compatibility invariants

- **`PROTOCOL_VERSION` stays unchanged.** Stock Moonlight clients and older Nightjar builds never send `rlCoherence=1` or `0x554E` and stream whole displays (or single-app `capture_window`) unchanged.
- **`rubylight_native_api::API_VERSION` stays `1`.** Strictly additive endpoints and response fields (`#[serde(default)]`, `#[serde(deny_unknown_fields)]` on requests, `Unknown` catch-all enum variants).
- **Capability-gated, never brand-gated.** Nightjar gates Coherence strictly on `HostCapabilities::has(Feature::Coherence)` (`HostCapabilities.Feature.COHERENCE` in Java), never on `Family::Rubylight`.

### Capability advertisement & launch opt-in

| Surface | Field / Line | Condition |
| --- | --- | --- |
| `/serverinfo` | `<CoherenceVersion>1</CoherenceVersion>` | `Offered::coherence == true` (host `coherence_workstation` enabled; works on Win10 1903+ with WGC window capture and degrades loudly on older Win10 via display fallback) |
| RTSP `DESCRIBE` | `a=x-rl-control:0x554e` (merged ascending into `a=x-rl-control:...`) | Launch query carried `rlCoherence=1` AND `Offered::coherence == true` |
| Launch query | `rlCoherence=1` (may pair with `rlDisplayScale=<pct>`) | Client opts into Coherence mode for this session |
| Native API `Capabilities::features` | `"session.coherence"` (`feature::COHERENCE`) | Host has Coherence enabled and caller holds `view` or `launch` permission |

`HostCapabilities::Feature::Coherence` (`COHERENCE` in Java) is proven when `CoherenceVersion >= 1` **and** native `"session.coherence"` is present.

### Control message `0x554E` (`coherence::COHERENCE_WINDOW_MESSAGE_TYPE`)

Defined in `client/protocol/core/src/coherence.rs` (`no_std + alloc`), registered in `client/protocol/core/src/ids.rs` within `RUBYLIGHT_7_CONTROL_IDS = 0x5548..=0x554F`. Delivery: **Bidirectional, Reliable** on the encrypted ENet control stream. All multi-byte fields are **little-endian**. Maximum packet payload: **4096 bytes** (window list capped at 32 visible windows per snapshot; titles UTF-8 truncated cleanly on char boundaries to 96 bytes, executable/app names to 48 bytes).

```text
Offset  Len  Field
0       1    version = 1
1       1    opcode (u8):
               0x01 = WindowSnapshot  (Host -> Client: full list + active capture state)
               0x02 = WindowDelta     (Host -> Client: active window rect/DPI/title update)
               0x03 = SelectWindow    (Client -> Host: switch active window or desktop view)
               0x04 = WindowAction    (Client -> Host: focus/minimize/maximize/restore/close/resize)
               0x05 = ActionResult    (Host -> Client: ack or structured refusal for seq)
2       2    flags (u16 LE; bit 0 = client_area_crop, bit 1 = virtual_display_active)
4       4    seq_or_revision (u32 LE; revision for Host->Client, seq for Client->Host)
8       ..   opcode-specific body:
               0x01 WindowSnapshot:
                 capture_mode: u8 (0=WindowWgc, 1=DisplayFallback, 2=DesktopSelected)
                 fallback_reason: u8 (0=None, 1=OsUnsupported, 2=ProtectedContent,
                                      3=WindowMinimized, 4=NoOpenWindows,
                                      5=BackendWholeDisplayOnly, 6=SecureDesktop,
                                      7=WgcCaptureFailed)
                 active_hwnd: u64, active_pid: u32, display_dpi: u16, count: u8 (<=32)
                 repeated `count` entries:
                   hwnd: u64, pid: u32, rect: [i32; 4] (l,t,r,b), dpi: u16,
                   state_flags: u16 (bit0=focused, bit1=minimized, bit2=maximized,
                                     bit3=protected_drm, bit4=elevated_uipi,
                                     bit5=on_session_display),
                   app_len: u8, app_utf8, title_len: u8, title_utf8
               0x02 WindowDelta:
                 hwnd: u64, pid: u32, rect: [i32; 4], dpi: u16, state_flags: u16,
                 title_len: u8, title_utf8
               0x03 SelectWindow:
                 mode: u8 (0=Window, 1=FullDisplay), hwnd: u64, pid: u32,
                 frame: u8 (0=Full, 1=ClientArea)
               0x04 WindowAction:
                 hwnd: u64, pid: u32,
                 action: u8 (1=Focus, 2=Minimize, 3=Maximize, 4=Restore, 5=Close, 6=ResizeFit),
                 req_width: u16, req_height: u16, req_dpi_pct: u16 (0=unchanged)
               0x05 ActionResult:
                 ref_seq: u32, ok: u8,
                 error_code: u8 (0=Ok, 1=WindowGone, 2=PidMismatch, 3=ElevatedUipiBlocked,
                                 4=ProtectedWindow, 5=ResizeRefused, 6=NotSessionOwner,
                                 7=SecureDesktop, 8=DisplayScaleRefused),
                 detail_len: u8 (<=160), detail_utf8
```

### Native API v1 routes (`client/protocol/native-api/src/coherence.rs`)

Served under `/api/native/v1`, authenticated by paired TLS certificate, permissions checked live from `h.paired`:

| Method & Path | Permission | Purpose |
| --- | --- | --- |
| `GET /api/native/v1/session/windows` | `view` | Returns `{ revision, active, capture_mode, fallback_reason, display_dpi, windows: [...] }` for the active Coherence session (or `active: false` when idle/non-Coherence). |
| `POST /api/native/v1/session/windows/select` | `launch` (session owner) | Selects a window `{ "mode": "window" \| "display", "hwnd": "...", "pid": 1234, "frame": "full" \| "client" }`. |
| `POST /api/native/v1/session/windows/action` | `launch` (session owner) | Runs `{ "hwnd": "...", "pid": 1234, "action": "focus" \| "minimize" \| "maximize" \| "restore" \| "close" \| "resize_fit", "width": 1600, "height": 1000 }`. |

Errors use standard `ErrorCode`: `no_session`, `not_session_owner`, `permission_required`, `invalid_value`, `not_found`, `not_writable` (when elevated/UIPI or secure desktop blocks the action).

## 3. Architecture & <=800-LOC Module Plan

All repo paths below carry `docs/code-map.md` for Phase 2 re-verification. Every new module follows `tools/arch_check.py` (`<= 800` LOC, `//!` / `/**` headers, no raw `Config` reads outside `stream/config.rs`, no `Family::Rubylight` gating, annotated `cfg(unix)` vs `target_os = "linux"`).

### Grandfathered files in `tools/size_ratchet.json` (DO NOT GROW)

| File | Ratchet ceiling (LOC) | Strategy to stay at or below ceiling |
| --- | --- | --- |
| `rust/host/stream/src/display_session.rs` | 3158 | Reuse existing `Ready::show_app_window(Option<String>)` and `display_session/scale.rs` unchanged; add zero new display-session logic inside `display_session.rs` itself. |
| `rust/host/stream/src/stream/control.rs` | 2675 | Add only a 3-line match arm for `0x554E` delegating to `super::coherence::handle_message`. |
| `rust/host/stream/src/stream/video/run.rs` | 1956 | Zero direct Coherence logic; capture reopens already trigger automatically via `Ready::show_app_window` + `capture.rs` target change detection. |
| `rust/host/stream/src/stream.rs` | 1324 | Start `CoherenceWorker` in place of `AppWindow` when `rlCoherence=1` (3-line branch). |
| `rust/windows/graphics/src/capture.rs` | 2130 | Keep window capture target opening in existing `open_stream_capture`; delegate Coherence fallback decision to `capture/coherence.rs` (<=4 lines). |
| `rust/windows/display/src/display_recovery.rs` | 913 | **Do not bloat `display_recovery.rs`.** Call `crate::coherence_recovery::recover_if_pending()` in a 2-line hook at startup recovery; all placement journal code lives in `coherence_recovery.rs`. |
| `rust/windows/io/src/input.rs` | 1342 | Zero growth — absolute mouse, touch, and pen already map onto `capture::target_rect` (`rust/windows/graphics/src/capture/app_window.rs:124`)! |
| `rust/host/web/src/native_api.rs` | 853 | Add `mod coherence;` + 3 route registrations + 1 capability string (keep net LOC `<= 853`). |
| `client/protocol/desktop-media/src/stream/mod.rs` | 2914 | Delegate `0x554E` events, title updates, and switcher rendering to `stream/coherence_switcher.rs`. |
| `client/protocol/desktop-config/src/strings/*.rs` | 1750 | **Zero growth!** Route all new 7.0.0 Coherence Desktop translations to `client/protocol/desktop-config/src/strings_70/<code>.rs` (14 non-English locale tables; English literals live at call sites). |
| `client/app/src/main/java/com/limelight/Game.java` | 1883 | Wire `CoherenceSwitcherSheet` via 3-line hook in `GameOverlays.java` / `CoherenceCoordinator.java`. |
| `client/app/src/main/java/com/limelight/ui/StreamMenu.java` | 1321 | Add 1 conditional tile (*Switch window*, gated by `HostCapabilities.Feature.COHERENCE` and active session flag). |

### Existing non-ratcheted files reused cleanly

- `rust/core/media-policy/src/app_window.rs` (295 LOC), `rust/windows/graphics/src/capture/app_window.rs` (256 LOC), `rust/windows/graphics/src/capture/window.rs` (259 LOC), and `rust/host/stream/src/stream/app_window.rs` (342 LOC) stay well below 800 LOC and keep single-app `capture_window = window` untouched; Coherence shares their `WindowTarget`, `bounds()`, `target_rect()`, and `os_gate::Feature::WgcWindow` helpers via `pub(crate)` visibility.

### Planned new <=800-LOC modules

| New module path | Crate / App | Target LOC | Responsibility |
| --- | --- | --- | --- |
| `client/protocol/core/src/coherence.rs` | `rubylight-protocol` | ~380 | `no_std` codecs for `0x554E` (`WindowSnapshot`, `WindowDelta`, `SelectWindow`, `WindowAction`, `ActionResult`), UTF-8 boundary truncation, fuzz-safe bounds checks. |
| `client/protocol/native-api/src/coherence.rs` | `rubylight-native-api` | ~220 | Additive v1 serde structs for `/session/windows*` + fixture tests. |
| `rust/core/session/src/coherence.rs` | `butterpollo-core` | ~420 | Pure window eligibility filter (excludes shell/cloaked/tool/<64px), safe fallback resolver (`FallbackReason`), MRU switcher ordering, aspect-preserving resize clamping, `put_back` window-placement comparison policy, and `CoherenceHub` snapshot/command bridge. |
| `rust/windows/graphics/src/capture/coherence.rs` | `butterpollo-windows` | ~450 | Win32 enumeration extending `app_windows()` with `GetWindowDisplayAffinity` (DRM check), `GetDpiForWindow`, UIPI/elevation check, `MonitorFromWindow` session-display tagging, `SetForegroundWindow`/`ShowWindow`/`SetWindowPlacement`/`WM_CLOSE` safe actions. |
| `rust/windows/display/src/coherence_recovery.rs` | `butterpollo-windows` | ~340 | Atomic `coherence-journal.json` recording `(hwnd, pid, process_create_time, WINDOWPLACEMENT, applied_rect)` before any Coherence move/resize and restoring safely on teardown or post-crash boot. |
| `rust/host/stream/src/stream/coherence.rs` | `butterpollo` | ~460 | Dedicated `"coherence"` worker thread (250 ms poll + immediate wake on `0x554E` command), drives `Ready::show_app_window`, switches transparently to display fallback with loud warning, pushes `WindowSnapshot` / `WindowDelta` over `0x554E`, binds to `CoherenceHub`. |
| `rust/host/web/src/native_api/coherence.rs` | `butterpollo` | ~220 | Handlers for `GET /session/windows`, `POST /session/windows/select`, `POST /session/windows/action`. |
| `client/protocol/policy/src/coherence.rs` | `nightjar-policy` | ~260 | Shared MRU list state, Alt-Tab cycle index, fallback banner mapping, and resize-fit request debouncer (`300 ms` after window drag ends). |
| `client/protocol/client/src/control/coherence.rs` | `nightjar-core` | ~180 | Control-stream send/receive helpers for `0x554E`. |
| `client/protocol/desktop-media/src/stream/coherence_switcher.rs` | `nightjar-desktop` | ~420 | Desktop switcher HUD (`glass.menu.hud.*`), optional top tab bar, SDL window title sync, viewport resize-to-remote-window debounce, keyboard/gamepad shortcuts. |
| `client/protocol/desktop-config/src/strings_70/<code>.rs` (14 locale files) | `nightjar-desktop` | ~40/locale | 7.0.0 Desktop translations for Coherence switcher & fallback banners across all 14 non-English locales (`de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`), keeping grandfathered `strings/*.rs` <= 1,750 LOC. |
| `client/app/src/main/java/com/limelight/ui/CoherenceSwitcherSheet.java` | Android `app` | ~390 | Android modal switcher sheet (`glass.menu.window.*` / `glass.menu.baked.*`) + fallback status pill + quick window actions. |
| `client/app/src/main/java/com/limelight/CoherenceCoordinator.java` | Android `app` | ~240 | Coordinates JNI `0x554E` callbacks, `StreamMenu` tile visibility, and aspect/DPI fit requests without growing `Game.java`. |

### Encoder, capture & OS compatibility

- **WGC + Desktop Duplication + D3D11 + Win10:** Coherence captures individual windows through existing WGC (`CreateForWindow`, `os_gate::WGC_WINDOW = 18362`, Windows 10 1903+) and reuses the exact same `compute.rs` (D3D12) and `gpu_color.rs` (D3D11 fallback) letterboxing pipeline (`display_policy::letterbox`). When WGC window capture is unavailable (Win10 < 1903, protected window, or whole-display backend forced), Coherence sets `Ready::show_app_window(None)` so `open_stream_capture` cleanly captures the virtual or physical display via Desktop Duplication / WGC monitor and emits `FallbackReason`.
- **AMD AMF & NVIDIA NVENC:** Both encoders consume the same converted NV12/P010 frame from `compute.rs` / `gpu_color.rs` regardless of whether capture opened on a `WindowTarget` or display. No encoder-specific changes are required; NVIDIA NVENC compatibility is verified against existing `rust/windows/graphics/src/nvenc.rs` frame submission contracts and vendor headers/samples (never claimed as run on hardware).

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### Host Coherence state machine (`rust/core/session/src/coherence.rs`)

```text
Off ──(launch rlCoherence=1)──▶ Enumerating
Enumerating
  ├──(safe window found & WGC window supported)──▶ WindowActive { hwnd, pid, display, dpi, frame }
  └──(no windows | WGC unsupported | backend=ddx/amd/idd-push)──▶ DisplayFallback { reason, display }

WindowActive
  ├──(SelectWindow(hwnd2) | OS foreground change if follow_focus)──▶ Switching { from, to }
  │      ├──(WGC target valid & !protected & !minimized)──▶ WindowActive { to }
  │      └──(protected | minimized | WGC error)──▶ DisplayFallback { reason, display }
  ├──(SelectWindow(FullDisplay))──▶ DesktopSelected { display }
  ├──(active window closed)──▶ pick next MRU safe window on display, else DisplayFallback(NoOpenWindows)
  ├──(secure desktop shown)──▶ DisplayFallback(SecureDesktop)
  └──(session_teardown)──▶ Restoring ──▶ Off

DesktopSelected ──(SelectWindow(hwnd))──▶ Switching
DisplayFallback ──(condition clears & user selects window or window restores)──▶ WindowActive
```

### Safe window filtering & security boundaries

1. **Enumerated safe windows only:** Excludes `DWMWA_CLOAKED != 0` (Windows Store suspended / other virtual desktops), `WS_EX_TOOLWINDOW`, zero-area or `< 64×64` px windows (`app_window::MIN_SIZE`), desktop shell hosts (`Progman`, `WorkerW`, `Shell_TrayWnd`, `Shell_SecondaryTrayWnd`), and Rubylight's own hidden helper windows.
2. **DRM / protected content detection:** Calls `GetWindowDisplayAffinity` during enumeration and before WGC attach. Any window with `WDA_MONITOR` or `WDA_EXCLUDEFROMCAPTURE` is tagged `state_flags::PROTECTED_DRM`; selecting it refuses WGC black-frame capture and transitions loudly to `DisplayFallback(ProtectedContent)` (or returns `ActionResult::ProtectedWindow` if the user explicitly asked for window-only).
3. **UIPI / elevation honesty:** If the host runs non-elevated (portable mode) and a selected window belongs to an elevated process, `state_flags::ELEVATED_UIPI` is flagged; `WindowAction` (`Minimize`, `Close`, `ResizeFit`) returns `ActionResult::ElevatedUipiBlocked` with a plain-English explanation instead of silently failing.
4. **HWND reuse protection:** Every command carries `(hwnd, pid)` and verifies `window::owned_by(hwnd, owner_of(hwnd), pid)` before applying any Win32 action (`rust/windows/graphics/src/capture/window.rs:213`), eliminating stale-HWND race conditions.

### Reconnect & multi-client matrix

| Scenario | Expected behavior |
| --- | --- |
| **Same-device reconnect / roam** | Coherence worker survives brief ENet drop if session stays live; upon reconnect, host immediately pushes a fresh `WindowSnapshot` (`revision + 1`) so client switcher & window title resync. |
| **Continue Here (e.g., Desktop → Tablet)** | New client takes session ownership, receives `WindowSnapshot`, and may send `ResizeFit` + `rlDisplayScale` for its form factor; the placement journal keeps the *original* pre-session `WINDOWPLACEMENT` (never overwrites original with Desktop's intermediate resize!). |
| **Dual-screen (`rlSecondScreen=1`)** | Coherence is refused on a second-screen launch (`rlCoherence=1&rlSecondScreen=1` returns clear launch error / falls back to normal second-display stream with warning) so two concurrent streams from one device never fight over foreground focus. |
| **Background / co-op instance stream** | Coherence is mutually exclusive with single-window background isolation; background streams keep their dedicated single `WindowTarget`. |
| **Physical display vs. Virtual display** | Virtual display supports both per-window resize and `rlDisplayScale` DPI changes (via `display_session/scale.rs`). Physical display allows Coherence window capture & switching, but **never changes physical monitor Windows DPI** (`scale.rs:5-10` preserved) and surfaces a clear info note if `req_dpi_pct` is ignored. |

### Window-placement & DPI crash-restore journal (`rust/windows/display/src/coherence_recovery.rs`)

Before Coherence executes its first `ResizeFit` or monitor move on any window `(hwnd, pid)`:
1. Read `GetWindowPlacement(hwnd)` (original normal rect, showCmd, min/max pos) and process creation timestamp `GetProcessTimes(pid)` (`process_create_time_100ns` to prevent PID reuse collision after reboot).
2. Write entry atomically (`temp` → `sync_all` → `rename`) to `config_dir/coherence-journal.json` keyed by `(hwnd, pid, process_create_time)` **before** calling `SetWindowPlacement` / `MoveWindow`, recording both `original_placement` and `applied_rect`.
3. Any virtual-display DPI change made for `rlDisplayScale` is already journaled by `rust/windows/display/src/display_recovery.rs` (`Entry::scale: Option<(u32, u32)>`) and restored on crash/exit.
4. On normal `session_teardown` OR on host service startup (`display_recovery::recover` hook after a crash):
   - For each entry in `coherence-journal.json`, verify `IsWindow(hwnd)`, `owner_of(hwnd) == pid`, and matching `process_create_time`.
   - Apply the exact `put_back` invariant (`rust/windows/display/src/display_recovery.rs:99`): restore `original_placement` **only if** the window's current `bounds(hwnd)` still matches `applied_rect` (within ±4 px for DWM frame rounding). If the user manually moved or resized the window locally, leave their placement untouched!
   - Remove restored or exited-process entries and delete `coherence-journal.json` when empty.

## 5. Settings & 10/10 Craft Plan

### Fewer-settings compliance

- **Default-off gate until measured:** Host key `coherence_workstation` (bool, default `false`, added to `SETTINGS_30` + `LIVE` + `Config::coherence_workstation()` in `rust/core/config/src/config/settings30.rs`, folded under *Capture & display → Window streaming* in Console `rust/web/src/lib/schema/extensions.ts` and `rust/host/web/src/console/settings.rs`). Updating `rust/core/config/src/config/settings30.rs` explicitly updates its hardcoded 12-entry test assertions (`there_are_12_unique_settings`, `answers()`, `the_defaults_are_the_documented_values`, and `the_console_schema_lists_exactly_these_settings_with_these_defaults`, which compiles `rust/web/src/lib/schema/extensions.ts` via `include_str!`). Two-sentence help: *"Allows Nightjar devices to stream and switch between individual app windows instead of a full desktop. Uses Windows Graphics Capture per window and falls back to the full display for protected or fullscreen apps."* Once Wave 3 HW measurements confirm zero default-path cost and clean switch stability, default-on can be evaluated separately.
- **Zero new Nightjar Settings rows:** Nightjar exposes Coherence purely as an opt-in session mode on Desktop/app launch (*Open in Coherence* action on the Desktop tile / session menu, gated by `HostCapabilities::has(Feature::Coherence)`) and reuses existing *PC display scaling* (`rlDisplayScale`) and *Show window frame* controls — keeping Nightjar's 4 top-level Settings sections untouched.

### Visual craft, dynamic glyphs & plain-English errors

- **Zero stream-time blur:** Desktop's quick Alt-Tab switcher strip renders on `glass.menu.hud.*` (`blur: 0`, `shadowBlur: 0`, `fill: #0E1017C7`). Android's modal Coherence Switcher sheet uses `glass.menu.window.*` (API 31+ with `isCrossWindowBlurEnabled`) or `glass.menu.baked.*` ONLY while open, removing its window completely on close so post-close composition cost equals pre-open cost.
- **Dynamic input glyphs:** Switcher hints reflect the last input device (`Ctrl+Alt+Shift+W` / `Arrows + Enter` on keyboard, `LB/RB + A` on gamepad, `D-pad + OK` on TV remote, tap/swipe on touch).
- **Plain-English fallback & error states:**
  - `ProtectedContent`: *"This app blocks window capture to protect its content, so the stream is showing the full display instead."*
  - `OsUnsupported`: *"Single-window streaming needs Windows 10 version 1903 or newer. Showing the full display instead."*
  - `BackendWholeDisplayOnly`: *"Capture method is set to a whole-display backend. Switch Capture method to Automatic or WGC in Rubylight settings to stream individual windows."*
  - `ElevatedUipiBlocked`: *"This window runs as administrator, so Nightjar cannot move or close it while Rubylight runs without the Windows service."*
  - Zero "Experimental" anywhere in UI. Zero mod names (never mention Nucleus Co-op or ANR in user-facing Console or Nightjar text, even though internal `capture/window.rs` comments reference co-op window capture).
- **15 locales:** All strings localized across all 15 languages (`en`, `de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`). Desktop routes all 14 non-English translation tables to `client/protocol/desktop-config/src/strings_70/<code>.rs` (English literals live at call sites via `tr`/`trf`/`en`; never growing grandfathered `strings/*.rs` past 1,750 LOC), alongside `client/app/src/main/res/values*/strings.xml` and `rust/assets/locale/*.json`. Window titles in the switcher use 1-line end-ellipsis with a fixed app-name badge and DPI pill (`150%`, tabular numbers `type.numbers.feature = "tnum"`) so long German/CJK window titles never push controls off screen.

## 6. Disjoint Implementer Slices (Phase 3 `parallel()`)

Strictly non-overlapping file ownership per branch: **Stage 1 (`Slice 14A`)** lands first with ALL shared registration files and schema lockstep edits, unlocking **Stage 2 parallel slices (`14B`, `14D`, `14E`)** and **Stage 3 (`14C`, after `14A + 14B`)** with zero overlapping lines.

### Stage 1 — Slice 14A: Protocol Wire, Core Logic, Shared Registrations & Native API Surface *(lands first before Stage 2 parallel slices)*
- `client/protocol/core/src/ids.rs` *(registers `0x554E` in `RUBYLIGHT_7_CONTROL_IDS`)*
- `client/protocol/core/src/lib.rs`
- `client/protocol/core/src/coherence.rs` *(new)*
- `client/protocol/core/src/fuzz.rs`
- `client/protocol/native-api/src/lib.rs` *(exports `pub mod coherence;` and `feature::COHERENCE`)*
- `client/protocol/native-api/src/coherence.rs` *(new)*
- `client/protocol/native-api/tests/fixtures.rs`
- `client/protocol/native-api/tests/fixtures/coherence.json` *(new)*
- `rust/core/src/lib.rs`
- `rust/core/session/src/coherence.rs` *(new — pure state machine + `CoherenceHub` snapshot/command bridge)*
- `rust/core/media-policy/src/extensions.rs` *(registers `Offered::coherence` / `CoherenceVersion` / `a=x-rl-control:0x554e`)*
- `rust/core/config/src/config/settings30.rs` *(adds `coherence_workstation` to `SETTINGS_30`, `LIVE`, `Config::coherence_workstation()`, and updates its hardcoded 12-entry test assertions: `there_are_12_unique_settings`, `answers()`, `the_defaults_are_the_documented_values`, and `the_console_schema_lists_exactly_these_settings_with_these_defaults`)*
- `rust/web/src/lib/schema/extensions.ts` *(adds `coherence_workstation` in lockstep with `settings30.rs`'s `include_str!` schema assertion)*
- `rust/host/stream/src/extensions.rs`
- `rust/host/stream/src/state.rs` *(adds 1 field `pub coherence: Arc<butterpollo_core::coherence::CoherenceHub>`)*
- `rust/host/web/src/native_api.rs` *(registers `mod coherence;`, `/session/windows`, `/session/windows/select`, `/session/windows/action`, and `feature::COHERENCE`)*
- `rust/host/web/src/native_api/coherence.rs` *(new — Axum handlers backed by `h.coherence`)*
- `client/protocol/policy/src/lib.rs`
- `client/protocol/policy/src/coherence.rs` *(new)*
- `client/protocol/policy/src/http/capabilities.rs` *(adds `Feature::Coherence`)*
- `client/protocol/client/tests/capabilities_contract.rs`
- `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java` *(adds `HostCapabilities.Feature.COHERENCE`)*
- `client/app/src/test/resources/protocol/capabilities/` *(capability contract fixtures)*

### Stage 2 — Slice 14B: Windows Window Capture Extension & Placement Crash-Restore Journal *(depends on Stage 1 Slice 14A; parallel with 14D/14E)*
- `rust/windows/src/lib.rs`
- `rust/windows/graphics/src/capture/coherence.rs` *(new)*
- `rust/windows/graphics/src/capture/window.rs` *(visibility `pub(crate)` helpers only)*
- `rust/windows/graphics/src/capture.rs` *(<=4-line fallback hook)*
- `rust/windows/display/src/coherence_recovery.rs` *(new)*
- `rust/windows/display/src/display_recovery.rs` *(2-line startup recovery hook)*

### Stage 2 — Slice 14D: Nightjar Engine & Desktop Switcher *(depends on Stage 1 Slice 14A; parallel with 14B/14C/14E)*
- `client/protocol/client/src/control/coherence.rs` *(new)*
- `client/protocol/client/src/control/driver.rs`
- `client/protocol/client/src/control/messages.rs`
- `client/protocol/client/src/control/events.rs`
- `client/protocol/client/src/http/native.rs`
- `client/protocol/client/tests/control_messages_contract.rs`
- `client/protocol/desktop-media/src/stream/coherence_switcher.rs` *(new)*
- `client/protocol/desktop-media/src/stream/mod.rs`
- `client/protocol/desktop-config/src/strings_70/<code>.rs` *(all 14 non-English 7.0.0 Desktop locale tables; never growing grandfathered `strings/*.rs` past 1,750 LOC)*

### Stage 2 — Slice 14E: Nightjar Android Switcher, Host Locales & Docs *(depends on Stage 1 Slice 14A; parallel with 14B/14C/14D)*
- `client/protocol/android/src/lib.rs`
- `client/protocol/android/src/coherence.rs` *(new)*
- `client/app/src/main/java/com/limelight/CoherenceCoordinator.java` *(new)*
- `client/app/src/main/java/com/limelight/ui/CoherenceSwitcherSheet.java` *(new)*
- `client/app/src/main/java/com/limelight/ui/StreamMenu.java`
- `client/app/src/main/res/values*/strings.xml`
- `client/app/src/test/java/com/limelight/CoherenceCoordinatorTest.java` *(new)*
- `rust/assets/locale/` *(host Coherence strings)*
- `docs/control-messages.md`, `docs/native-api.md`, `docs/configuration.md`
- `rust/release/notes.d/7-0-coherence-workstation.md` *(new)*

### Stage 3 — Slice 14C: Host Coherence Worker & Integration Tests *(depends on 14A + 14B; parallel with 14D/14E)*
- `rust/host/stream/src/stream/coherence.rs` *(new — binds `h.coherence` to Windows capture & `0x554E`)*
- `rust/host/stream/src/stream.rs` *(3-line branch)*
- `rust/host/stream/src/stream/control.rs` *(3-line `0x554E` dispatch)*
- `rust/host/web/src/console/settings.rs`
- `tests/host/tests/reconnect_tests/coherence_tests.rs` *(new)*
- `tests/host/tests/nvhttp_parity/native_tests/coherence.rs` *(new)*
- `tests/host/tests/nvhttp_parity/native_tests.rs` *(1-line `mod coherence;`)*

## 7. Verification & Measurement Plan

### Narrowest unit, property, and interop tests

1. **Wire & fuzz:** `cargo test -p rubylight-protocol ids coherence fuzz` — validates `0x554E` encode/decode, UTF-8 truncation safety, 32-window cap, and fuzz immunity.
2. **Core & journal policy:** `cargo test -p butterpollo-core coherence settings30` and `cargo test -p butterpollo-windows coherence_recovery` (non-GPU temp-dir journal tests) — validates safe-window filtering, `FallbackReason` mapping, `put_back` placement restoration (restores untouched window, skips user-moved window, skips recycled PID), and the updated `settings30` entry count + `rust/web/src/lib/schema/extensions.ts` lockstep check.
3. **Native API & capabilities:** `cargo test -p rubylight-native-api`, `cargo test -p nightjar-core --test capabilities_contract --test control_messages_contract`, `cargo test -p butterpollo nvhttp::native_tests::coherence`.
4. **Loopback & interop:** `cargo test -p butterpollo stream::reconnect_tests::coherence_tests` and `interop_tests` — proves:
   - Stock client without `rlCoherence=1` sees zero wire changes.
   - Protected window triggers `DisplayFallback(ProtectedContent)` + session warning + `WindowSnapshot` update.
   - Reconnect and Continue Here preserve original placement journal entry and resync window list.
5. **Android & architecture:** `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.CoherenceCoordinatorTest`, `python tools/arch_check.py` (including grandfathered `client/protocol/desktop-config/src/strings/*.rs <= 1,750` LOC), `python tools/code_map.py check`.

### Visual screenshot checks

- **Desktop (`shot.rs`):** Capture Coherence quick switcher (`glass.menu.hud.*`, 0 blur) with 6 mixed windows including a protected badge, elevated badge, long German title, and CJK title (Night + High-Contrast).
- **Android (`emulator-smoke.py`):** Capture Coherence Switcher bottom sheet at `393dp-font-1`, `393dp-font-2`, and `393dp-font-2-landscape`, plus fallback banner over demo stream.
- **Console (`screenshots.cjs`):** Capture Settings → Display/Capture showing the folded `coherence_workstation` row from `rust/web/src/lib/schema/extensions.ts` in Light & Dark, Desktop & Phone.

### Serialized idle-gated hardware criteria (RX 7900 XT + Radeon 780M)

- **Idle gate:** Verify `http://127.0.0.1:47989/serverinfo` reports `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0`. Use isolated test host: `butterpollo.exe --config-dir C:\tests\coherence --port 48125 --bind 127.0.0.1`.
- **Exit code 3 abort rule:** Exit code `3` from `rust/release/e2e.py` or `check.ps1` signals an AMD encoder hang — **abort immediately and investigate; never retry**.
- **Native Windows hardware tests (`#[ignore]`):**
  - Open two real test windows + one `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` test window; verify WGC switches cleanly between the two safe windows, crops client area accurately, maps absolute input to `target_rect`, transitions loudly to `DisplayFallback(ProtectedContent)` on the protected window, and restores exact `WINDOWPLACEMENT` on teardown and simulated crash restart.
- **Performance & stability acceptance gate (RX 7900 XT, confirmed on Radeon 780M):**
  - Standard display stream (Coherence off): `present_to_sent p99` delta `<= +0.05 ms` (0 overhead).
  - 50 consecutive window switches at 1080p60 HEVC: median switch-to-first-frame `<= 120 ms`, p95 `<= 180 ms`, zero D3D11/D3D12 handle or VRAM growth, zero encoder hangs, zero stray `coherence-journal.json` entries after teardown.
- **NVIDIA note:** NVIDIA NVENC path is checked against existing `rust/windows/graphics/src/nvenc.rs` frame contracts and vendor SDK headers/samples only; never claimed as hardware-tested.

---

## 8. Post-Discovery & 6.3.0 Rebase Addendum — Desktop IME, macOS Layout, Touchpad/Stylus & Sticky DDX

- **Desktop CJK / Dead-Key IME Composition & Live Keyboard Layouts (Slice 14D, zero wire changes):**
  1. Extend `client/protocol/desktop-media/src/stream/text_field.rs` + `input.rs` with a Coherence-aware desktop IME mode that calls `SDL_StartTextInputWithProperties` and handles `SDL_EVENT_TEXT_EDITING` pre-edit composition (porting Android `HostTextField.java`'s composing-diff algorithm over existing UTF-8 `send_text`, leaving raw scancodes untouched during gameplay).
  2. Implement macOS `TISCopyCurrentKeyboardInputSource` and live Linux Wayland/X11 XKB layout reporting in `text_field.rs::current_layout()`.
  3. Add Desktop relative trackpad touch mode in `stream/touch.rs` for Windows/Linux touchscreens (Surface / handhelds in Coherence) and wire Android `MotionEvent.AXIS_ORIENTATION` pen barrel roll (`0x5500000B`, Slice 14E) when advertised by `HostCapabilities`.
- **6.3.0 (`7f0e6e287d`) Sticky DDX Clearance:** Explicitly clear `7f0e6e287d`'s mid-session sticky DDX fallback latch whenever Coherence initiates window-scoped WGC capture so switching to single-window streaming never stays trapped on Desktop Duplication.
