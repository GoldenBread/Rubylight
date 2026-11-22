# Item 16: Wave 4 Console-Quality Final Polish Pass — Rubylight 7.0 & Nightjar 7.0

[Protocol reservations](00-protocol-reservations.md) · [Rubylight 7.0 Wave 0 redesign](01-ui-rubylight-studio.md) · [Nightjar 7.0 Wave 0 redesign](02-ui-nightjar-glass.md)

Status: engineering design (Phase 1). Written against `main` `b3d0c26b9b` (`6.2.0`) and `00-protocol-reservations.md`.

## 1. Summary & User Experience

**Wave 4 (`16-ui-final-polish`)** is the mandatory cross-cutting **Console-Quality Final Polish Pass** executed **after Wave 0's ground-up 7.0 Console-Quality Complete Redesign (`01`, `02`) and all Wave 1–3 features (`03`–`15`) land**, and **before 7.0.0 release review**. It is **not** constrained to pre-7.0 `rust/web/DESIGN.md` layouts or legacy "Moonlit Glass" idioms; instead, it unifies Wave 0's ground-up 7.0 console-quality design system and all twelve Wave 1–3 features across **every user-facing Rubylight 7.0 and Nightjar 7.0 surface**, eliminating visual drift, layout seams, orphan settings, duplicate status badges, focus-ring bugs, contrast regressions, compositor/blur instability (`#61`), scaling mismatches (`#144`/`#146`), and text clipping — and locks the result behind an upgraded **4-Surface Multimodal Screenshot Review Matrix**.

### Complete 7.0 Surface Coverage

- **Rubylight 7.0 — All 5 User-Facing Surfaces:**
  1. **Web Console** (`rust/web/`, plus script-free HTML fallback `rust/host/src/console/`) — 7.0 console-grade dark/light surfaces, unified live-session telemetry, Pairing QR view, streamlined Settings & Troubleshoot, responsive `1440 px` desktop and `390 px` mobile grids.
  2. **Browser `/play` & `/guest`** (`rust/web/src/play/Play.svelte`, `rust/web/src/guest/Guest.svelte`) — 10-ft TV/console/car browser pairing + library + full-screen play surface (`/play`) and zero-install browser guest watch/co-play surface (`/guest`), visually unified with 7.0 tokens, crisp 10-ft focus rings, gamepad/remote/keyboard navigation, and clear waiting/streaming/ended states.
  3. **Host-Drawn Couch Menu (`L3+R3`)** (`rust/core/src/couch_menu/` + `rust/host/src/couch_menu.rs`) — zero-readback opaque host-composited overlay matching 7.0 ruby focus & border tokens, crisp integer scaling across 720p/1080p/4K in sRGB/BT.2020/PQ, unified Wave 1–3 session controls, and clear permission-locked rows with zero closed-path GPU cost.
  4. **Windows Installer & Instant QR Finish Screen** (`rust/setup/`, styled via canonical `rust/setup/src/ui_theme.rs`) — Win10/11 dark/light Per-Monitor-V2 DPI-aware native chrome, live `rubylight-vgamepad` VHF + `rubylight-vdisplay` + audio/firewall/service checklist, and the **Instant QR Pairing Finish Screen** linking directly into Nightjar 7.0's First-Run / Pairing Cinema.
  5. **System Tray & Notifications** (`rust/windows/src/tray.rs`, canonical `rust/windows/src/tray/status.rs`) — crisp 16/24/32/48 px DPI semantic badges with contrast rings on light and dark Windows 10/11 taskbars, clear toast notifications, live driver/stream status, and copy parity with the 7.0 Web Console.

- **Nightjar 7.0 — All Form Factors & Redesigned Console Experiences:**
  - **Phone:** physics-driven spring/detent bottom sheets, **Modern Virtual Controller Studio (`#151`)** (`client/app/src/main/java/com/limelight/binding/input/pad/`), and **Reverse Landscape (`#152`)** + cutout/rounded-corner safe-area framing (`12-phone-safe-area`).
  - **Handheld:** compact 720p/1080p controller-first Console Home, shoulder-bumper tab switching, crisp legibility at arm's length.
  - **Android TV / Fire TV (10-ft OS):** living-room console OS (`1.5×` typographic & hit-target scale, TV overscan safety, high-visibility 3 m focus halo, `CompositorHeartbeat` TV firmware stability, instant Remote `OK`/`Hold OK` ↔ Gamepad glyph switching).
  - **Desktop (Windows / Linux / macOS):** **resolution-aware scaling (`#144`/`#146`)** keeping typography, cards, and docks proportional from 720p handhelds to 4K/HiDPI monitors, seamless keyboard/mouse/gamepad handoff, and Coherence Workstation integration.
  - **Console Control Center Floating Dock & Visual Cards:** replaces pre-7.0 text-list stream menus with the 7.0 floating dock + visual telemetry/action cards uniting DVR, Quick Suspend, HDR-to-SDR tone-map, Couch Party Audio, Stream Doctor 2.0, Input Deck, Coherence Switcher, Flip 180°, and Host Couch Menu launch.
  - **Visual Preview-Card Settings & Unified Visual Controller Editor:** Interactive visual preview cards for picture, framing, upscaling, and audio choices while locking top-level IA strictly to 4 sections (*Recommended*, *Controls*, *Audio*, *App*) + persistent *More settings* fold; **Settings → Controls**, every **Game Page**, and the in-stream **Console Control Center** open ONE flagship **Visual Console-Grade Controller Editor** (replacing dozens of scattered flat controller preference rows via lossless `Global` profile migration) featuring a live real-time controller diagram (DualSense, Xbox Series/Elite, Switch Pro / Switch 2 Pro, Steam Controller 2026, Steam Deck/Handheld, Generic + Touch `#151` tab), Sticks & Triggers Studio, Gyroscope Studio, Trackpads & Radial Menu Builder, Haptics & Lightbar Studio, and layered `Global → Per-Controller-Model → Per-Game` profiles synced via `09-host-hub-sync`.
  - **First-Run / Pairing Cinema:** cohesive animated onboarding and instant QR/PIN pairing flow built to pair seamlessly with Rubylight 7.0's Installer QR Finish Screen and Web Console pairing view (plus stock Sunshine/Apollo/Vibepollo/GameStream hosts).
  - **Compositor & Blur Stability (`#61`):** deterministic cross-window blur lifecycle on Android API 31–35+ and Desktop SDL blur target lifecycle — zero corner flash, zero compositor hang, instant fallback on power saver/thermal/tunneling, and hard zero-blur invariance while closed.

### Wave 1–3 Feature Unification Matrix

