# Item 17: UX Craft, Cold-Start, Installer, Errors & Crash-Restore Soak (#129)

Status: design, not implemented (Wave 4). Rebased 2026-11-25 onto post-refactoring `origin/main` `955ef39598` (`v6.4.1`) and `docs/designs/00-protocol-reservations.md`.

Item 17 brings Rubylight 7.0.0 and Nightjar 7.0.0 from feature-complete to craft-complete across six engineering pillars aligned 100% with Wave 0's ground-up **7.0 Console-Quality Complete Redesign** across every user-facing surface: (1) cohesive console-grade tactile UI soundscape & spring-synchronized micro-haptics, (2) sub-250 ms Nightjar cold-start and parallel fast-launch pipeline, (3) redesigned 7.0 Installer + **Instant QR Pairing Finish Screen** (`rust/setup/`, consuming canonical `ui_theme.rs` from `01`) & System Tray (`rust/windows/io/src/tray.rs`, extending canonical `tray/status.rs` from `01`) polish with live `rubylight-vgamepad` VHF driver health and one-click repair, (4) plain-English error rewrites across `log_analysis/`, host warnings, Native API `ErrorBody { error: ApiError { ... } }`, and Nightjar error sheets, (5) pre-mutation DPI, display & Quick Suspend crash-restore hardening (issue #129) on the single canonical `RecoveryJournalV2` (`rust/core/session/src/recovery_journal.rs`, `recovery-journal.json` + `--recover-session` service hook from `08`) verified by a 10-cycle forced-kill soak test, and (6) a complete multi-locale (all **22** host locales in `rust/assets/locale/*.json` and all **15** Nightjar locales via `strings_70/<code>.rs`), TalkBack/AccessKit/keyboard accessibility, and dynamic input glyph sweep across all 5 Rubylight surfaces and all Nightjar form factors (auditing/extending canonical `client/protocol/policy/src/input_glyphs.rs` from `02`, preserving Moonlight V+ port credits).

---

## 1. Summary & User Experience

### 1.1 The Six Pillars

1. **Tactile UI Soundscape & Spring-Synchronized Micro-Haptics**
   - Elevates Wave 0's redesigned 7.0 Console Home, physics-sheets, First-Run / Pairing Cinema, Visual Preview-Card Settings, Virtual Controller Studio (`#151`), and Console Control Center floating dock with cohesive, quiet, console-grade tactile feedback:
     - **Cohesive console audio cues:** procedurally synthesized, low-pass filtered acoustic micro-sounds normalized to `-18 LUFS`:
       - **Focus tick** (`<= 24 ms`): crisp, soft tactile tick moving across Console Home hero/shelf cards, Preview-Card Settings, and Control Center visual cards.
       - **Confirm** (`<= 42 ms`): warm dual-harmonic tap on Play, Apply, or primary selection.
       - **Back** (`<= 38 ms`): gentle descending release cue on Back, Cancel, or dismiss.
       - **Control Center open / close** (`<= 65 ms`): subtle rising glass-swell cue when the floating dock opens and a quiet damped settle when it closes.
       - **DVR clip chime** (`<= 140 ms`): clean two-note crystalline confirmation chime when an instant-replay clip saves.
       - **Pairing chime** (`<= 180 ms`): signature three-note resolved chime when First-Run / Pairing Cinema or the Installer QR Finish Screen pairs a host cleanly.
       - Plus **Launch Curtain lift**, **Quick Suspend freeze/thaw**, and **Reject** (muted low thud on disabled controls).
     - **Spring-synchronized Android & gamepad micro-haptics:**
       - Extends `Haptics.java` with semantic kinds (`FOCUS_TICK`, `CONFIRM`, `BACK`, `CONTROL_CENTER_OPEN`, `CONTROL_CENTER_CLOSE`, `DVR_CLIP_CHIME`, `PAIRING_SUCCESS`, `CURTAIN_LIFT`, `SUSPEND_TOGGLE`, `REJECT`, `SHEET_DETENT`, `PAD_SNAP`) synchronized directly to **Wave 0's Console Home, physics-sheet detent, Control Center floating dock, and Virtual Controller Studio (`#151`) snap springs** (`Springs.java` / Desktop `ui/spring.rs`), mapped to Android 11–15+ `HapticFeedbackConstants` with graceful fallback down to Android 5.0 (API 21).
       - Gamepad menu navigation on Android and Desktop triggers brief, refined low-amplitude local rumble pulses (`14–28 ms`, `10–22%` motor amplitude) synchronized with Console Home & Control Center spring arrivals strictly outside an active stream.
     - **Mute, Reduced-Motion & Zero Stream-Audio Latency Invariants:**
       - Respects Android ringer/silent mode and `SOUND_EFFECTS_ENABLED`, Windows/Linux/macOS mute & Do Not Disturb, `ui_sounds` opt-in setting (default `false`), and `Motion.reduced()` / `prefers-reduced-motion` (suppresses rapid repeat focus ticks and replaces multi-beat spring cues with a single quiet settle tick).
       - **Zero stream audio latency impact**: the moment a stream's audio renderer starts, the UI soundscape engine unbinds/closes its audio output handle (`SoundPool` / SDL3 audio stream) and gamepad UI rumble yields 100% to host stream haptics. Neither allocates, locks, or mixes on the stream audio thread.

2. **<250 ms Nightjar Cold-Start & Parallel Fast Stream Launch**
   - Opening Nightjar from cold process start renders the cached 7.0 Console Home library and accepts input in **<250 ms (p95)** on Android (mid-range ARM64 phone & Android TV) and Desktop (Windows/Linux/macOS).
   - Heavy work that currently runs synchronously or stalls early frames (`MediaCodecList` enumeration, mDNS discovery sockets, TLS truststore & client certificate parsing, USB device enumeration, cover-art disk scanning, GPU driver probing) moves behind a deterministic `StartupScheduler` with prioritized background lanes.
   - A persisted **warm decoder & capability cache** stores hardware codec/HDR/4:4:4 probe results keyed by OS build fingerprint + MediaCodec HAL version (Android) or GPU LUID/PCI ID + driver version (Desktop), eliminating 80–220 ms of repeated startup and pre-launch probing.
   - When the player taps **Play**, a **parallel pre-launch pipeline** overlaps network readiness (WoL / Away Wake Relay check if asleep, `/serverinfo` freshness check), local decoder & surface warmup, audio track pre-open, and Launch Curtain (`0x5548`) presentation so the transition from tap to first decoded frame drops idle dead time without adding a single frame of streaming latency.

3. **Redesigned 7.0 Installer, Instant QR Finish Screen & System Tray Polish**
   - **Installer (`rust/setup/`, built via `-p butterpollo-setup`)**: builds directly on Wave 0's single canonical `rust/setup/src/ui_theme.rs` (created in `01`, consumed/extended here in `17`) to deliver Per-Monitor-V2 DPI-aware dark/light native Win32 chrome (zero WebView2 bloat).
     - Integrates Wave 0's `rubylight-vgamepad` VHF virtual gamepad driver (zero unsigned KMDF; uses Windows Virtual HID Framework) alongside `rubylight-vdisplay`, virtual audio, Windows Firewall, and service registration, displaying live per-component health (Display driver, Virtual gamepad VHF driver, Audio driver, Firewall, Service).
     - **Instant QR Pairing Finish Screen (`rust/setup/src/finish_qr.rs`)**: when installation or repair completes and the local host starts, displays a high-contrast 7.0 pairing QR code + local PIN right inside the installer finish view so the user can aim Nightjar 7.0's **First-Run / Pairing Cinema** straight at the screen and start streaming in seconds — with direct fallback buttons **Open Web Console** and **Finish** (and graceful non-blocking fallback if service start is deferred).
     - **`--repair` one-click repair mode (`rust/setup/src/repair.rs`)**: validates driver-store signatures, restarts stuck services, repairs firewall/Start Menu shortcuts, and verifies live `rubylight-vgamepad` VHF + `rubylight-vdisplay` IDD device enumeration without wiping user config or paired devices.
   - **System Tray & Notifications (`rust/windows/io/src/tray.rs`)**: extends Wave 0's canonical `rust/windows/io/src/tray/status.rs` (created in `01`, extended in `17`) with 7.0 Console-Quality semantic badge scales (`16/24/32/48 px` crisp contrast-ringed emerald = streaming, amber = paused / Quick Suspended, ruby = attention / driver repair needed) for light and dark Windows 10/11 taskbars.
     - Tooltip, Windows notifications, and tray menu (`rust/windows/io/src/tray/menu.rs`) display live `rubylight-vgamepad` VHF and `rubylight-vdisplay` health (polled off the Win32 UI thread), plus direct actions: **Show Pairing QR…**, **Repair drivers & service…** (elevates `setup.exe --repair` visibly via UAC; fails visibly if declined), **Restore displays now** (`display_recovery::reset()`), and **Open Diagnostics** (`/help`).

4. **Plain-English Error Rewrite**
   - Every user-visible error across host log analysis (`rust/core/session/src/log_analysis/`), host console warnings (`rust/host/stream/src/host_warnings.rs`), and Nightjar connection/decoder/native-API sheets follows a strict three-part template:
     1. **What happened** — 1–2 short plain-English sentences. Zero raw `HRESULT`, `0x887A0005`, `AMF_CODEC_NOT_SUPPORTED`, `WSAEWOULDBLOCK`, `ECONNREFUSED`, or Java exception class names in the primary text.
     2. **What to do next** — one concrete action or one-click fix button (e.g., "Restart Windows", "Switch to HEVC", "Repair drivers", "Open Firewall settings").
     3. **Technical details** — collapsed disclosure containing the exact error code, timestamp, codec/adapter context, and a **Copy details** button for bug reports.

5. **DPI & Display Crash-Restore Hardening (Issue #129)**
   - Fixes issue #129 (applications open prior to an exclusive/virtual-display stream returning oversized or mis-scaled after stream end or host crash) and hardens crash recovery for every Wave 0–3 host mutation on the single canonical `RecoveryJournalV2` (`rust/core/session/src/recovery_journal.rs`, persisted atomically to `recovery-journal.json` with the `--recover-session` service hook created in `08`).
   - Every host state mutation—per-monitor Windows DPI scale across ALL physical displays, resolution, refresh rate, rotation, HDR toggle, ICC profile, display arrangement/clone topology, audio sink & per-app audio route, microphone route, frame limiter, GPU driver profile, and Quick Suspend frozen process records—is committed to `recovery-journal.json` (`RecoveryJournalV2`) via atomic write-ahead journaling **BEFORE** the OS mutation executes.
   - Adds a bounded post-restore DPI settlement verifier (`0 ms` / `150 ms` / `500 ms`, capped at `1500 ms`) so asynchronous Windows Explorer/DWM DPI recalculation after monitor wake cannot silently override the restored scale.
   - Verified by an automated **10-cycle forced-kill / crash-restore soak test** (`TerminateProcess` injected at pre-apply, active-stream, and mid-restore windows, exercising `--recover-session` and startup recovery on `recovery-journal.json`).

6. **15 Nightjar + 22 Host Locales, Accessibility & Dynamic Input Glyph Sweep (All Surfaces)**
   - Brings every new and rewritten 7.0.0 string to 100% parity across **all 5 Rubylight surfaces** (Web Console + no-script HTML console, Browser `/play` & `/guest`, Host-Drawn Couch Menu `L3+R3`, Windows Installer + QR Finish Screen, System Tray & Notifications) in **all 22 host locale files** in `rust/assets/locale/*.json` (`bg`, `cs`, `de`, `en`, `en_GB`, `en_US`, `es`, `fr`, `hu`, `it`, `ja`, `ko`, `pl`, `pt`, `pt_BR`, `ru`, `sv`, `tr`, `uk`, `vi`, `zh`, `zh_TW`) and **all Nightjar form factors & redesigned surfaces** (Phone physics-sheets + Modern Virtual Controller Studio `#151` + Reverse Landscape `#152`, Handheld, Android TV / Fire TV 10-ft OS, Desktop Win/Linux/macOS with `#144`/`#146` scaling, Console Control Center floating dock & visual cards, Visual Preview-Card Settings, First-Run / Pairing Cinema, and Wave 1–3 overlays) in **all 15 Nightjar locales** (`en`, `de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`, with 100% of new Desktop translations routed exclusively to canonical chained `client/protocol/desktop-config/src/strings_70/<code>.rs` — zero lines added to grandfathered `strings/*.rs`!).
   - Audits and extends the single canonical **`client/protocol/policy/src/input_glyphs.rs`** refactored in `02` from `desktop/src/glyphs.rs` (**explicitly preserving Moonlight V+ port credits** in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README per `AGENTS.md`, never naming ports in UI text or release notes) so dynamic input glyphs switch instantaneously on the latest real input event across Console Home, First-Run/Pairing Cinema, Console Control Center floating dock & cards, Visual Preview-Card Settings, Virtual Controller Studio `#151`, Error Sheet, and Wave 1–3 overlays: Xbox, PlayStation (`0x054C`), Nintendo (`0x057E`, respecting Nintendo confirm/cancel swap), Steam Deck (`0x28DE`), Keyboard, TV Remote (`OK`/`Hold OK`), and **no hints for Touch / Mouse**.
   - Full TalkBack (Android — including physics-sheet detents, Control Center dock landmarks, Preview-Card descriptions, Virtual Controller Studio `#151` accessible editor actions), AccessKit & keyboard traversal (Desktop), and WCAG AA (`>= 4.5:1` text / `>= 3.0:1` UI & focus contrast) compliance across all 5 Rubylight surfaces + all Nightjar themes, with zero focus traps and strict `Motion.reduced()` / `prefers-reduced-motion` fidelity.

---

## 2. Protocol, Native API & Capability Contract

Per `docs/designs/00-protocol-reservations.md` §7, **Item 17 requires ZERO new control-stream message IDs, ZERO new input packet magics, ZERO `/serverinfo` changes, and ZERO RTSP `DESCRIBE`/`ANNOUNCE` changes.**

### 2.1 Wire & API Compatibility Invariants

| Contract Surface | Guarantee |
| --- | --- |
| `PROTOCOL_VERSION` | **Unchanged.** Stock Moonlight and older Nightjar clients negotiate and stream byte-for-byte identically. |
| `rubylight_native_api::API_VERSION` | **Stays `1`.** All schema additions below are strictly additive optional fields with `#[serde(default)]`. |
| Host Console `/api/metadata` | Adds optional `driver_health` object (`vdisplay`, `vgamepad`, `audio`, `firewall`, each `{ state, detail, repairable }`) and additive `title`, `next_step`, `action_id`, `technical_detail` fields on `warnings[]` (preserving existing `code` and `message`). |
| Host Console `/api/logs/analysis` | Preserves existing `Finding` fields (`id`, `category`, `level`, `title`, `explanation`, `fix`, `link`, `setting`, `count`, `first_seen`, `last_seen`, `sample`) and adds optional `action_id: Option<&'static str>` and `technical_code: Option<String>`. |
| Host Console `POST /api/system/repair` | New authenticated, CSRF-protected, `host_control`-scoped local console endpoint (not exposed to remote API tokens by default) that launches elevated `setup.exe --repair --from-console` and returns status `{ launched: bool, message: String }`. |
| Capability Gating (`HostCapabilities`) | Nightjar NEVER inspects `Family::Rubylight` / `Family.RUBYLIGHT` (`tools/arch_check.py` enforces this). Error sheets map protocol error codes, HTTP statuses, and Native API error strings uniformly across Rubylight, Sunshine, Apollo, Vibepollo, and GameStream hosts. |

### 2.2 Plain-English Error Envelope (Additive Native API v1 Compatibility)

Preserves the existing `ErrorBody { error: ApiError { code, message, setting, retry_after_ms } }` envelope in `client/protocol/native-api/src/error.rs` intact (NEVER flattens `error` into a bare string!). Adds `title`, `next_step`, and `technical_detail` strictly as optional `#[serde(default, skip_serializing_if = "Option::is_none")]` fields **inside `ApiError`**:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ApiError,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub setting: Option<String>,
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub technical_detail: Option<String>,
}
```

Wire JSON example:

```json
{
  "error": {
    "code": "driver_rejected",
    "message": "This PC cannot encode AV1 10-bit on the active graphics card.",
    "setting": null,
    "retry_after_ms": null,
    "title": "Video format not supported on this PC",
    "next_step": "Choose HEVC or H.264 in Nightjar video settings, or turn off 10-bit HDR.",
    "technical_detail": "AMF_CODEC_NOT_SUPPORTED (codec=av1, ten_bit=true, adapter=RX 6600)"
  }
}
```

Older Nightjar builds continue deserializing `ErrorBody { error: ApiError }` and ignore the unknown optional fields inside `ApiError`; Nightjar 7.0.0 renders `error.title`, `error.message`, `error.next_step`, and puts `error.technical_detail` inside the collapsed disclosure panel. If an older host omits `title`/`next_step`, Nightjar resolves them locally via `nightjar-policy::error_copy::explain()`.

---

## 3. Architecture & <=800-LOC Module Plan

All paths below must be re-verified against `docs/code-map.md` and `client/docs/code-map.md` in Phase 2 (`docs/code-map.md`).

### 3.1 Grandfathered Files in `tools/size_ratchet.json` (DO NOT GROW)

The following relevant files are grandfathered above the 800-LOC ceiling in `tools/size_ratchet.json`. Item 17 MUST NOT add net lines to any of them; instead, all new logic lives in fresh `<= 800`-LOC helper modules wired by thin delegation calls (and where touched, we shrink the grandfathered file by extracting existing helpers):

| Grandfathered File | Ratchet Ceiling | Strategy in Item 17 |
| --- | ---: | --- |
| `rust/setup/src/install.rs` | 2226 | Delegate VHF `rubylight-vgamepad` install & health check to new `rust/setup/src/vgamepad.rs`, repair workflow to `rust/setup/src/repair.rs`, and Instant QR Pairing Finish Screen to `rust/setup/src/finish_qr.rs`. |
| `rust/setup/src/system.rs` | 1732 | Keep PnP/driver-store queries for VHF in `rust/setup/src/vgamepad.rs`. |
| `rust/windows/io/src/tray.rs` | 827 | Extend canonical `rust/windows/io/src/tray/status.rs` (created in `01`) and extract menu construction to `rust/windows/io/src/tray/menu.rs`; keep `tray.rs` below 800 LOC. |
| `rust/host/stream/src/display_session.rs` | 3158 | Add pre-mutation DPI & Quick Suspend journal hook inside new `rust/host/stream/src/display_session/journal_tx.rs` writing canonical `RecoveryJournalV2`. |
| `rust/windows/display/src/display_recovery.rs` | 913 | Split Windows recovery executor into `rust/windows/display/src/display_recovery/mod.rs`, `restore.rs`, `dpi_settle.rs`, and `soak_tests.rs` consuming canonical `RecoveryJournalV2` from `rust/core/session/src/recovery_journal.rs` — bringing every file below 500 LOC! |
| `rust/windows/display/src/display/recovery.rs` | 1436 | Call `dpi_settle::verify_and_restore_scales()` instead of adding inline polling logic. |
| `client/app/src/main/java/com/limelight/console/ConsoleHomeActivity.java` | 1440 | Delegate cold-start deferral to new `StartupScheduler.java` and warm probe lookup to `DecoderCapabilityCache.java`. |
| `client/app/src/main/java/com/limelight/GameConnection.java` | 1942 | Delegate parallel pre-launch to `FastLaunchCoordinator.java` and errors to `ErrorSheet.java`. |
| `client/app/src/main/java/com/limelight/binding/video/MediaCodecHelper.java` | 1634 | Wrap probe reads through new `DecoderCapabilityCache.java` (1-line lookup hook). |
| `client/protocol/desktop/src/main.rs` | 1124 | Delegate deferred init to `app/startup.rs` and UI feedback to `ui/feedback.rs`. |
| `client/protocol/desktop-ui/src/ui/dialogs.rs` | 2537 | Put plain-English error modal in new `ui/error_sheet.rs`. |
| `client/protocol/desktop-config/src/strings/*.rs` (14 files) | 1750 each | **ZERO lines added to `strings/*.rs`:** Route all new 7.0.0 Desktop translations exclusively into per-locale `client/protocol/desktop-config/src/strings_70/<code>.rs` modules chained by `strings.rs` lookup so no existing grandfathered locale file exceeds its 1750 ceiling! |

### 3.2 New & Refactored `<= 800`-LOC Module Plan

| Module Path | Target LOC | Responsibility |
| --- | ---: | --- |
| `rust/core/session/src/log_analysis/mod.rs` | ~220 | Engine (`analyze`, `classify`, `Finding`, `Rule`), merging split rule tables. Replaces monolithic 550-LOC `log_analysis.rs` (kept under 800). |
| `rust/core/session/src/log_analysis/rules_encoder.rs` | ~220 | Plain-English AMF, NVENC, PyroWave, codec, HDR-to-SDR tone-map rules (never claiming NVENC tested on HW; checked against NVENC SDK error codes). |
| `rust/core/session/src/log_analysis/rules_display.rs` | ~220 | Plain-English IDD virtual display, WGC, Desktop Duplication, Win10 D3D11, DPI scale #129, and Quick Suspend rules. |
| `rust/core/session/src/log_analysis/rules_driver_net.rs` | ~220 | Plain-English `rubylight-vgamepad` VHF, audio sink, firewall, port conflict, STUN/Away Wake Relay, and packet-loss rules. |
| `rust/core/session/src/error_catalog.rs` | ~260 | Shared plain-English warning & Native API error catalog (`title`, `explanation`, `next_step`, `action_id`) with unit tests asserting zero raw `0x`/`HRESULT`/`errno` tokens in top-level strings. |
| `rust/setup/src/vgamepad.rs` | ~320 | `rubylight-vgamepad` VHF driver install, pnputil staging, signature check, device-node health query, and safe removal. |
| `rust/setup/src/repair.rs` | ~340 | `--repair` orchestrator: validates `rubylight-vdisplay`, `rubylight-vgamepad`, virtual audio, service registration, firewall rules, Start Menu shortcut; generates plain-English repair summary. |
| `rust/setup/src/finish_qr.rs` | ~240 | Redesigned 7.0 Installer **Instant QR Pairing Finish Screen**: queries local host pairing payload/PIN once installed service starts, renders high-contrast QR matrix + PIN + **Open Web Console** / **Finish** buttons styled through canonical `rust/setup/src/ui_theme.rs`, and degrades cleanly when service start is deferred. |
| `rust/setup/src/ui_theme.rs` *(consume/extend single canonical setup theme module created in `01`)* | ~260 | Consumes and extends canonical 7.0 visual tokens & Win32/Common Controls 6 dark-chrome/brand-icon renderer from `01` with Per-Monitor-V2 DPI-aware installer, QR finish screen, and `--repair` progress styling (zero WebView2 dependency). |
| `rust/windows/io/src/tray/status.rs` *(extend canonical module created in `01`)* | ~240 | Extends canonical 7.0 tray status/badge module from `01` with 16/24/32/48 px DPI scales + cached `rubylight-vgamepad` VHF & `rubylight-vdisplay` driver health snapshot (polled off the Win32 UI thread). |
| `rust/windows/io/src/tray/menu.rs` | ~260 | Tray context menu & notification builder (`Show Pairing QR…`, `Repair drivers & service…`, `Restore displays now`, `Open Diagnostics`, dynamic app/Quick Suspend items). |
| `rust/core/session/src/recovery_journal.rs` *(harden canonical `RecoveryJournalV2` created in `08`)* | ~320 | Single canonical atomic write-ahead journal (`RecoveryJournalV2` persisted to `recovery-journal.json` with `File::sync_all()` + atomic replace), hardened with `scale_requested` and `physical_dpi_baseline` alongside `08`'s Quick Suspend frozen-process entries. |
| `rust/windows/display/src/display_recovery/restore.rs` | ~380 | Idempotent crash-restore executor driven by `--recover-session` and startup recovery: reads `RecoveryJournalV2` (`recovery-journal.json`), thaws Quick Suspend frozen processes (verified by `GetProcessTimes`), restores frame limiter, audio/mic routes, arrangement, HDR/ICC/rotation/mode, and per-monitor DPI scales. |
| `rust/windows/display/src/display_recovery/dpi_settle.rs` | ~240 | Issue #129 bounded DPI settlement verifier (`0` / `150` / `500 ms` retries, max `1500 ms` budget) catching delayed Explorer/DWM DPI reversion after monitor re-attach. |
| `rust/windows/display/src/display_recovery/soak_tests.rs` | ~320 | Deterministic fault-injection & `RecoveryJournalV2` round-trip tests (plus `#[ignore]` live Windows DPI/soak harness). |
| `rust/host/stream/src/display_session/journal_tx.rs` | ~260 | Transaction guard wrapping display/DPI/audio/suspend mutations into `RecoveryJournalV2` (`recovery-journal.json`): commits `requested` intent BEFORE OS call, finalizes `applied` after OS call, rolls back cleanly on early error. |
| `rust/host/web/src/web/system_repair.rs` | ~180 | `GET /api/system/driver-health` and `POST /api/system/repair` (CSRF + local-admin guard + UAC elevation launch of `setup.exe --repair`). |
| `rust/tests/crash_restore_soak.py` | ~360 | 10-cycle forced-kill (`TerminateProcess`) crash-restore soak harness against an isolated host on port `48123` (`--bind 127.0.0.1`), exercising `RecoveryJournalV2` (`recovery-journal.json`) + `--recover-session`. |
| `client/protocol/policy/src/error_copy.rs` | ~320 | Host-neutral plain-English error mapper shared by Android (via JNI) and Desktop (`ErrorExplanation { title, what_happened, next_step, action, technical_detail }`). |
| `client/protocol/policy/src/input_glyphs.rs` *(audit/extend canonical module created in `02`)* | ~280 | Audits and extends the single canonical dynamic glyph resolver refactored in `02` from `desktop/src/glyphs.rs` (`Xbox`, `PlayStation`, `Nintendo`, `SteamDeck`, `Keyboard`, `Remote`, `Touch` — no hints for touch/mouse — from VID/PID + event source + Nintendo A/B swap); **explicitly preserves Moonlight V+ port credits** in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README. |
| `client/protocol/policy/src/feedback.rs` | ~210 | Pure policy for console-grade UI soundscape cues (`FocusTick`, `Confirm`, `Back`, `ControlCenterOpen`, `ControlCenterClose`, `DvrClipChime`, `PairingChime`, `CurtainLift`, `SuspendToggle`, `Reject`, `SheetDetent`, `PadSnap`) & spring-synced haptic triggers: respects `ui_sounds` opt-in, mute/DND, reduced-motion throttle, and **strictly suppresses all UI sound/rumble when `stream_active == true`**. |
| `client/app/src/main/java/com/limelight/ui/UiSoundscape.java` | ~260 | Android `SoundPool` micro-sound player with synthesized 44.1 kHz PCM console cues (focus tick, confirm, back, Control Center open/close, DVR clip chime, pairing chime) loaded off main thread; releases `SoundPool` on `Game.onCreate()` before `AndroidAudioRenderer` starts (zero stream latency). |
| `client/app/src/main/java/com/limelight/ui/ErrorSheet.java` | ~280 | Nightjar 7.0 console-quality plain-English error physics-sheet with primary action button, secondary help link, and collapsed "Technical details" + Copy button. |
| `client/app/src/main/java/com/limelight/ui/InputGlyphs.java` | ~240 | Android glyph presenter wrapping `nightjar-policy::input_glyphs` across Console Home, First-Run/Pairing Cinema, Control Center floating dock, Visual Preview-Card Settings, and Virtual Controller Studio `#151` (replaces ad-hoc `SONY` vendor check in `InputSource.java`). |
| `client/app/src/main/java/com/limelight/startup/StartupScheduler.java` | ~220 | Android cold-start lane coordinator: paints cached 7.0 Console Home first, defers mDNS/TLS/USB/cover-scan until `Choreographer` posts frame 1 + `reportFullyDrawn()`. |
| `client/app/src/main/java/com/limelight/startup/DecoderCapabilityCache.java` | ~260 | Persisted JSON cache of `MediaCodecHelper` hardware capabilities keyed by `Build.FINGERPRINT` + MediaCodec HAL signature + `Build.TIME`. |
| `client/app/src/main/java/com/limelight/startup/FastLaunchCoordinator.java` | ~280 | Parallel pre-launch fan-out (host readiness, decoder warm-check, audio pre-open, Launch Curtain sync) with strict cancellation on back press. |
| `client/protocol/desktop/src/app/startup.rs` | ~240 | Desktop cold-start deferred init + GPU/driver-keyed warm decoder capability cache (`decoder_caps_cache.json`). |
| `client/protocol/desktop/src/app/fast_launch.rs` | ~240 | Desktop parallel pre-launch orchestrator feeding Launch Curtain (`0x5548`). |
| `client/protocol/desktop-ui/src/ui/feedback.rs` | ~280 | Desktop synthesized PCM console soundscape (focus tick, confirm, back, Control Center open/close, DVR clip chime, pairing chime; SDL3 audio stream closed before stream audio opens) + spring-synced gamepad menu rumble ticks. |
| `client/protocol/desktop-ui/src/ui/error_sheet.rs` | ~240 | Desktop Nightjar 7.0 console-quality plain-English error sheet with AccessKit labels and collapsible technical details. |
| `client/protocol/desktop-ui/src/ui/glyphs.rs` | ~220 | Desktop dynamic input glyph painter backed by `nightjar_policy::input_glyphs`. |
| `client/protocol/desktop-config/src/strings_70/<code>.rs` (14 non-English locales) | ~180 each | Canonical chained 7.0.0 Desktop translation modules for all 14 non-English locales, keeping grandfathered `strings/*.rs` untouched at `<= 1750` LOC. |
| `tools/check_i18n.py` | ~300 | Automated locale completeness across **all 22 host locale files** (`rust/assets/locale/*.json`) covering all 5 Rubylight surfaces + **all 15 Nightjar locales** across Android (`values-*/strings.xml`) & Desktop (`strings/` + `strings_70/`, asserting 0 lines added to `strings/*.rs`), placeholder parity, zero-bare-literal, zero-`"Experimental"`, and zero-mod-name verifier. |

### 3.3 Cold-Start & Fast-Launch Execution Timeline

```text
Android / Desktop Cold Start (Target <= 250 ms to interactive frame)
====================================================================
t = 0 ms     Process entry -> read minimal SharedPrefs / config.toml (theme + cached Console Home snapshot)
t = 25 ms    Inflate/create main window + paint Nightjar 7.0 Console Home from cached ConsoleSnapshot
t = 80 ms    First frame presented -> reportFullyDrawn() / first egui swap (UI interactive!)
             [Post-Frame-1 Background Lanes — NEVER block main thread]
t = 90 ms    Lane A (I/O): load persisted DecoderCapabilityCache (validate fingerprint/driver; re-probe only on miss)
             Lane B (Net): start mDNS/UDP discovery & paired host status polls
             Lane C (Init): warm TLS certs, USB enumerator, UiSoundscape PCM buffers (if ui_sounds enabled)

Stream Tap -> First Decoded Frame (Parallel Pre-Launch)
=======================================================
Tap "Play" -> show Launch Curtain immediately (<= 16 ms)
  +-> Task 1: Wake-on-LAN / Away Wake Relay (if asleep) + HTTPS /serverinfo + /launch
  +-> Task 2: Verify cached decoder profile + allocate Surface / Vulkan/D3D11/Metal swapchain
  +-> Task 3: Close UiSoundscape handle (0 ms contention) + pre-open AAudio/WASAPI/CoreAudio buffer
  All join -> RTSP handshake + ENet control + first IDR -> lift Launch Curtain
```

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### 4.1 Display, DPI & Quick Suspend Crash-Restore State Machine (Issue #129)

```text
                 +-------------------+
                 |       CLEAN       | (no recovery-journal.json)
                 +---------+---------+
                           |
         begin_tx(): capture Snapshot of ALL physical monitors
         (mode, refresh, HDR, rotation, clone, positions, DPI scale),
         audio/mic route, limiter, driver profile
                           v
                 +-------------------+
                 |  INTENT_JOURNALED | <-- durable write + fsync to recovery-journal.json BEFORE any OS call!
                 |  (requested=true) |     armed for --recover-session service hook & startup recovery
                 +---------+---------+
                           |
              apply OS mutations (IDD, SetDisplayConfig,
              set_dpi_scale, audio_route, Quick Suspend freeze)
                           |
             +-------------+-------------+
             | success                   | OS error OR TerminateProcess / crash
             v                           v
   +-------------------+       +-------------------+
   |   COMMITTED_LIVE  |       |  RECOVERY_PENDING | <-- --recover-session service hook OR next host start
   |  (applied saved,  |       +---------+---------+
   |   requested=false)|                 |
   +---------+---------+                 | recover() [idempotent, best-effort every step]:
             |                           |  1. Thaw Quick Suspend entries in RecoveryJournalV2 (verify GetProcessTimes)
      clean stream end                   |  2. Restore frame limiter & GPU driver profile
             |                           |  3. Restore audio & mic routes
             v                           |  4. Restore display arrangement / clone groups
   +-------------------+                 |  5. Restore per-monitor rotation, mode+rate, HDR, ICC
   |     RESTORING     |<----------------+  6. Restore per-monitor DPI scale + dpi_settle loop
   +---------+---------+                       (0 ms, 150 ms, 500 ms; <= 1500 ms total)
             |                           |  7. Keep entries ONLY for disconnected/failed monitors
    +--------+--------+
    | all ok          | monitor unplugged / transient failure
    v                 v
+-------+     +---------------+
| CLEAN |     | PARTIAL_KEPT  | (stays in recovery-journal.json with pid=0 until monitor returns
+-------+     +---------------+  or next stream adopts original baseline)
```

### 4.2 Canonical `RecoveryJournalV2` (`rust/core/session/src/recovery_journal.rs`, `recovery-journal.json`)

Item 17 hardens and 10-cycle soaks the **single canonical `RecoveryJournalV2`** created in `08` (`rust/core/session/src/recovery_journal.rs`, persisted atomically to `recovery-journal.json` and recovered via the `--recover-session` service hook — zero parallel journal files or duplicate suspended-PID schemas). Backwards-compatible with v1 (`#[serde(default)]` on every new field):

```rust
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct RecoveryJournalV2 {
    #[serde(default = "default_v2")]
    pub schema_version: u32,           // 2 (reads v1 as 1)
    pub pid: u32,
    pub started: u64,                  // GetProcessTimes creation timestamp
    pub entries: BTreeMap<String, Entry>,
    /// All physical monitors' pre-mutation DPI scale (device_id -> original_pct),
    /// captured BEFORE any exclusive/extended layout switch (fixes #129).
    #[serde(default)]
    pub physical_dpi_baseline: BTreeMap<String, u32>,
    /// Canonical Quick Suspend frozen-process entries from 08; thawed by
    /// `--recover-session` / startup recovery only if GetProcessTimes(pid)
    /// matches the recorded creation timestamp (prevents PID-reuse races).
    #[serde(default)]
    pub quick_suspend: Option<QuickSuspendRecovery>,
}
```

Per-display `Entry` adds `#[serde(default)] pub scale_requested: bool;`:
- Before calling `set_dpi_scale(m, requested)` or switching to an exclusive virtual display, `journal_tx` persists `scale = Some((before, requested))` with `scale_requested = true` AND snapshots every active physical monitor into `physical_dpi_baseline` inside `recovery-journal.json`.
- On recovery, `restore_scale_needed(scale_requested, current, applied, previous)` mirrors `restore_mode_needed`: if `scale_requested == true` (crash mid-mutation), restore `previous` whenever `current != previous`; if `scale_requested == false` (committed), restore `previous` when `current == applied` OR when `physical_dpi_baseline` shows Windows shifted a physical monitor away from its pre-stream DPI during monitor re-attach (#129).
- `dpi_settle::verify_and_restore_scales()` re-queries `dpi_scale(m)` at `0 ms`, `150 ms`, and `500 ms` (aborting early as soon as all attached monitors match `physical_dpi_baseline` for two consecutive checks, capped at `1500 ms` total) and logs `dpi_settled_after_ms` for diagnosis.

### 4.3 Reconnect & Failure Matrix

| Scenario | Host Behavior | Nightjar Behavior | Recovery / Journal Outcome |
| --- | --- | --- | --- |
| Clean stream disconnect & reconnect | Keeps `physical_dpi_baseline` while paused if `keep_while_paused`; restores + DPI-settles after `dd_config_revert_delay`. | Reuses `DecoderCapabilityCache`; skips re-probe; UI soundscape re-arms only after stream audio closes. | `recovery-journal.json` clears to empty cleanly. |
| Host killed (`TerminateProcess`) BEFORE `SetDisplayConfig` completes | `INTENT_JOURNALED` already flushed on disk in `recovery-journal.json` (`scale_requested=true`, `mode_rate_requested=true`). | Shows plain-English "Connection to PC was lost" sheet with **Reconnect** button (zero raw socket errno). | Service supervisor invokes `--recover-session` immediately on host exit (or next host startup runs `recover()` + `dpi_settle`), restoring exact DPI/mode/HDR/audio. |
| Host killed WHILE game is frozen via Quick Suspend | `RecoveryJournalV2` (`recovery-journal.json`) holds `08`'s frozen process records + creation timestamps. | On next host poll, `/session/suspend` shows thawed/running or cleanly ended state. | `--recover-session` service hook (or startup recovery) verifies `GetProcessTimes` and calls `NtResumeProcess` on every frozen process before tearing down displays. |
| Host killed AND service supervisor restarts host | Service supervisor invokes `--recover-session` and restarts `butterpollo.exe`. | Retries reconnect within grace window or shows plain-English reconnect prompt. | Startup recovery detects dead `journal.pid` in `recovery-journal.json` and executes the exact same idempotent `recover()` + `dpi_settle` path. |
| Monitor unplugged before crash-restore | Unplugged monitor's entry stays in `kept` with `pid=0`; connected monitors restore completely. | Host console surfaces plain-English notice: "One monitor was unplugged when streaming ended; its settings will restore when reconnected." | Next host start or next stream adopts the saved original values so they are never overwritten. |
| Driver update or OS upgrade between Nightjar launches | Unchanged. | `DecoderCapabilityCache` detects mismatched `Build.FINGERPRINT` or GPU driver version, invalidates cache, re-probes asynchronously off frame 1. | No stale codec flags or black-screen decode attempts. |
| User declines UAC on `Repair drivers & service...` | Tray / Console receives `ERROR_CANCELLED (1223)` and shows plain-English "Administrator approval is needed to repair drivers. Nothing was changed." | N/A | Zero partial driver mutations. |
| Installer finishes before host service binds local pairing port | `finish_qr.rs` polls briefly (bounded `<= 4 s`), displays clean status + **Open Web Console** fallback instead of hanging or showing a broken QR. | First-Run / Pairing Cinema scans QR once live or accepts manual LAN/PIN entry cleanly. | Zero installer deadlock; pairing succeeds cleanly. |

---

## 5. Settings & 10/10 Craft Plan

- **Fewer-Settings Rule**:
  - **Zero new host settings** for DPI crash-restore, plain-English errors, Installer QR Finish Screen, or tray driver health — they are always-on correctness & craft improvements.
  - **One opt-in client setting**: `ui_sounds` (`bool`, default **`false`** — default-off gate until reviewed/measured across platforms; stored in `PreferenceConfiguration` / desktop `Settings`, migrated cleanly, registered in `SettingsKeysTest` and `generate-settings.py`).
  - **Zero new settings** for spring-synchronized micro-haptics, cold-start caching, parallel pre-launch, dynamic input glyphs, or TalkBack/AccessKit accessibility — always right by default.
  - Help text for `ui_sounds` follows the exact two-sentence rule across all 15 Nightjar locales: *"Plays soft console sounds when you move through Nightjar's menus and save clips. Turns off automatically during a stream so it never affects game audio."*
- **Plain-English Error States**:
  - Every `Rule` in `log_analysis/`, every warning in `host_warnings.rs`, and every `ErrorExplanation` in `nightjar-policy::error_copy` is unit-tested against a forbidden-token regex (`0x[0-9A-Fa-f]{4,}`, `HRESULT`, `DXGI_ERROR_`, `AMF_`, `NV_ENC_`, `WSA`, `Exception`, `Errno`, `panic`) on primary `title` / `explanation` / `next_step` fields. Technical strings live exclusively in `technical_detail` / `sample`.
- **Dynamic Input Glyphs Across Every Redesigned Surface**:
  - Audits and extends the single canonical `client/protocol/policy/src/input_glyphs.rs` refactored in `02` from `desktop/src/glyphs.rs`, **explicitly preserving Moonlight V+ port credits** in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README (and never naming ports in UI text or release notes).
  - Live switching on every `KeyEvent` / `MotionEvent` / SDL3 event across Console Home, Game Detail, First-Run / Pairing Cinema, Visual Preview-Card Settings, Console Control Center floating dock & visual cards, Virtual Controller Studio (`#151`), Error Sheet, Launch Curtain, Input Deck, Coherence, Party Audio, and Stream Doctor 2.0.
  - Supports Xbox, PlayStation (`0x054C`), Nintendo (`0x057E` with confirm/cancel swap), Steam Deck (`0x28DE`), Keyboard, TV Remote (`OK`/`Hold OK`), and **no hints for Touch / Mouse**.
- **Zero Stream-Time Blur When Closed**:
  - `ErrorSheet` during a stream and `UiFeedback` never leave blur layers running over active video frames; `ErrorSheet` uses opaque/tinted 7.0 console surfaces or obeys Control Center's `#61` single-window blur teardown rule so post-close GPU cost equals pre-open cost.
- **WCAG Contrast & Reduced-Motion Fidelity**:
  - `>= 4.5:1` text contrast and `>= 3.0:1` UI/focus contrast verified across all 5 Rubylight surfaces (including Installer QR Finish Screen in `ui_theme.rs` and System Tray badges in `tray/status.rs`) and all Nightjar themes; `Motion.reduced()` / `prefers-reduced-motion` suppresses repeat focus ticks and spring overshoot cleanly.
- **Zero "Experimental" in UI & Zero Mod Names**:
  - Automated CI/lint check (`tools/check_i18n.py`) fails if `"Experimental"` appears in any user-facing UI string table, or if any mod name (`Nucleus Co-op` / `ANR`) appears in Rubylight Web Console, `/play`, `/guest`, Couch Menu, Installer/QR Finish Screen, System Tray, or Nightjar UI strings (`GPL-3.0 §7(e)`).
- **Host (22 Locales) & Nightjar (15 Locales)**:
  - Complete parity verified across **all 22 host locale files** in `rust/assets/locale/*.json` (`bg`, `cs`, `de`, `en`, `en_GB`, `en_US`, `es`, `fr`, `hu`, `it`, `ja`, `ko`, `pl`, `pt`, `pt_BR`, `ru`, `sv`, `tr`, `uk`, `vi`, `zh`, `zh_TW`) across all 5 Rubylight surfaces, and **all 15 Nightjar locales** (`en`, `de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`) in Android (`res/values-*/strings.xml`) and Desktop (canonical chained `client/protocol/desktop-config/src/strings_70/<code>.rs`, 0 lines added to grandfathered `strings/*.rs`).
- **Platform & Encoder Parity**:
  - Installer & tray keep Windows 10 and Windows 11, physical displays, WGC, Desktop Duplication, D3D11, AMD AMF, and NVIDIA NVENC working. NVENC error-code mappings (`NV_ENC_ERR_*`) are checked strictly against NVIDIA Video Codec SDK headers/samples (`docs/nvenc.md#checking-nvidia-code`) and never claimed as run on NVIDIA hardware.

---

## 6. Disjoint Implementer Slices

Every slice below has a **strictly non-overlapping file list** so Wave 4 implementers can run in `parallel()` without merge conflicts.

### Slice 17A: Host DPI & Quick Suspend Crash-Restore Journal + Soak Harness (`#129`)
- **Files (exclusive)**:
  - `rust/core/session/src/recovery_journal.rs` *(harden canonical `RecoveryJournalV2` + `recovery-journal.json` schema created in `08`)*
  - `rust/windows/display/src/display_recovery.rs` *(converted to module directory `rust/windows/display/src/display_recovery/mod.rs`)*
  - `rust/windows/display/src/display_recovery/restore.rs` *(new; invoked by `--recover-session` & startup recovery)*
  - `rust/windows/display/src/display_recovery/dpi_settle.rs` *(new)*
  - `rust/windows/display/src/display_recovery/soak_tests.rs` *(new)*
  - `rust/windows/display/src/display/recovery.rs`
  - `rust/host/stream/src/display_session/journal_tx.rs` *(new)*
  - `rust/host/stream/src/display_session.rs` *(thin 10-line delegation to `journal_tx`; net LOC <= ratchet)*
  - `rust/host/stream/src/service_tasks.rs` *(ensure `--recover-session` service hook invokes hardened `RecoveryJournalV2` + `dpi_settle` path)*
  - `rust/tests/crash_restore_soak.py` *(new)*

### Slice 17B: Redesigned 7.0 Installer, QR Finish Screen & System Tray Polish with `rubylight-vgamepad`
- **Files (exclusive)**:
  - `rust/setup/src/vgamepad.rs` *(new)*
  - `rust/setup/src/repair.rs` *(new)*
  - `rust/setup/src/finish_qr.rs` *(extend canonical Instant QR Pairing Finish Screen created in Wave 0 Slice `01-F`, styled via canonical `ui_theme.rs` — zero duplicate `finish_qr.rs`)*
  - `rust/setup/src/ui_theme.rs` *(consume/extend single canonical setup theme module created in `01`)*
  - `rust/setup/src/install.rs` *(thin delegation to `vgamepad.rs`, `repair.rs`, `finish_qr.rs`; net LOC <= ratchet)*
  - `rust/setup/src/ui.rs`
  - `rust/core/session/src/tray.rs`
  - `rust/windows/io/src/tray.rs` *(thin delegation; keep `<= 800` LOC)*
  - `rust/windows/io/src/tray/status.rs` *(extend canonical module created in `01`)*
  - `rust/windows/io/src/tray/menu.rs` *(new)*
  - `rust/host/web/src/web/system_repair.rs` *(new)*
  - `rust/host/web/src/web.rs` *(register `/api/system/driver-health`, `/api/system/repair`; net <= ratchet)*

### Slice 17C: Host Plain-English Errors (`log_analysis` & `host_warnings`)
- **Files (exclusive)**:
  - `rust/core/session/src/log_analysis.rs` *(converted to `rust/core/session/src/log_analysis/mod.rs`)*
  - `rust/core/session/src/log_analysis/rules_encoder.rs` *(new)*
  - `rust/core/session/src/log_analysis/rules_display.rs` *(new)*
  - `rust/core/session/src/log_analysis/rules_driver_net.rs` *(new)*
  - `rust/core/session/src/error_catalog.rs` *(new)*
  - `rust/host/stream/src/host_warnings.rs`
  - `client/protocol/native-api/src/error.rs` *(preserve `ErrorBody { error: ApiError }`; add optional `#[serde(default, skip_serializing_if = "Option::is_none")]` `title`, `next_step`, `technical_detail` inside `ApiError`)*

### Slice 17D: Shared Client Policy (`nightjar-policy`) for Errors, Glyphs & Console Soundscape/Haptics
- **Files (exclusive)**:
  - `client/protocol/policy/src/lib.rs`
  - `client/protocol/policy/src/error_copy.rs` *(new)*
  - `client/protocol/policy/src/input_glyphs.rs` *(audit/extend canonical module created in `02`; preserve Moonlight V+ port credits in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README)*
  - `client/protocol/policy/src/feedback.rs` *(new)*
  - `client/protocol/android/src/policy_ux.rs` *(new JNI bridge for error_copy, input_glyphs, feedback)*
  - `client/protocol/android/src/lib.rs` *(wire `policy_ux` module; net <= ratchet)*

### Slice 17E: Nightjar Android Cold-Start, Fast Launch, Console Soundscape, Spring-Synced Haptics, ErrorSheet & A11y
*(Depends on 17D JNI bridge)*
- **Files (exclusive)**:
  - `client/app/src/main/java/com/limelight/ui/Haptics.java` *(spring-synced Console Home, Control Center, physics-sheet detent & Virtual Controller Studio `#151` micro-haptics)*
  - `client/app/src/main/java/com/limelight/ui/Springs.java` *(thin spring-detent callback hook to `Haptics` & `UiSoundscape`)*
  - `client/app/src/main/java/com/limelight/ui/UiSoundscape.java` *(new)*
  - `client/app/src/main/java/com/limelight/ui/ErrorSheet.java` *(new)*
  - `client/app/src/main/java/com/limelight/ui/InputGlyphs.java` *(new)*
  - `client/app/src/main/java/com/limelight/ui/InputSource.java`
  - `client/app/src/main/java/com/limelight/startup/StartupScheduler.java` *(new)*
  - `client/app/src/main/java/com/limelight/startup/DecoderCapabilityCache.java` *(new)*
  - `client/app/src/main/java/com/limelight/startup/FastLaunchCoordinator.java` *(new)*
  - `client/app/src/main/java/com/limelight/console/ConsoleHomeActivity.java` *(thin delegation; net <= ratchet)*
  - `client/app/src/main/java/com/limelight/GameConnection.java` *(thin delegation; net <= ratchet)*
  - `client/app/src/main/java/com/limelight/binding/video/MediaCodecHelper.java` *(1-line cache lookup; net <= ratchet)*
  - `client/app/src/main/java/com/limelight/preferences/PreferenceConfiguration.java` *(add `ui_sounds`; net <= ratchet)*
  - `client/app/src/test/java/com/limelight/startup/ColdStartAndCacheTest.java` *(new)*
  - `client/app/src/test/java/com/limelight/ui/ErrorSheetAndGlyphsTest.java` *(new)*

### Slice 17F: Nightjar Desktop Cold-Start, Fast Launch, Console Soundscape, ErrorSheet, Glyphs & AccessKit
*(Depends on 17D policy crate)*
- **Files (exclusive)**:
  - `client/protocol/desktop/src/app/startup.rs` *(new)*
  - `client/protocol/desktop/src/app/fast_launch.rs` *(new)*
  - `client/protocol/desktop-ui/src/ui/feedback.rs` *(new — console soundscape cues + spring-synced gamepad menu rumble)*
  - `client/protocol/desktop-ui/src/ui/spring.rs` *(thin spring-settle notification hook to `ui/feedback.rs`)*
  - `client/protocol/desktop-ui/src/ui/error_sheet.rs` *(new)*
  - `client/protocol/desktop-ui/src/ui/glyphs.rs` *(new)*
  - `client/protocol/desktop/src/main.rs` *(thin delegation; net <= ratchet)*
  - `client/protocol/desktop-config/src/config.rs` *(add `ui_sounds: false`; net <= ratchet)*
  - `client/protocol/desktop-ui/src/ui/settings.rs` *(add `ui_sounds` row; net <= ratchet)*

### Slice 17G: All-Surface Locale Completeness Sweep (22 Host / 15 Nightjar) & i18n / A11y Linter
*(Runs after 17C/17E/17F freeze English strings)*
- **Files (exclusive)**:
  - `tools/check_i18n.py` *(new)*
  - `rust/assets/locale/*.json` *(all **22** host locale files covering Web Console, `/play`, `/guest`, Couch Menu, Installer/QR Finish, and Tray)*
  - `client/app/src/main/res/values/strings.xml` and `values-*/strings.xml` *(all **15** Nightjar locales: base English + 14 translated XMLs covering Phone/Handheld/TV, Control Center, Preview-Card Settings, First-Run/Pairing Cinema, Virtual Controller Studio `#151`, and Wave 1–3)*
  - `client/protocol/desktop-config/src/strings.rs`
  - `client/protocol/desktop-config/src/strings_70/<code>.rs` *(all new 7.0.0 Desktop translations across the 14 non-English locales; **0 lines added** to grandfathered `strings/*.rs`)*