| Wave | Item | Rubylight 7.0 Surfaces (Console / `/play` & `/guest` / Couch Menu / Setup & Tray) | Nightjar 7.0 Surfaces (Phone / Handheld / TV / Desktop / Control Center / Preview-Card Settings) |
| --- | --- | --- | --- |
| 1 | `03-launch-curtain` | Console Sessions live phase badge + Settings toggle; `/play` launching curtain parity | Launching curtain phase & launcher prompt dismiss over game ambient on all 4 form factors |
| 1 | `04-frame-warp-2` | Console Stream details & performance metric row | Zero-blur Stats HUD (`glass.menu.hud`) warp confidence readout + Preview-Card Settings help |
| 1 | `05-stream-dvr` | Console Sessions DVR ring & clip manager + Storage settings; Couch Menu quick-clip row | Console Control Center DVR clip/record visual card + clip confirmation toast |
| 1 | `06-hdr-sdr-tonemap` | Console Settings → Video select (`hdr_sdr_tonemap`); Couch Menu tone-map toggle | Console Control Center & Preview-Card Settings live HDR-to-SDR tone-map preview card |
| 2 | `08-quick-suspend` | Console Sessions suspended-process state + Resume/End; Tray amber suspended badge & action | Game detail hero & Console Control Center Quick Suspend / Resume visual card |
| 2 | `09-host-hub-sync` | Console Devices sync status + cover cache panel | Multi-device synced profiles & instant covers on Console Home across all 4 form factors |
| 2 | `10-couch-party-audio` | Console Sessions guest audio mixer; `/guest` viewer permission & audio state indicator | Console Control Center per-guest voice/game spatial balance physics-sheet / card |
| 2 | `11-stream-doctor` | Console Troubleshoot Stream Doctor 2.0 findings + one-click fix; Tray attention badge | In-stream zero-blur health pill + Control Center Diagnostics visual card with safe Apply |
| 2 | `12-phone-safe-area` | *(client-only)* | Phone notch/camera-cutout & rounded-corner safe video + Reverse Landscape (`#152`) + Virtual Controller (`#151`) clearance |
| 3 | `13-client-input-deck` | Console Settings → Input folded gate (`input_deck`) + `rubylight-vgamepad` VHF status | Unified **Visual Console-Grade Controller Editor** (live diagram, Sticks & Triggers, Gyroscope, Radial Builder, Haptics/Lightbar, layered profiles, flat-key consolidation + Touch Studio `#151` tab) + zero-blur `glass.menu.hud` radial wheel |
| 3 | `14-coherence-workstation` | Console Settings → Video folded gate (`coherence_workstation`) | Desktop quick switcher strip (4K/720p scaled `#144`/`#146`) + Android Coherence physics-sheet |
| 3 | `15-away-wake-relay` | Console Settings → Network `WakeRelayPanel` | Offline PC card away-wake progress + Desktop opt-in relay switch in Preview-Card Settings |

## 2. Protocol, Native API & Capability Contract

- **Zero wire changes (`00-protocol-reservations.md` §7):** Wave 4 adds **no** control message IDs, **no** input packet magics, **no** `/serverinfo` fields, **no** RTSP `DESCRIBE` lines, and **no** Native API endpoints. `PROTOCOL_VERSION` and `rubylight_native_api::API_VERSION = 1` remain byte-for-byte unchanged.
- **Strict capability-gating audit:** Every Wave 0–3 surface across Nightjar Rust (`client/protocol/client/src/http/capabilities.rs` + `client/protocol/desktop/`) and Android Java (`HostCapabilities.java` + `client/app/src/main/java/`) is audited against two hard checks:
  1. `python tools/arch_check.py` reports **zero** `rubylight-family-gating` violations (`Family::Rubylight` / `Family.RUBYLIGHT` never gate any Wave 0–3 surface).
  2. Every native-API-backed feature (`LaunchCurtain`, `StreamDvr`, `HdrSdrToneMap`, `QuickSuspend`, `HubSync`, `PartyAudio`, `StreamDoctorV2`, `InputDeck`, `Coherence`, `AwayWakeRelay`, and `session.couch_menu`) gates strictly on `HostCapabilities::has(...)`, never `may_have(...)`, and stays cleanly hidden when connected to stock Sunshine, Apollo, Vibepollo, or GameStream hosts (verified against `client/app/src/test/resources/protocol/capabilities/` fixtures).
- **Single-source 7.0 design tokens contract:**
  - **Rubylight 7.0:** `rust/web/src/styles/tokens.css` is the ONLY place raw hex/size/shadow/radius/motion values live across Web Console, Browser `/play`, Browser `/guest`, no-script HTML console (`rust/host/src/console/style.css`), Host-Drawn Couch Menu palette constants (`rust/core/src/couch_menu/render.rs`), Windows Installer (`rust/setup/src/ui_theme.rs`), and System Tray (`rust/windows/src/tray/status.rs`). `rust/web/Tokens.test.mjs` and `PolishAudit.test.mjs` enforce `>= 4.5:1` text contrast pairs, `>= 3.0:1` focus/control pairs, and exact equality between `[data-theme='dark']` and `@media (prefers-color-scheme: dark)`.
  - **Nightjar 7.0:** `client/design/tokens.json` is the ONLY source of design numbers for Phone, Handheld, Android TV/Fire TV, Desktop, Console Control Center dock/cards, Visual Preview-Card Settings, First-Run/Pairing Cinema, Virtual Controller Studio (`#151`), and resolution-aware scaling (`#144`/`#146`). `python client/scripts/gen_design_tokens.py` emits Android XML (`client/app/src/main/res/values/design_tokens.xml`), Desktop Rust constants (`client/protocol/desktop/src/ui/design_tokens.rs`), and Nightjar site CSS (`client/site/assets/design-tokens.css`), and `python client/scripts/test_gen_design_tokens.py` + `--check` fail CI on any drift.

## 3. Architecture & <=800-LOC Module Plan

All repo paths carry `[verify post-sprint code-map.md]` for Phase 2 re-verification. Wave 4 is a **net-simplification pass**: it must never grow any grandfathered file above its `tools/size_ratchet.json` ceiling, and extracts all shared polish/audit helpers into small (`<= 800` LOC) modules with mandatory `//!` (Rust) or `/**` (Java) module headers.

### Grandfathered files in `tools/size_ratchet.json` (STRICT CEILING ENFORCEMENT)