---

## 7. Verification & Measurement Plan

### 7.1 Narrowest Unit, Policy & Contract Tests (No GPU / No Hardware Required)

1. **Host Core, Setup & Tray**:
   - `cargo test -p butterpollo-core --locked log_analysis error_catalog tray recovery_journal` — verifies every error rule matches its log signature, zero raw `HRESULT`/`0x` tokens appear in top-level plain-English fields, every `anchor` exists in `docs/troubleshooting.md`, and `RecoveryJournalV2` round-trips cleanly.
   - `cargo test -p butterpollo-setup --locked vgamepad repair finish_qr ui_theme` — unit tests for `--repair` idempotency, Instant QR Pairing Finish Screen payload/fallback rendering, side-by-side port preservation, canonical `ui_theme` styling, and VHF driver status parsing.
   - `cargo test -p butterpollo-windows --locked display_recovery tray` — `RecoveryJournalV2` (`recovery-journal.json`) v1->v2 upgrade, `scale_requested` recovery matrix, `dpi_settle` mock convergence, `--recover-session` PID-reuse guard (`GetProcessTimes` mismatch skips thaw), and canonical `tray/status.rs` 16/24/32/48 px DPI badge rendering.
2. **Client Policy, Android & Desktop**:
   - `cargo test -p nightjar-policy --locked error_copy input_glyphs feedback` — verifies stream-active hard mute of UI soundscape/rumble, full console cue table (focus tick, confirm, back, Control Center open/close, DVR clip chime, pairing chime), reduced-motion throttling, Xbox/PlayStation/Nintendo/SteamDeck/Keyboard/Remote/Touch (hidden) glyph matrices (preserving Moonlight V+ credits), and plain-English error fallback.
   - `cargo test -p nightjar-desktop --locked startup fast_launch error_sheet glyphs feedback` — verifies cold-start cache hit/invalidation, SDL soundscape release before stream start, spring-synced rumble suppression during streams, and AccessKit labels.
   - `.\gradlew.bat :app:testNonRootDebugUnitTest --tests "com.limelight.startup.*" --tests "com.limelight.ui.ErrorSheetAndGlyphsTest" --tests "com.limelight.preferences.SettingsKeysTest"` — verifies Robolectric cold-start scheduling, cache fingerprint invalidation, `Haptics` API-level constants & spring detent hooks, `UiSoundscape` stream-start teardown, and `SettingsKeysTest` key discipline.
3. **Architecture & Multi-Locale Sweep**:
   - `python tools/arch_check.py` — enforces `<= 800` LOC on all new modules, zero growth on grandfathered files (including 0 lines added to `strings/*.rs`), zero `Family::Rubylight` / `Family.RUBYLIGHT` gating, and `//!` / `/**` module headers.
   - `python tools/check_i18n.py` — 100% key and format-placeholder parity across all **22** host locale files (`rust/assets/locale/*.json`) and all **15** Nightjar locales (`values-*/strings.xml` + `strings/` & `strings_70/`), zero `"Experimental"` in UI, zero mod names in UI.
   - `python tools/code_map.py check` & `python tools/generated_docs.py --check`.

### 7.2 Visual & Accessibility Checks

- **Desktop CPU screenshots** (`cargo test -p nightjar-desktop --locked shot`): render `ErrorSheet` (collapsed & expanded details), Console Control Center dynamic glyphs (Xbox / PS / Nintendo / Steam Deck / Keyboard / hidden on Touch/Mouse), and Visual Preview-Card Settings `ui_sounds` row in Night, Day, OLED Black, and High Contrast modes.
- **Android emulator TalkBack & cold-start trace** (`adb shell am start -W -S com.butterpollo.client.debug/.console.ConsoleHomeActivity`): measure `TotalTime` & `WaitTime` across 10 cold starts after cache warmup; gate: **p95 <= 250 ms**. Verify TalkBack traversal across Console Home physics-sheets, First-Run/Pairing Cinema, Console Control Center dock, Preview-Card Settings, and Virtual Controller Studio (`#151`) plus `48×48 dp` touch targets via `gradlew :app:lintNonRootDebug`.
- **Rubylight 7.0 Playwright & Win32 Captures** (`rust/web/scripts/screenshots.cjs` + `butterpollo-setup` / `tray` checks): capture Driver Health card (`rubylight-vgamepad` VHF + `rubylight-vdisplay`), Repair modal, Plain-English Diagnostics findings at `1440 px` and `390 px` in Light & Dark themes, plus Installer + Instant QR Pairing Finish Screen (`ui_theme.rs`) and System Tray badges (`tray/status.rs`) on Windows 10/11 light and dark taskbars.