| File | Ratchet ceiling (LOC) | Wave 4 polish discipline |
| --- | ---: | --- |
| `rust/web/src/pages/Settings.svelte` `[verify post-sprint code-map.md]` | 1208 | Deduplicate Wave 1–3 setting notices into shared `Banner`/`StatusPill` components; net LOC `<= 1208`. |
| `rust/web/src/components/devices/DeviceEditor.svelte` `[verify post-sprint code-map.md]` | 950 | Consolidate Hub Sync + Input Deck device rows via shared `DeviceFeatureSection.svelte`; net LOC `<= 950`. |
| `rust/web/src/components/settings/SettingControl.svelte` `[verify post-sprint code-map.md]` | 865 | No one-off Wave 1–3 branches; standardize via schema metadata. |
| `rust/web/src/lib/schema/basics.ts` / `video.ts` `[verify post-sprint code-map.md]` | 1530 / 1461 | Prune duplicate help prose; keep every Wave 1–3 key in `tiers.ts`. |
| `rust/windows/src/tray.rs` `[verify post-sprint code-map.md]` | 827 | Visual/status badge logic lives in canonical `rust/windows/src/tray/status.rs` (created in `01`, extended in `17`); keep `tray.rs <= 800`. |
| `rust/setup/src/install.rs` `[verify post-sprint code-map.md]` | 2226 | Installer dark/light/DPI chrome and QR finish theme live in canonical `rust/setup/src/ui_theme.rs` (single canonical setup theme module created in `01`, consumed in `17`). |
| `client/protocol/desktop/src/ui.rs` `[verify post-sprint code-map.md]` | 1586 | Keep top-level chrome dispatch lean; route 4K/720p scaling (`#144`/`#146`) and Control Center polish to leaf modules under `ui/`. |
| `client/protocol/desktop/src/ui/settings.rs` `[verify post-sprint code-map.md]` | 4087 | Audit Preview-Card Settings & Wave 1–3 rows against the 4-section + *More settings* fold; remove redundant/orphan rows so LOC drops or stays `<= 4087`. |
| `client/protocol/desktop/src/ui/dialogs.rs` `[verify post-sprint code-map.md]` | 2537 | Standardize Wave 0–3 dialog buttons (single amber primary, red destructive only for Quit/Delete/Unpair) without exceeding `2537`. |
| `client/protocol/desktop/src/ui/glass.rs` `[verify post-sprint code-map.md]` | 1289 | Enforce zero-blur `glass.menu.hud` guard, `#61` blur-lifecycle safety, and zero nested blur passes. |
| `client/protocol/desktop/src/strings/{de..zh_hant}.rs` (14 files) `[verify post-sprint code-map.md]` | 1750 each | **Hard ceiling (ZERO LINES ADDED):** All 7.0.0 Desktop strings from Waves 0–4 live exclusively in canonical chained `client/protocol/desktop/src/strings_70/<code>.rs` overflow tables (`<= 800` LOC each, defined in Item 02); grandfathered `strings/*.rs` remain untouched `<= 1750` LOC! |
| `client/app/src/main/java/com/limelight/ui/StreamMenu.java` `[verify post-sprint code-map.md]` | 1321 | Unify Console Control Center floating dock & Wave 1–3 visual cards (DVR, Quick Suspend, Party Audio, Input Deck, Coherence, Flip 180°) with consistent density; keep `<= 1321`. |
| `client/app/src/main/java/com/limelight/preferences/StreamSettings.java` `[verify post-sprint code-map.md]` | 3425 | Verify Visual Preview-Card Settings, 4-section fold integrity, and flat controller-key consolidation into the unified Visual Controller Editor (migrated & checked in `SettingsKeysTest`; net LOC drops cleanly well below `3425`). |
| `client/app/src/main/java/com/limelight/binding/input/ControllerHandler.java` & `pad/PadEditor.java` `[verify post-sprint code-map.md]` | ratchet ceiling | **ZERO growth:** All Visual Controller Editor UI stays in modular `<= 800`-LOC `binding/input/editor/**` + `PadSkin70`/`PadSnapGrid`/`PadStudioInspector` siblings; `ControllerHandler.java` and `PadEditor.java` never grow above their ceilings. |
| `client/app/src/main/java/com/limelight/console/ConsoleHomeActivity.java` / `GameDetailActivity.java` `[verify post-sprint code-map.md]` | 1440 / 1167 | Unify Quick Suspend, Hub Sync, First-Run/Pairing Cinema entry, and Input Deck actions into 7.0 Console Home hero & physics-sheet slots. |

### Planned new <=800-LOC audit & polish verification modules

| New module path | Home | Target LOC | Responsibility |
| --- | --- | ---: | --- |
| `rust/web/PolishAudit.test.mjs` `[verify post-sprint code-map.md]` | `rust/web` | ~280 | Static + SSR assertions across Web Console (`src/pages/**`, `src/components/**`), Browser `/play` (`src/play/**`), and Browser `/guest` (`src/guest/**`): zero raw hex/px/shadow outside `tokens.css`, zero `"Experimental"`, zero forbidden mod names (`Nucleus Co-op` / `ANR`) in UI copy, all 6 UI states rendered cleanly, every Wave 1–3 key registered in `tiers.ts`, and Couch Menu palette constant sync. |
| `client/scripts/ui_polish_audit.py` `[verify post-sprint code-map.md]` | `client/scripts` | ~420 | Cross-repo CI gate verifying: (1) 15-locale parity across Nightjar Android `values-*/strings.xml` and Nightjar Desktop `strings/` + `strings_70/` (asserting 0 lines added to grandfathered `strings/*.rs`), **plus all 22 host locale files** in `rust/assets/locale/*.json`; (2) zero `"Experimental"` and zero mod names (`Nucleus Co-op` / `ANR`) in user-facing UI strings across all 5 Rubylight surfaces & all Nightjar form factors; (3) zero blur token/layer leaks on always-on stream HUD views (`glass.menu.hud`) and `#61` blur-window removal assertions; (4) Android XML RTL audit (`paddingStart`/`marginStart` vs raw `Left`/`Right`, `autoMirrored="true"` on directional icons) & Virtual Controller Studio `#151` touch-target minimums; (5) consistent `"Quit game"` terminology (never `"Close game"`). |
| `client/protocol/desktop/src/ui/polish_shots_tests.rs` `[verify post-sprint code-map.md]` | `nightjar-desktop` | ~380 | Headless CPU-rasterized (`shot.rs`) visual regression suite rendering Console Home, First-Run/Pairing Cinema, Visual Preview-Card Settings, Console Control Center floating dock & visual cards, and every Wave 1–3 Desktop overlay across **resolution-aware scaling `#144`/`#146` viewports** (`1280×720` handheld, `1280×800`, `1920×1080`, `3840×2160` 4K HiDPI) × Night, Day, OLED, High-Contrast × `en`, `de`, `ja`, asserting non-zero contrast, zero clipped glyphs, and zero blur on `glass.menu.hud`. |
| `rust/web/scripts/screenshots_wave4.cjs` `[verify post-sprint code-map.md]` | `rust/web/scripts` | ~260 | Companion fixture preset manifest driving `screenshots.cjs` across all Wave 0–3 Web Console pages (Overview, Sessions live with Launch Curtain/DVR/Quick Suspend/Party Audio/Coherence, Devices + Pairing QR, Settings + Away Wake Relay, Troubleshoot Stream Doctor 2.0), Browser `/play` (pairing, 10-ft library, active stream, hold-back overlay), and Browser `/guest` (name join, waiting, streaming, ended). |
| `rust/core/src/couch_menu/polish_tests.rs` `[verify post-sprint code-map.md]` | `butterpollo-core` | ~180 | Headless pixel-contrast & layout assertions for the Host-Drawn Couch Menu (`L3+R3`): verifies 7.0 ruby accent/panel/text contrast (`>= 4.5:1` / `>= 3.0:1`), crisp integer scaling across 720p/1080p/4K, locked-row readability, Wave 1–3 row wrapping, and zero text truncation in EN/DE. |

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### Canonical 6-State UI Surface Contract

Every page, panel, dock card, physics-sheet, and dialog across **all 5 Rubylight 7.0 surfaces** and **all Nightjar 7.0 form factors** MUST implement and visually polish all six states — zero blank spinners, zero silent failures, zero orphan error strings:

```text
Loading (Skeleton / cover placeholder, zero layout jump)
  ├──(success, items > 0)──▶ Ready / Active (live state, semantic status pill word + colour)
  │      ├──(partial/fallback)──▶ Degraded / Warning (loud Banner / card alert with 1-click fix)
  │      └──(capability off)──▶ Disabled / Unavailable (flat muted well, never opacity-faded)
  ├──(success, items == 0)──▶ Empty (EmptyState icon + 1 plain sentence + next-step CTA)
  └──(HTTP/IPC/socket err)──▶ Error (plain-English error message + Retry / Diagnostics action)
```

### Stream-Time Overlay, Control Center & Compositor Stability (`#61`) State Machine (Hard Zero-Blur Invariant)

```text
StreamingClean (default while playing)
  │  INVARIANT: 0 blur passes, 0 extra Android windows, 0 drop shadows, 0 added GPU cost;
  │             Host Couch Menu executes 1 atomic load (0 extra shader dispatches)
  ├──(HUD shown: Stats, Stream Doctor pill, Input Deck radial, Coherence peek,
  │   Launch Curtain, Virtual Controller Studio #151 overlay)──▶ HudOnly
  │      INVARIANT: draws ONLY on `glass.menu.hud.*` (blur=0, shadowBlur=0) inside existing view
  │                 pass, strictly honoring Phone Safe Area & Reverse Landscape (#152) insets
  ├──(Control Center opened: Floating Dock & Visual Cards, Party Audio sheet, Coherence sheet,
  │   DVR clip card, Quick Suspend confirm, Virtual Controller Studio #151 editor)──▶ ControlCenterOpen
  │      Android API 31+ (crossWindowBlurEnabled): single blur window under dock
  │        (`MenuBlurWindow` / `StreamMenuBlurWindow`), dynamic `addCrossWindowBlurEnabledListener`,
  │        #61 compositor stability guard (zero corner-flash animation, instant stepdown on
  │        battery saver / thermal throttle / multimedia tunneling / unstable TV compositor)
  │      Android fallback / battery saver / tunneling: `glass.menu.baked.*` (blur=0, near-opaque)
  │      Desktop: SDL video-texture blur chain runs ONLY while ControlCenterOpen
  │      ──(Close / Back)──▶ Android window REMOVED (`dismiss()`, never `View.GONE`),
  │                          Desktop blur targets freed immediately ──▶ StreamingClean
  │      INVARIANT: post-close client GPU/composition cost == pre-open baseline!
  └──(Host Couch Menu L3+R3 / Ctrl+Alt+Backspace / corner-hold opened)──▶ HostCouchMenuOpen
         Host runs opaque rect-sized draw in active colour space (never reads capture frame)
         ──(Close / B / Esc / tap outside)──▶ 1-dispatch closed path restored ──▶ StreamingClean
```

### Runtime transition & resilience matrix

| Transition / Edge Case | Required verified behavior |
| --- | --- |
| **Theme switch live (System Day ↔ Night ↔ OLED ↔ High-Contrast)** | Every Wave 0–3 non-stream screen (Console Home, Visual Preview-Card Settings, First-Run/Pairing Cinema) switches tokens immediately without restart; Console Control Center dock/cards, Stats HUD, Input Deck radial, and Coherence HUD stay **night** in all themes (except High-Contrast → `solid` black `#0A0A0CFF` + `#A8ABBAFF` rim). |
| **Compositor / blur stability mid-stream (`#61`)** | On Android API 31–35+, power saver, thermal throttle, multimedia tunneling, or rapid open/close toggles cleanly step `ControlCenterOpen` between cross-window blur and `glass.menu.baked.*` with zero corner flash, zero frozen frame, and zero retained `WindowManager` layer; Android TV / Fire TV `CompositorHeartbeat` keeps direct-composition firmware alive without stealing touch/focus. |
| **Resolution & DPI scaling live change (`#144`/`#146`)** | Dragging Nightjar Desktop between a 720p/1080p 100% monitor and a 4K 200% HiDPI monitor (or resizing across `1280×720` ↔ `3840×2160`) rescales cards, Preview-Card thumbnails, Control Center dock, and typography smoothly with zero clipping, zero blurry bitmaps, and preserved focus. |
| **Phone rotation, Reverse Landscape (`#152`) & Virtual Controller Studio (`#151`)** | Rotating `ROTATION_90 ↔ ROTATION_270` (Reverse Landscape) or switching to `2.0×` font scale mirrors safe-area insets instantaneously around camera punch-holes, rounded OLED corners, and bottom gesture pills for Virtual Controller Studio `#151` controls, Launch Curtain, Control Center physics-sheets, Input Deck radial, Coherence sheet, DVR toast, and Stats HUD. |
| **Browser `/play` & `/guest` reconnect / session end** | `/play` handles pairing expiry, host disconnect, and 10-ft Hold-Back overlay cleanly with focus restored to the game tile; `/guest` transitions cleanly across `name -> joining -> waiting -> streaming -> ended` (kicked/revoked/host ended) with plain-English messages and zero layout jump at 390 px or 1080p. |
| **Reduced motion (`prefers-reduced-motion` / `Motion.reduced()`)** | Rubylight Web Console, `/play`, and `/guest` set transitions to `0 ms` and halt pulsing badges/skeletons (`Design.browser.cjs` verified); Nightjar collapses Console Home, physics-sheet, First-Run Cinema, and Control Center springs into crisp `<= 90 ms` fades or instant snaps with zero overshoot. |
| **Back-navigation focus restore** | Pressing Back/Esc from ANY nested Wave 0–3 dialog, physics-sheet, Preview-Card detail, Virtual Controller Studio `#151` inspector, or Control Center card closes only the top layer and restores the focus ring to the exact opener control. |
| **Settings migration & backup journal** | Any Wave 1–3 key merged, renamed, or retired during Wave 4 consolidation is recorded in `rust/core/src/config/retired.rs` (`RENAMED_KEYS` / `REMOVED_KEYS`), Desktop `config.rs::migrate`, and Android `SettingsKeysTest`, and verified round-trip through `settings_backup.rs` so upgrading from `6.2.0` or early `7.0.0` betas never strands settings. |