### 7.3 Serialized Idle-Gated Hardware Soak & Measurement Criteria

All GPU / live Windows display runs MUST obey the hardware gate:
1. Confirm the user is idle and `(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` shows `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0`.
2. Run exclusively on an isolated test host (`butterpollo.exe --config-dir C:\tests\ux-soak --port 48123 --bind 127.0.0.1`) — NEVER against the installed service.
3. **Exit code 3 abort rule**: if `e2e.py`, `check.ps1`, or `crash_restore_soak.py` returns **exit code 3** (AMD encoder hang), **STOP immediately and investigate — NEVER retry**.
4. **10-Cycle Forced-Kill Crash-Restore Soak (`python rust/tests/crash_restore_soak.py --cycles 10`)**:
   - Cycles 1–3: `TerminateProcess` during `INTENT_JOURNALED` (immediately before/during `SetDisplayConfig` + DPI scale apply).
   - Cycles 4–7: `TerminateProcess` mid-stream under exclusive virtual display with physical monitor pre-stream DPI at 125%/150% + HDR + audio route active.
   - Cycles 8–10: `TerminateProcess` while a dummy test app is frozen via Quick Suspend (`0x554B`) + mid-restore.
   - **Pass Gate**: 10/10 cycles recover via `--recover-session` / startup recovery on `RecoveryJournalV2` (`recovery-journal.json`), restoring exact pre-test per-monitor DPI scale (0% drift across all attached displays after `dpi_settle`), exact resolution/refresh/HDR/positions, exact default audio device, and 0 permanently suspended test PIDs; `recovery-journal.json` empties cleanly.