## 5. Settings & 10/10 Craft Plan

### Fewer-settings audit across Waves 0–3

Wave 4 audits every setting introduced across Waves 0–3 against `client/AGENTS.md` and the 7.0 console-quality IA:
1. **Nightjar top-level IA locked at 4 sections + Visual Preview-Card Settings:** *Recommended*, *Controls*, *Audio*, *App*, followed by the persistent *More settings* fold. Visual Preview-Card Settings upgrade how options are understood (visual framing/upscaling/controller/tone-map cards) without adding a 5th top-level section or duplicating existing keys.
2. **Every surviving setting passes the 3-question test:** Does it have a safe measured default? Can it merge into an existing control? Does its help say (1) what the player gets, (2) what it costs, and (3) when to change it — in **two short sentences** in both apps across all 15 Nightjar and 22 host locales?
3. **Default-off enforcement:** Any Wave 1–3 feature lacking measured positive hardware win on RX 7900 XT + Radeon 780M stays opt-in default-off (`coherence_workstation = false`, `away_wake_relay = false`, etc.) with honest opt-in phrasing and zero `"Experimental"` badge.

### 10/10 Console-Quality Visual & Accessibility Polish Checklist

1. **WCAG Contrast Floor (Zero Exceptions):**
   - Normal text on every surface across all 5 Rubylight surfaces + all Nightjar form factors: `>= 4.5:1` (`>= 7.0:1` in High-Contrast).
   - Focus rings, borders, switches, Control Center card rims, Preview-Card outlines, Virtual Controller Studio `#151` contours, Couch Menu borders, and icon controls: `>= 3.0:1`.
   - Verified mathematically (`Tokens.test.mjs`, `check-tokens.mjs`, `couch_menu::polish_tests`) **and** pixel-measured on rendered worst-case white covers (night) and black covers (day) via `client/design/prototype/check-contrast.py` (`fails: 0`).