5. **Stream Audio Latency Regression Check (`e2e.py` with `BUTTERPOLLO_TIMING_RECORD=1`, 90 s ABBA)**:
   - Compare `ui_sounds = false` vs. `ui_sounds = true` (outside stream, closed before stream audio opens): host and client audio/frame p99 delta must remain within measurement noise (`<= +0.05 ms`).
6. **NVIDIA NVENC Note**: NVENC error mappings in `rules_encoder.rs` are verified strictly against vendored NVENC SDK headers/samples (`docs/nvenc.md#checking-nvidia-code`) and explicitly recorded as untested on NVIDIA hardware.

---

## 8. Post-Discovery & Post-6.4.1 Rebase Addendum — Drivers, Migration, Cloud-Save Guard, Defender, `tools/i18n.py` & Symbols

- **Post-6.4.1 Rebase (`#129` partial, `#138`, `tools/i18n.py`, `minSdk 25`):**
  1. `6.4.1` (`5f4e666ea6`) already persisted per-monitor desktop source IDs on restore and filtered disconnected cached paths out of `wake_watch` (`#129` partial), and `6.3.2`/`6.4.1` (`b5db0e10ec`, `36188b4a2e`) already added VHF virtual gamepad devnode recovery (`#138`). Build Slice 17A's `RecoveryJournalV2` + `dpi_settle` + 10-cycle forced-kill soak cleanly on top of those fixes without duplicating them!
  2. Extract and wrap existing VHF setup logic (`rust/setup/src/install/` + `libvirtualgamepad.rs`) into `rust/setup/src/vgamepad.rs` rather than duplicating install/uninstall flows.
  3. Route `DisplayRecoveryRetry` through canonical `RecoveryJournalV2`, preserve `install_panic_hook(shared_log)`, and align Android cold-start fallback floor to `minSdk 25` (Android 7.1).
  4. **Reuse & extend `tools/i18n.py` (DO NOT duplicate `tools/check_i18n.py`!):** `origin/main` now ships `tools/i18n.py` + `tools/i18n_glossary.json`. In Slice 17G, extend `tools/i18n.py check` to cover `client/protocol/desktop-config/src/strings_70/<code>.rs` + zero-`"Experimental"` / zero-mod-name linting instead of creating a parallel `tools/check_i18n.py`.