2. **Single-Accent Console Hierarchy (Strictly Driven by Wave 0's Brand-New 7.0 Tokens):**
   - **Rubylight 7.0 — Obsidian Studio / Alabaster Mineral (Web Console, `/play`, `/guest`, Couch Menu, Installer, Tray):** Quiet 4-tier surfaces (`--bg`, `--panel`, `--panel-raised`, `--sunken`), one ruby primary action (`--accent` `#CE1E49` dark / `#C81942` light from `rust/web/src/styles/tokens.css`) per view, `--accent-text` for accent copy, `--brand` (`#C41242`) reserved strictly for BrandMark, dark mode driven by crisp 1 px `--line` specular borders rather than heavy murky shadows.
   - **Nightjar 7.0 — Obsidian Cinema / Alabaster Studio / OLED / High Contrast (Phone, Handheld, Android TV/Fire TV, Desktop):** Cinema Amber (`#F5B041` Night / `#8A5000` Day / `#FFC857` High-Contrast from `client/design/tokens.json`) ONLY on the single primary Play/Confirm capsule, active selection tint, and focus ring. Never two amber primary buttons on one screen; destructive red ONLY for irreversible actions (*Quit game*, *Delete clip*, *Unpair*).
3. **Duplicate-Badge, Scaling (`#144`/`#146`) & Touch/10-ft Hit-Target Sweep:**
   - Eliminate duplicate status pills (never stack >2 badges on a Console Home cover card; never show redundant "Live" + "Streaming" pills on Sessions or `/play`).
   - Enforce `>= 48 dp` touch targets on Android Phone/Handheld (`1.5×` on Android TV / Fire TV 10-ft OS) and `--control-height = 44 px` across Web Console, `/play`, `/guest`, and Installer.
   - Enforce uniform focus language: Rubylight `:focus-visible` `2 px` `--focus` outline + roving keyboard/gamepad traversal; Nightjar `3 dp` (`4 dp` TV) moon focus ring + dark gap + spring scale (`focus.scaleControl = 1.03`), cleanly scaled across 720p–4K (`#144`/`#146`).
4. **Unified Visual Controller Editor, Virtual Controller Studio (`#151`) & Phone Safe Area (`#152`) Polish:**
   - Audit every sub-studio of the **Visual Console-Grade Controller Editor** across Android Phone/Handheld/TV and Desktop: live interactive controller diagram (DualSense, Xbox Series/Elite, Switch Pro / Switch 2 Pro, Steam Controller 2026, Steam Deck/Handheld, Generic) with smooth live telemetry highlights, 100% gamepad/TV-remote/keyboard/touch/TalkBack navigation, Sticks & Triggers deadzone/curve/hair-trigger visualizers, Gyroscope Studio live orientation + 1-tap Calibrate Zero, 4–12-slice Radial Menu Builder, Haptics & Lightbar live test pulse, layered `Global → Per-Controller-Model → Per-Game` profile switcher, and zero stray legacy flat controller preference rows in Settings.
   - Verify **Modern Virtual Controller Studio (`#151`)** (seamlessly integrated as the Touch tab of the Controller Editor): freely resizable/reshapable controls across the full screen, separable ABXY & D-pad clusters, movable floating micro-toolbar, `0%`-opacity stealth mode, crisp legibility & snap-to-grid alignment guides, and zero overlap with punch-hole cameras, rounded OLED corners, or bottom gesture bars in both standard landscape (`ROTATION_90`) and Reverse Landscape (`ROTATION_270`).
5. **Dynamic Input Glyphs Everywhere:**
   - Audit every Wave 0–3 screen — Console Home, First-Run/Pairing Cinema, Visual Preview-Card Settings, Console Control Center floating dock & visual cards, Virtual Controller Studio `#151` prompts, Launch Curtain, Stream Doctor 2.0, Quick Suspend, Party Audio, Input Deck, Coherence Switcher, Away Wake Relay — against canonical `client/protocol/policy/src/input_glyphs.rs` (refactored in `02` from `desktop/src/glyphs.rs`, explicitly preserving Moonlight V+ port credits in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README per `AGENTS.md`). Hints update instantaneously from the last real input device: TV Remote (`OK`/`Hold OK`), Gamepad (Xbox `A/Y`, PlayStation `0x054C` `✕/△`, Nintendo `0x057E` with confirm/cancel swap, Steam Deck `0x28DE`), Keyboard (`Enter`/`F`), and **no hints for Touch or Mouse**.
6. **Zero `"Experimental"`, Zero Mod Names:**
   - Automated grep + AST audit (`ui_polish_audit.py`, `PolishAudit.test.mjs`) confirms zero occurrences of `"Experimental"` anywhere in Rubylight or Nightjar UI, and zero occurrences of `"Nucleus"`, `"Nucleus Co-op"`, or `"ANR"` in any user-facing string across all 5 Rubylight surfaces or Nightjar (`GPL-3.0 §7(e)` + `AGENTS.md`).
7. **22 Host + 15 Nightjar Locales, Typography & RTL-Safe Layout:**
   - **All 22 host locales** complete in `rust/assets/locale/*.json` (`bg`, `cs`, `de`, `en`, `en_GB`, `en_US`, `es`, `fr`, `hu`, `it`, `ja`, `ko`, `pl`, `pt`, `pt_BR`, `ru`, `sv`, `tr`, `uk`, `vi`, `zh`, `zh_TW`).
   - **All 15 Nightjar locales** complete in Android `values-*/strings.xml` and Desktop `strings/` + canonical chained `strings_70/<code>.rs` (`en`, `de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`; **0 lines added** to grandfathered `strings/*.rs`!).
   - Latin-1 locales (`en`, `de`, `es`, `fr`, `it`, `pt_br`) render Figtree 400/600; the 9 non-Latin-1 locales (`pl`, `tr`, `vi`, `ru`, `uk`, `ja`, `ko`, `zh_hans`, `zh_hant`) use the system typeface across the entire UI so zero words mix two fonts.
   - German & CJK stress checks: long compound German labels wrap cleanly to 2 lines without horizontal overflow (`390 px` Console / `/guest`, `393 dp` at `2.0×` font scale Android); CJK glyphs render with zero ascender/descender clipping inside fixed-height capsules and Control Center cards.
   - RTL-safe discipline: logical CSS properties (`margin-inline`, `padding-inline`, `inset-inline`) on Web Console, `/play`, and `/guest`; Android `start`/`end` attributes and `android:autoMirrored="true"` on directional icons.

## 6. Disjoint Implementer Slices (Phase 3 `parallel()`)

Strictly non-overlapping file lists for parallel Wave 4 execution (note: Installer `rust/setup/src/ui_theme.rs` + QR Finish Screen and System Tray `rust/windows/src/tray/status.rs` implementation lives in Item 17 Slice 17B to keep zero cross-item ownership collision in Wave 4, while Item 16 visually verifies all 5 Rubylight surfaces together in Section 7):

### Slice 16A: Automated UI Policy, 22-Host/15-Nightjar Locale, Token & Copy Audit Gates
- `client/scripts/ui_polish_audit.py` *(new — checks Android `values-*/strings.xml`, Desktop `strings/` + `strings_70/`, and all 22 host `rust/assets/locale/*.json`)* `[verify post-sprint code-map.md]`
- `rust/web/PolishAudit.test.mjs` *(new — covers Web Console, `/play`, `/guest`, and Couch Menu palette sync)* `[verify post-sprint code-map.md]`
- `rust/web/Tokens.test.mjs` *(add any new 7.0 surface/text/focus contrast pairs)* `[verify post-sprint code-map.md]`
- `client/app/src/test/java/com/limelight/preferences/SettingsKeysTest.java` *(verify zero orphan/reused keys)* `[verify post-sprint code-map.md]`

### Slice 16B: Rubylight 7.0 Multi-Surface Polish (Web Console, `/play`, `/guest`, Couch Menu) & Playwright Matrix
- `rust/web/src/styles/tokens.css` `[verify post-sprint code-map.md]`
- `rust/web/src/styles/forms.css` `[verify post-sprint code-map.md]`
- `rust/web/src/components/**` *(Web Console Wave 0–3 component alignment, badges, focus rings, logical RTL margins)* `[verify post-sprint code-map.md]`
- `rust/web/src/pages/**` *(Overview, Sessions, Devices + Pairing QR, Settings, Troubleshoot visual unification)* `[verify post-sprint code-map.md]`
- `rust/web/src/play/**` *(Browser `/play` 10-ft TV/console pairing, library & hold-back overlay polish)* `[verify post-sprint code-map.md]`
- `rust/web/src/guest/**` *(Browser `/guest` join, waiting, streaming & ended polish)* `[verify post-sprint code-map.md]`
- `rust/web/src/lib/schema/tiers.ts` `[verify post-sprint code-map.md]`
- `rust/web/scripts/screenshots.cjs` & `rust/web/scripts/screenshots_wave4.cjs` *(new)* `[verify post-sprint code-map.md]`
- `rust/web/Design.browser.cjs` `[verify post-sprint code-map.md]`
- `rust/host/src/console/style.css` *(no-script HTML console token & hit-target parity)* `[verify post-sprint code-map.md]`
- `rust/core/src/couch_menu/render.rs`, `rust/core/src/couch_menu/layout.rs`, and `rust/core/src/couch_menu/polish_tests.rs` *(new — Host-Drawn Couch Menu `L3+R3` palette, contrast & scaling polish)* `[verify post-sprint code-map.md]`
- `rust/assets/locale/*.json` *(all **22** host locale files — completeness & copy polish across Web Console, `/play`, `/guest`, Couch Menu)* `[verify post-sprint code-map.md]`

### Slice 16C: Nightjar 7.0 Single-Source Tokens & Prototype Contrast Matrix *(runs before 16D/16E if any token changes)*
- `client/design/tokens.json` `[verify post-sprint code-map.md]`
- `client/design/prototype/**` *(Wave 0–3 Console-Quality prototype states — Console Home, First-Run/Pairing Cinema, Visual Preview-Card Settings, Console Control Center floating dock & visual cards, Virtual Controller Studio `#151`, 4K/720p scaling `#144`/`#146` — in `shoot.mjs` + worst-pixel regions in `check-contrast.py`)* `[verify post-sprint code-map.md]`
- `client/scripts/gen_design_tokens.py` and its generated token outputs (`client/app/src/main/res/values/design_tokens.xml`, `client/protocol/desktop/src/ui/design_tokens.rs`, `client/site/assets/design-tokens.css`) `[verify post-sprint code-map.md]`

### Slice 16D: Nightjar Desktop 7.0 Polish (4K/720p `#144`/`#146`, Control Center, Visual Controller Editor, Preview-Card Settings, First-Run Cinema) & `shot.rs` Regression Suite *(parallel with 16A/16B/16E once 16C token outputs settle)*
- `client/protocol/desktop/src/ui/**` *(explicitly **excluding** generated `client/protocol/desktop/src/ui/design_tokens.rs` owned by Slice 16C; covers Console Home, First-Run/Pairing Cinema `ui/first_run.rs`, Visual Preview-Card Settings 4-section audit, **Visual Console-Grade Controller Editor** `ui/controller_editor/**`, Console Control Center floating dock & visual cards, `#61` SDL blur-target lifecycle, `#144`/`#146` resolution-aware scaling, Launch Curtain, DVR, Party Audio, Stream Doctor 2.0, Input Deck radial, Coherence, Away Wake Relay)* `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/ui/polish_shots_tests.rs` *(new)* `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/theme.rs` `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/strings_70/**`, `client/protocol/desktop/src/strings.rs`, and `client/protocol/desktop/src/strings/tests.rs` *(15-locale 7.0.0 polish exclusively in `strings_70/<code>.rs`, keeping grandfathered `strings/{de..zh_hant}.rs` untouched `<= 1,750` LOC)* `[verify post-sprint code-map.md]`

### Slice 16E: Nightjar Android Phone / Handheld / TV 7.0 Polish (Physics Sheets, Visual Controller Editor, Virtual Controller Studio `#151`, Reverse Landscape `#152`, Blur Stability `#61`) & Emulator Matrix *(parallel with 16A/16B/16D once 16C token outputs settle)*
- `client/app/src/main/java/com/limelight/ui/**` *(Console Control Center floating dock & visual cards, `StreamMenu.java`, `ActionSheet.java`, `MenuBlurWindow.java`/`StreamMenuBlurWindow.java` `#61` blur stability, `CompositorHeartbeat.java`, CoherenceSwitcherSheet, PartyAudio, StreamDoctor)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/console/**` *(ConsoleHomeActivity, GameDetailActivity, First-Run/Pairing Cinema, LaunchCurtain)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/binding/input/editor/**` *(Unified Visual Console-Grade Controller Editor: `ControllerEditorActivity`, `ControllerDiagramView`, `SticksTriggersStudioFragment`, `GyroStudioFragment`, `RadialsHapticsStudioFragment`, `TouchStudioTabBridge`, `ControllerProfileMigrator`)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/binding/input/pad/**` *(Modern Virtual Controller Studio `#151`: `PadSkin70.java`, `PadSnapGrid.java`, `PadStudioInspector.java`, `ModernController.java`, `PadEditor.java`, `PadPainter.java`, `PadGlyphs.java`, safe-area clearance with Reverse Landscape `#152`)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/binding/input/deck/**` *(blur-free `InputDeckRadialView`)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/preferences/StreamSettings.java` *(Visual Preview-Card Settings, 4-section fold integrity & flat controller-key consolidation)* `[verify post-sprint code-map.md]`
- `client/app/src/main/res/**` *(excluding generated `design_tokens.xml` owned by Slice 16C; covers 15-locale `values-*/strings.xml`, RTL `start`/`end` layouts, `settings_tiers.xml`)* `[verify post-sprint code-map.md]`
- `client/scripts/emulator-smoke.py` `[verify post-sprint code-map.md]`

## 7. Verification & Measurement Plan

### Automated policy, contrast, locale & unit checks

1. **Static & policy gates:**
   - `python client/scripts/ui_polish_audit.py` — 0 missing translations across all **15 Nightjar locales** (`values-*/strings.xml` + Desktop `strings/` & `strings_70/`, with 0 lines added to grandfathered `strings/*.rs`) and all **22 host locales** (`rust/assets/locale/*.json`), 0 `"Experimental"` UI strings, 0 mod names, 0 stream-time blur token leaks, 0 RTL `Left`/`Right` regressions.
   - `python tools/arch_check.py` — 0 new size-ratchet violations (`<= 800` LOC on all new files, grandfathered files `<= ceiling`), 0 `rubylight-family-gating` violations, 0 unannotated `cfg(unix)`.
   - `python tools/code_map.py check` & `python client/scripts/test_gen_design_tokens.py` & `python client/scripts/gen_design_tokens.py --check`.
2. **Rubylight 7.0 tests (all 5 host surfaces):**
   - `cd rust/web && npm run check && node --test "*.test.mjs" && npm run build`, plus Playwright `node --test Design.browser.cjs` (keyboard/gamepad focus, dialog focus trap/restore, reduced motion, flat disabled wells, `/play` and `/guest` builds).
   - `cargo test -p butterpollo-core --locked couch_menu` (verifies Couch Menu `L3+R3` contrast, integer scaling & layout tests).
   - `cargo test -p butterpollo-setup --locked ui` and `cargo test -p butterpollo-windows --locked tray` (verifies canonical `ui_theme.rs` installer/QR finish styling and canonical `tray/status.rs` DPI badge rendering).
3. **Nightjar 7.0 unit & Robolectric tests:**
   - `cargo test -p nightjar-desktop --locked ui::tests ui::settings_tests ui::polish_shots_tests strings`
   - `cargo test -p nightjar-core --locked --test capabilities_contract`
   - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.preferences.SettingsKeysTest --tests com.limelight.ui.StreamMenuTest --tests com.limelight.console.HomeShellTest` (plus Virtual Controller Studio `#151` layout/safe-area & `#61` blur-window removal unit tests).

### Mandatory 4-Surface Multimodal Screenshot Review Matrix (All 5 Rubylight Surfaces + All Nightjar Form Factors)

Every Wave 4 PR MUST generate and visually inspect (multimodal human/agent review) all four screenshot suites before merge:

| Surface | Command | Viewport / Theme / Surface Matrix | Pass Criteria |
| --- | --- | --- | --- |
| **1. Rubylight 7.0 — All 5 Host Surfaces (Playwright + Couch Menu + Installer/Tray)** | `PLAYWRIGHT_MODULE=... CHROMIUM_EXECUTABLE=... node rust/web/scripts/screenshots.cjs <dir> wave4` (+ `screenshots_wave4.cjs`), `cargo test -p butterpollo-core --locked couch_menu::polish_tests`, and Win32 Installer/QR Finish + Tray badge captures (`butterpollo-setup` / `tray/status.rs`) | **Web Console** (`1440×900` + `390×844` × Light/Dark across `/`, `/devices`, `/sessions` live w/ Wave 1–3, `/settings`, `/troubleshoot`, Pairing QR + no-script HTML); **Browser `/play`** (`1920×1080` 10-ft & `1280×720`: pairing, library, active stream, Hold-Back overlay); **Browser `/guest`** (`1440×900` + `390×844`: join, waiting, streaming, ended); **Host-Drawn Couch Menu (`L3+R3`)** (720p/1080p/4K integer scales, EN/DE, locked rows); **Installer + Instant QR Finish Screen & System Tray** (16/24/32/48 px light/dark taskbar badges) | Zero horizontal scroll at `390 px`, zero overlapping badges, all `>= 4.5:1` / `>= 3.0:1` pairs pass, `/play` 10-ft focus crisp from 3 m, Couch Menu ruby bars & locked rows legible at every integer scale, QR Finish Screen scannable & Tray badges crisp on light and dark taskbars. |
| **2. Nightjar 7.0 Prototype (Playwright + Worst-Pixel Contrast)** | `cd client/design/prototype && node tokens-css.mjs && node check-tokens.mjs && node shoot.mjs && python check-contrast.py` | Phone `412×915`, Handheld `1280×720`, Android TV / Fire TV `1920×1080` (10-ft OS), Desktop `1280×800`, `2560×1440`, `3840×2160` (`#144`/`#146` 4K scaling) × Night, Day, OLED, High-Contrast, `tier=baked`, `lang=de`, `input=remote\|pad\|kbd\|touch`, `stress=1` white+black covers covering Console Home, First-Run/Pairing Cinema, Visual Preview-Card Settings, Console Control Center floating dock & visual cards, Virtual Controller Studio `#151`, and Wave 1–3 overlays | `check-tokens.mjs` exit 0; `shoot.mjs` reports **zero** `overflow:` or `small:` warnings; `check-contrast.py` prints `fails: 0` (`>= 4.5:1` on worst backdrop pixel). |
| **3. Nightjar Desktop 7.0 (`shot.rs` CPU Rasterizer — `#144`/`#146` 4K–720p Scaling)** | `cargo test -p nightjar-desktop --locked polish_shots -- --nocapture` | `1280×720` (Handheld), `1280×800` (Compact), `1920×1080` (1080p), and `3840×2160` (4K HiDPI `#144`/`#146`) × Night, Day, OLED, High-Contrast × `en`, `de`, `ja` across Console Home, First-Run/Pairing Cinema, Visual Preview-Card Settings (4 sections + expanded fold), **Visual Console-Grade Controller Editor** (live diagram, Sticks & Triggers curve studio, Gyroscope Studio, Radial Menu Builder, Haptics & Lightbar), Console Control Center floating dock & visual cards, Launch Curtain, DVR, Party Audio, Stream Doctor 2.0, Input Deck radial, Coherence Switcher, Away Wake Relay | Crisp proportional density across 720p–4K (`#144`/`#146`), zero clipped German/CJK glyphs, single amber primary Play capsule, `glass.menu.hud` verified 0 blur pixels, focus ring unbroken, dynamic glyphs absent on mouse/touch. |
| **4. Nightjar Android Phone / Handheld / TV 7.0 (`emulator-smoke.py` — `#61`, `#151`, `#152`)** | `python client/scripts/emulator-smoke.py` (Android 35+ Google APIs Phone/Handheld AVDs + Android TV / Fire TV 1080p 10-ft AVD) | Phone (`393dp-font-1`, `412dp-font-1`, `393dp-font-1.3`, `393dp-font-2`, `393dp-font-2-landscape`, **Reverse Landscape `ROTATION_270` `#152`**), Handheld (`1280×720`), and **TV 1080p 10-ft OS** covering First-Run/Pairing Cinema, Console Home (physics-sheets), Visual Preview-Card Settings (4 sections + TalkBack fold), **Visual Console-Grade Controller Editor** (`ControllerEditorActivity` across Phone/Handheld/TV + **Touch Studio `#151`** tab with separable ABXY/D-pad, movable micro-toolbar & `0%` stealth mode), **Console Control Center** open/close blur stability (`#61`), Input Deck radial, Coherence sheet, Party Audio, Stream Doctor 2.0 | Zero `UiAutomator` unreachable controls, zero overlap at `2.0×` font scale, camera cutout & rounded corners clear in both `ROTATION_90` & `ROTATION_270` (`#152`), Virtual Controller Studio `#151` controls clear gesture pill, Control Center cross-window blur cleanly removed on close (`#61`). |

### Serialized idle-gated hardware verification (zero stream-time regression)

- **Idle gate:** Verify `http://127.0.0.1:47989/serverinfo` reports `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0` before running live GPU/stream checks. Use isolated test host (`butterpollo.exe --config-dir C:\tests\wave4-polish --port 48127 --bind 127.0.0.1`).
- **Exit code 3 abort rule:** Exit code `3` from `rust/release/e2e.py` or `check.ps1` signals an AMD encoder hang — **abort immediately and investigate; never retry**.
- **Stream-time composition, blur stability (`#61`) & latency gate (RX 7900 XT + Radeon 780M + real Android Phone/Handheld/TV):**
  - Open and close Console Control Center floating dock, Input Deck radial, Coherence Switcher, Party Audio sheet, and Host-Drawn Couch Menu (`L3+R3`) 10 times during a 90 s 1080p60/4K60 stream (`BUTTERPOLLO_TIMING_RECORD=1`):
    - Post-close client GPU frame time matches pre-open baseline within `±0.1 ms` (proving zero leaked blur layers, zero `#61` compositor stall, and zero retained Android blur windows).
    - Host `present_to_sent p99` delta `<= +0.05 ms` vs `6.2.0` baseline (proving zero closed-path Couch Menu cost).
  - Physical TV + handheld + phone check: visually confirm 10-ft TV focus halo from 3 m, Reverse Landscape (`#152`) + Virtual Controller Studio (`#151`) thumb ergonomics, `#144`/`#146` 4K vs 720p desktop crispness, and instant Remote vs Gamepad vs Keyboard vs Touch glyph switching.
  - **NVIDIA note:** No NVIDIA encoder changes occur in Wave 4; NVIDIA NVENC compatibility remains header/sample-verified only and never claimed as hardware-tested.