- **Service Auto-Repair for `rubylight-vgamepad` & Stuck Updater Recovery (Slice 17B):**
  1. Extend `rust/service/src/driver.rs`, `supervisor.rs`, and `rust/setup/src/install/` `repair_drivers` to monitor and auto-repair BOTH `rubylight-vdisplay` and `rubylight-vgamepad` (independent daily rate-limits in `driver-repair.json`), and flag a post-rollback driver protocol check after `update_recovery::restore()`.
  2. Fix `rust/core/src/update_recovery.rs`: when `update-result.json` stays in `phase == "installing"` while `update.lock` is free and `"backup"` is missing (crash before backup), transition to `phase == "failed"` with an actionable error so `report_failed_install` never stays silent forever. (Note: `rust/core/src/update_recovery.rs` stays in root `rust/core/src/` — bump `rust/service/Cargo.toml` and run `python rust/release/service_inputs.py --update` when changing it!)
- **One-Click Post-Install & Side-by-Side Sunshine/Apollo/Vibepollo Migration (Slice 17B):** Expose a Web Console / Troubleshoot **Import from Sunshine / Apollo / Vibepollo** action backed by `migration::import` + `settings_backup` dry-run diff preview and `before-restore-*` snapshot, and lift `--side-by-side` and `>1 sources` blocks so evaluators can pick a source profile interactively or promote side-by-side installs cleanly.
- **Cloud-Save & OneDrive `game_settings`/`game_files` Race Guard (Slice 17A):** Synchronously flush `game_files::flush` inside `RunningApp::stop` right after game exit before releasing launcher cloud-upload focus, preserve original file mtimes when restored content is byte-identical, and detect Steam `remotecache.vdf` / OneDrive Known Folder Move (`Documents\My Games`) to warn clearly on the App page and skip unsafe overwrites while locked/syncing.
- **Pre-Publish Defender Scan, Authenticode Gate & Smart App Control Copy (Slice 17B):** Port Nightjar's `client/scripts/defender-scan.ps1` into `.github/workflows/rust-windows.yml` and `rust/release/check.ps1`, add `Get-AuthenticodeSignature` + timestamp verification on non-RC tags in `check.ps1`, and detect Windows Smart App Control / S mode refusal when staging `rubylight-vdisplay`'s catalog in `rust/setup/` with plain-English guidance.
- **Release Debug-Symbol Archival, Minidump Symbolication & 7.0.0 Release Sync (New Slice 17H — CI/Tooling Only):**
  1. Archive detached debug symbols (`objcopy --only-keep-debug` + `--add-gnu-debuglink` on Windows MinGW/Linux, `dsymutil` on macOS) as `rubylight-<ver>-windows-x64-symbols.tar.zst` & `nightjar-desktop-<ver>-<platform>-symbols.tar.zst` per release tag (keeping end-user installers stripped), and ship `tools/symbolicate.py` (`<= 250` LOC) resolving `.dmp` RVAs via `llvm-symbolizer`/`addr2line`.
  2. Ship `tools/check_release_sync.py` (`<= 250` LOC) and update `rust/release/bump.py` to lockstep-verify `Cargo.toml` == `client/app/build.gradle` `versionName` (`versionCode >= 412`) == `packaging/winget/` (updating stale manifest + adding Nightjar Windows manifest) == single pinned `RUBYLIGHT_SHA` in `client/.github/workflows/release.yml` == `service_inputs.py` == `notes.d` / Play `<= 500`-char changelog.
