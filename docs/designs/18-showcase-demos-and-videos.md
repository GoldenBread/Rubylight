# Item 18: Showcase Demos, Tours & Videos (Rubylight 7.0 & Nightjar 7.0 Complete Redesign)

Status: design, not implemented (Wave 5). Written against `main` `b3d0c26b9b` (`6.2.0`), `docs/designs/00-protocol-reservations.md`, and the Wave 0–4 ground-up **7.0 Console-Quality Complete Redesign**.

Item 18 establishes a 100% reproducible, strictly offline showcase pipeline for the ground-up **Rubylight 7.0** and **Nightjar 7.0** Console-Quality Complete Redesign across four deterministic surfaces: (1) Playwright Studio Console 7.0, Browser `/play` & `/guest`, and companion host-surface captures (Host-Drawn Couch Menu, Installer QR Finish Screen, System Tray) plus 60 fps interactive tours driven by deterministic offline fixtures, (2) Nightjar 7.0 Android offline `DemoLauncherActivity` stills, 60 fps recordings, and multi-cut trailer (`trailer.py`), (3) Nightjar 7.0 Desktop offline CPU-rasterized demo frames (`shot.rs` + `ui/demo_70.rs` wired through `client/protocol/desktop/src/ui.rs`), and (4) an automated provenance & honesty verifier enforcing 100% original procedural/SVG artwork via `covers.mjs`, synthetic flight footage via `GenerateAssets.java`, zero third-party game art/logos/characters, zero mod names in UI media, explicit `DEMO` captions on scripted telemetry, and real measured figures sourced solely from `rust/PERFORMANCE.md`.

---

## 1. Summary & User Experience

### 1.1 Why a Deterministic Offline Showcase Pipeline Matters

Launching the ground-up **7.0 Console-Quality Complete Redesign** requires cohesive, pixel-accurate visuals across `README.md`, `client/README.md`, the redesigned Rubylight site (`site/`), the redesigned Nightjar site (`client/site/` + `de/`), and release announcements—in dark and light themes, desktop, handheld, TV, and phone viewports, landscape (`16:9`) and portrait (`9:16`) cuts—without ever risking:
- leaked personal IP addresses, PC names, MACs, pairing secrets, Steam libraries, or OS notifications from a live host;
- third-party copyrighted game box art, logos, sprites, or characters;
- mod names (`Nucleus Co-op`, `ANR`) or `"Experimental"` labels appearing in Studio Console, Nightjar, tray, or installer UI visuals (forbidden by `AGENTS.md` / GPL-3.0 §7(e));
- unverified performance numbers or unlabelled synthetic overlay metrics.

### 1.2 Showcase Deliverables Across EVERY Redesigned 7.0 Surface

1. **Rubylight 7.0 Complete Redesign Captures & 60 fps Tour (`rust/web/scripts/` + host offline capture tests)**
   - **Web Studio Console 7.0 Stills (Desktop `1440×900` & Phone `390×844`, Light & Dark themes from Wave 0's new `rust/web/src/styles/tokens.css`)**:
     - **Console Command Stage** (`/`) — Ready, Live Stream (telemetry strip, Stream Doctor 2.0 health guidance, Frame Warp 2.0, HDR-to-SDR tone-map status), Quick Suspend active, and Multi-Client states;
     - **Display Topology Visualizer** — interactive physical + `rubylight-vdisplay` virtual display layout & per-monitor DPI map demonstrating clean virtual monitor assignment and #129 DPI crash-restore posture;
     - **Cover-Lit Per-Game Studio** — per-game studio sheet lit by the selected fictional title's procedural cover, graphics driver profile controls, per-device overrides, Input Deck profile binding, and DVR clips;
     - **QR Onboarding** — first-run & device-pairing QR onboarding sheet with RFC 5737 / scrubbed offline demo URI (zero real LAN/WAN addresses, MACs, or live PINs);
     - **`Ctrl+K` Command Palette** — quick switcher open with instant results across games, settings, devices, and actions (showing `Ctrl K` on Windows/Linux and `⌘K` on Apple platforms);
     - **DVR Clips, Devices & Plain-English Diagnostics** — instant-replay clip shelf with original procedural thumbnails, paired devices with `rubylight-vgamepad` VHF ready status, Host Hub Sync, Away Wake Relay, Settings Home + All Settings, and Item 17 plain-English Diagnostics (`/help`).
   - **Browser `/play` & `/guest` Surfaces (`rust/web/play/`, `rust/web/guest/`)**:
     - Deterministic offline Playwright captures of the redesigned TV/console browser player (`/play` PIN pairing & controller-navigable library) and browser guest watch/play room (`/guest` invite lobby & active stream frame with synthetic backdrop and `DEMO` badge) without opening any live WebRTC or network socket.
   - **Companion Native Host Surfaces (Offline Deterministic Renders — Zero Live Host Required)**:
     - **Host-Drawn Couch Menu** — deterministic CPU-rendered RGBA golden frame from `rust/core/src/couch_menu/render.rs` (Main page, Radeon settings, Stream preset, Session actions) composited over synthetic `GenerateAssets.java` flight footage with an explicit `DEMO` badge;
     - **Installer QR Finish Screen** — deterministic offline visual capture of `rust/setup/` consuming canonical `rust/setup/src/ui_theme.rs` (created in `01`, consumed in `17`), showing per-component health (`rubylight-vdisplay`, `rubylight-vgamepad` VHF, audio, firewall, service) and the redesigned **QR Finish Screen** for immediate Nightjar pairing;
     - **System Tray** — deterministic offline capture of canonical `rust/windows/src/tray/status.rs` (created in `01`, extended in `17`) DPI-scaled status badges (`16/24/32/48 px` for Idle, Streaming, Quick Suspended, Repair Needed) paired with the redesigned tray context menu (`Repair drivers & service...`, `Restore displays now`, `Open Diagnostics`).
   - **60 fps Interactive Tour**: `console-tour-7.0.mp4` (`1920×1080/60` H.264 High, `+faststart`, silent) + `console-tour.webp` (animated README teaser) recorded against a deterministic fixture server with scripted pointer/keyboard choreography sweeping Command Stage -> Display Topology Visualizer -> Cover-Lit Per-Game Studio -> QR Onboarding -> `Ctrl+K` -> DVR Clips -> Diagnostics -> `/play` & `/guest`.

2. **Nightjar 7.0 Complete Redesign Android Offline Stills, Clips & 60 fps Multi-Cut Trailer (`client/`)**
   - Extended `DemoLauncherActivity` + `Demo70States.java` showcasing EVERY redesigned Nightjar 7.0 surface across **TV (`1920×1080`)**, **Handheld (`1200×540` / `1920×1080`)**, and **Phone (`1080×2400`)** in **Night, Day, OLED Black, and High Contrast** (driven strictly by Wave 0's new `client/design/tokens.json`):
     - **Cinematic Full-Bleed Console Home** (`state=home-cinema`) — edge-to-edge hero artwork, cover-tinted ambient wash, Continue Playing & store shelves, Hub Sync instant covers, and dynamic button hints;
     - **Console Control Center Floating Dock & Visual Cards** (`state=control-center`, plus focused card states `control-center-quick`, `control-center-picture`, `control-center-audio`, `control-center-dvr`, `control-center-input`, `control-center-power`) — showcasing all 6 redesigned visual cards: **Quick Actions**, **Picture / VRR / Frame-Warp 2.0 / HDR-to-SDR**, **Audio Mixer + Party Voice**, **Instant Clip DVR**, **Controller / Radial (Input Deck)**, and **Power / Quick Suspend**;
     - **Visual Preview-Card Settings** (`state=settings-cards`) — the redesigned 4-section Recommended-first Settings (*Recommended*, *Controls*, *Audio*, *App*) + persistent *More settings* fold with live visual preview cards;
     - **First-Run & Pairing Cinema** (`state=first-run-cinema`) — Obsidian Cinema ambient LAN host discovery, optical QR scanner & 4-digit PIN cinema flow;
     - **Visual Console-Grade Controller Editor** (`state=controller-editor`, plus sub-studio states `controller-editor-sticks`, `controller-editor-gyro`, `controller-editor-radial`, `controller-editor-haptics`) — live interactive controller diagram (DualSense / Xbox Series / Switch Pro / Steam Controller 2026 / Handheld) with scripted live telemetry highlights (sticks, triggers, back paddles `L4/R4`, gyro orientation), Sticks & Triggers deadzone & response curve studio, Gyroscope Studio, 4–12-slice Radial Menu Builder, Haptics & Lightbar studio, and layered `Global → Model → Game` profile switcher (replacing scattered flat controller settings keys);
     - **Modern Virtual Controller Studio (#151)** (`state=virtual-pad-studio`) — the Touch tab/mode of the Visual Controller Editor showing freely resizable/reshapable controls across the full screen, separable ABXY & D-pad clusters, movable floating micro-toolbar, and `0%`-opacity stealth mode over synthetic flight footage (zero gamepad button-hint overlay on touch);
     - **Phone Safe Area & Reverse Landscape (#152)** (`state=reverse-landscape`) — 180°-rotated (`ROTATION_270`) telescopic USB-C grip view proving cutout, rounded-corner, and gesture-bar clearance in `Safe-Fit` vs. `Fill`;
     - **Launch Curtain** (`state=curtain`), **Stream Doctor 2.0** (`state=doctor`), and **Coherence Workstation** (`state=coherence`).
   - Updated `trailer.py` + `trailer_beats_70.py` outputs: `trailer-16x9.mp4` (`1920×1080/60`), `trailer-9x16.mp4` (`1080×1920/60` with verified safe areas `y in [290, 1440]`, right `120 px` clear), `teaser.webp` (`<= 2.5 MB`, 25 fps README loop), and `poster-16x9.webp` (`1280×720`) / `poster-9x16.webp` (`720×1280`).

3. **Nightjar 7.0 Desktop Offline CPU-Rasterized Demo Frames (`client/protocol/desktop/src/shot.rs` + `ui/demo_70.rs`)**
   - Deterministic, zero-GPU CPU-rasterized PNGs matching the live egui renderer (`Canvas::paint` + 4-tier glass chain) proving **Desktop / Handheld / TV visual parity** across Cinematic Full-Bleed Console Home (Night, Day, OLED, High Contrast), Console Control Center floating dock & all 6 visual cards, **Visual Console-Grade Controller Editor** (live controller diagram, Sticks & Triggers deadzone/curve studio, Gyroscope Studio, Radial Menu Builder, Haptics & Lightbar), Visual Preview-Card Settings, First-Run & Pairing Cinema, Launch Curtain, Stream Doctor 2.0, Input Deck Radial, Coherence Workstation multi-window switcher, DVR Clips, and Quick Suspend.

4. **Single Original 7.0 Console-Art Fictional Catalog Across Both Repos**
   - `client/scripts/demo/covers.mjs` (split with `covers_scenes.mjs`) becomes the single source of truth for the fictional 10-title catalog (`Racing Game`, `Space Sim`, `Open World`, `Night Express`, `Cobalt Drift`, `Harbor Light`, `Signal Zero`, `Cloud Forge`, `Tidebreaker`, `Paper Orbit` — upgraded to match the **7.0 Console-Quality Art Direction**: rich layered atmospheric lighting, widescreen-safe hero framing + crisp 2:3 poster focal crops, Figtree typography, 100% procedural SVG with zero external images, zero AI art, and zero third-party logos), emitting Android `app/src/debug/assets/demo/art/N.jpg`, shared `demo-catalog.json`, Desktop `shot.rs` assets, and Console 7.0 `rust/web/scripts/demo/art/*.webp`.
   - `client/scripts/demo/GenerateAssets.java` remains the sole generator of 100% synthetic `1920×1080/60` flight footage (`sample_*.mp4`) carrying explicit HUD reference bars and the `DEMO` watermark.

---

## 2. Protocol, Native API & Capability Contract

Per `docs/designs/00-protocol-reservations.md` §7, **Item 18 requires ZERO protocol, control-message, `/serverinfo`, or Native API wire changes.** `PROTOCOL_VERSION` and `rubylight_native_api::API_VERSION = 1` remain byte-for-byte unchanged.

### 2.1 Fixture Schema & Surface Fidelity Contract

Even though showcase mode is 100% offline, its mock JSON payloads and render hooks MUST validate directly against the real 7.0.0 production types so screenshots and trailers can never drift from real shipping UI:

| Offline Fixture Surface | Validated Against Real Type / Route / Renderer | Contract Enforcement |
| --- | --- | --- |
| Console 7.0 + `/play` + `/guest` Playwright mocks | `rust/web/src/lib/api.ts` TypeScript interfaces + Svelte SSR + `vite.play.config.ts` / `vite.guest.config.ts` bundles | `rust/web/DemoFixtures.test.mjs` validates every fixture response shape (including Display Topology, Per-Game Studio, QR Onboarding, `Ctrl+K`, `/play`, `/guest`, DVR, Quick Suspend, Stream Doctor 2.0) and asserts zero unmocked network calls. |
| Host-Drawn Couch Menu, Installer QR Finish & System Tray | `rust/core/src/couch_menu/render.rs`, canonical `rust/setup/src/ui_theme.rs` (`-p butterpollo-setup`), canonical `rust/windows/src/tray/status.rs` | Deterministic unit/golden render tests (`cargo test -p butterpollo-core`, `-p butterpollo-setup`, `-p butterpollo-windows`) emit verified offline PNGs directly from real production render code. |
| Nightjar Android `DemoFixtures` / `Demo70States` | `HostCapabilities.Feature` (Wave 1–3 features from `00-protocol-reservations.md` §6) | `DemoFixturesCapabilitiesTest.java` asserts every 7.0.0 demo state enables features strictly via `HostCapabilities` (never `Family.RUBYLIGHT` brand checks). |
| Nightjar Desktop `ui/demo_70.rs` | `nightjar_core::http::capabilities::HostCapabilities` & `rubylight_native_api` v1 structs | `cargo test -p nightjar-desktop --locked demo_70` compiles against exact production Rust structs; any API break fails compilation immediately. |
| Benchmark Callout Cards (`trailer.py`, `render-console-tour.py`) | `rust/PERFORMANCE.md` dated run anchors | `tools/check_demo_provenance.py` parses every quoted benchmark number in trailer/tour manifests and asserts exact verbatim match against its `rust/PERFORMANCE.md` anchor text. |

### 2.2 Strict Separation of Debug Demo Code from Release Binaries

- **Android**: All `Demo*` activities, fixtures, procedural covers (`app/src/debug/assets/demo/art/`), and synthetic flight videos (`app/src/debug/assets/demo/video/`) remain strictly under `client/app/src/debug/`. Release APKs (`nonRootRelease`, `rootRelease`) compile and package zero demo bytes (`applicationId` stays `com.butterpollo.client`).
- **Desktop**: `shot.rs` and `ui/demo_70.rs` remain `#[cfg(any(feature = "demo-library", test))]`, wired strictly through `client/protocol/desktop/src/ui.rs` right beside existing `mod demo;` (NEVER via grandfathered `main.rs` or non-existent `lib.rs`), so release binaries carry zero demo payload.
- **Host Console**: `rust/web/scripts/` and `docs/media/` capture scripts are never copied into `rust/web/dist/` or embedded in `butterpollo.exe`.

---

## 3. Architecture & <=800-LOC Module Plan

All paths below must be re-verified against `docs/code-map.md` and `client/docs/code-map.md` in Phase 2 (`[verify post-sprint code-map.md]`). Every new `.rs` module carries a `//!` header, every new `.java` class carries a `/**` header, and every new source file stays strictly `<= 800` LOC.

### 3.1 Grandfathered Files in `tools/size_ratchet.json` (DO NOT GROW)

| Grandfathered File | Ratchet Ceiling | Strategy in Item 18 |
| --- | ---: | --- |
| `client/protocol/desktop/src/ui.rs` `[verify post-sprint code-map.md]` | 1586 | Wire `ui/demo_70.rs` with a 2-line `#[cfg(any(feature = "demo-library", test))] pub(crate) mod demo_70;` declaration beside existing `mod demo;` (NEVER via grandfathered `main.rs` or non-existent `lib.rs`); keep net LOC `<= 1586`. |
| `client/protocol/desktop/src/ui/demo.rs` `[verify post-sprint code-map.md]` | 859 | Keep existing 6.x fixture untouched; place all 7.0.0 desktop demo states in new `client/protocol/desktop/src/ui/demo_70.rs`. |
| `client/protocol/desktop/src/ui/tests.rs` `[verify post-sprint code-map.md]` | 9298 | Put all new 7.0.0 CPU screenshot tests in `client/protocol/desktop/src/shot_70_tests.rs`. |
| `client/app/src/main/java/com/limelight/ui/StreamMenu.java` `[verify post-sprint code-map.md]` | 1321 | Zero edits from Item 18; `Demo70States` drives the real redesigned Console Control Center & Stream Menu via `DemoFixtures`. |
| `rust/web/src/lib/api.ts` `[verify post-sprint code-map.md]` | 1653 | Zero edits from Item 18; fixtures import existing types read-only. |
| `client/protocol/desktop/src/strings/*.rs` `[verify post-sprint code-map.md]` | 1750 each | Zero edits from Item 18; desktop demo strings use existing keys or `strings_70/<code>.rs`. |

*(Note: `.cjs`, `.mjs`, `.ps1`, and `.py` scripts are outside `arch_check.py`'s `.rs/.java/.kt/.ts/.svelte` scope, yet every new script stays modular and `<= 500` LOC for maintainability.)*

### 3.2 New & Updated Module Plan

| Module / Script Path | Target LOC | Responsibility |
| --- | ---: | --- |
| `client/scripts/demo/covers.mjs` `[verify post-sprint code-map.md]` | ~420 *(split scenes into `covers_scenes.mjs`)* | Upgraded to 7.0 Console-Quality art direction: generates 100% original procedural SVG artwork (2:3 posters + 16:9 hero-crop-safe compositions, Figtree typography, zero external images/logos) for Android (`app/src/debug/assets/demo/art/N.jpg`), Desktop (`shot.rs`), and Console 7.0 (`rust/web/scripts/demo/art/*.webp`), plus canonical `demo-catalog.json` and `out/demo/assets/covers-contact.png`. |
| `client/scripts/demo/covers_scenes.mjs` *(new)* | ~480 | Pure procedural SVG scene definitions for the 10 fictional titles, calibrated for both Nightjar ambient extraction and Rubylight Cover-Lit Per-Game Studio lighting. |
| `client/scripts/demo/GenerateAssets.java` `[verify post-sprint code-map.md]` | ~340 | 100% synthetic Java2D `1920×1080/60` flight footage generator (`sample_*.mp4`) with crisp reference geometry for Frame Warp 2.0, VRR, Stream Doctor 2.0, Modern Virtual Controller Studio (#151), Phone Safe Area (#152), and Couch Menu compositing. |
| `rust/web/scripts/demo/fixtures_70.cjs` *(new)* | ~380 | Deterministic Rubylight 7.0 fixtures for every web surface: Console Command Stage, Display Topology Visualizer (physical + `rubylight-vdisplay` + DPI map), Cover-Lit Per-Game Studio, QR Onboarding (scrubbed offline RFC 5737 URI), `Ctrl+K` palette, Browser `/play` & `/guest`, DVR Clips, Devices (`rubylight-vgamepad` VHF ready, Hub Sync, Away Wake Relay), Settings, and Plain-English Diagnostics. 100% original catalog from `demo-catalog.json`; **zero mod names, zero `"Experimental"`**. |
| `rust/web/scripts/demo/art/` *(generated)* | assets | Procedural SVG/WebP covers exported by `covers.mjs --web` (0 third-party art). |
| `rust/web/scripts/screenshots.cjs` `[verify post-sprint code-map.md]` | ~280 | Refactored to consume `demo/fixtures_70.cjs`, add `SHOT_MODE=showcase` (enforces fictional catalog + `Sample data` ribbon), and capture all redesigned Rubylight 7.0 routes including Command Stage (`/`), Display Topology Visualizer, Cover-Lit Per-Game Studio, QR Onboarding, `Ctrl+K`, Browser `/play`, Browser `/guest`, DVR, Devices, Settings, and Diagnostics. |
| `rust/web/scripts/console-tour.cjs` *(new)* | ~300 | Playwright deterministic 60 fps interactive tour recorder sweeping every redesigned Rubylight 7.0 web surface (Command Stage -> Display Topology Visualizer -> Cover-Lit Per-Game Studio -> QR Onboarding -> `Ctrl+K` -> DVR -> Diagnostics -> `/play` & `/guest`), writing deterministic frames + timing manifest. |
| `rust/web/scripts/render-console-tour.py` *(new)* | ~280 | Pipes captured Console 7.0 + Couch Menu + Installer QR Finish + Tray frames with Studio 7.0 typography and explicit `Sample data · Offline demo` captions to FFmpeg (`libx264` `1920×1080/60` `+faststart` silent + `libwebp` animated README teaser) with `mpdecimate` smoothness report. |
| `rust/web/DemoFixtures.test.mjs` *(new)* | ~180 | Node test verifying `fixtures_70.cjs` JSON validity against `api.ts` contracts, zero commercial titles, zero mod names, zero `"Experimental"`, zero real IP/MAC/PIN leaks, and SSR stability across main console, `/play`, and `/guest`. |
| `rust/web/scripts/capture_host_surfaces.py` *(new)* | ~190 | Orchestrates deterministic offline golden PNG captures for the 3 non-browser host surfaces: Host-Drawn Couch Menu (`rust/core/src/couch_menu/render.rs` composited over `GenerateAssets.java` frame with `DEMO` pill), Installer QR Finish Screen (via `-p butterpollo-setup` + canonical `ui_theme.rs`), and System Tray (`rust/windows/src/tray/status.rs` DPI badges + menu). |
| `client/app/src/debug/java/com/limelight/demo/Demo70States.java` *(new)* | ~450 | Debug-only fixture wiring for ALL redesigned Nightjar 7.0 surfaces: `home-cinema`, `control-center` (all 6 visual cards: Quick Actions, Picture/VRR/Frame-Warp, Audio Mixer + Party Voice, Instant Clip DVR, Controller/Radial, Power/Suspend), `controller-editor` (and sub-studio states: live DualSense/Xbox/Switch/Steam/Handheld diagram telemetry, Sticks & Triggers curves, Gyroscope Studio, Radial Menu Builder, Haptics & Lightbar), `settings-cards`, `first-run-cinema`, `virtual-pad-studio` (#151 Touch tab with separable ABXY/D-pad, floating micro-toolbar & 0% stealth), `reverse-landscape` (#152), `curtain`, `doctor`, and `coherence`. Applies explicit `DEMO` overlay watermark on every scripted stream view. |
| `client/app/src/debug/java/com/limelight/demo/DemoLauncherActivity.java` `[verify post-sprint code-map.md]` | ~220 | Routes new 7.0.0 `--es state ...` extras to `Demo70States.java` while preserving all existing 6.x states. |
| `client/scripts/demo/capture-stills.ps1` `[verify post-sprint code-map.md]` | ~145 | Captures all 7.0 portrait, handheld, TV, and reverse-landscape stills (`home-cinema`, `control-center-*`, `controller-editor`, `settings-cards`, `first-run-cinema`, `virtual-pad-studio`, `reverse-landscape`, `launch-curtain`, `stream-doctor`, `coherence`) with PNG dimension & safe-area checks. |
| `client/scripts/demo/capture-video.ps1` `[verify post-sprint code-map.md]` | ~325 | Adds 7.0 60 fps recorded beats (`17-home-cinema`, `18-first-run-pairing`, `19-curtain`, `20-control-center-dock`, `21-controller-editor`, `22-settings-cards`, `23-virtual-pad-studio`, `24-reverse-landscape`, `25-doctor-and-suspend`) locked to the 60 fps Choreographer/screenrecord grid. |
| `client/scripts/demo/trailer_beats_70.py` *(new)* | ~380 | Modular 7.0.0 storyboard beats imported by `trailer.py`: Cinematic Full-Bleed Console Home, First-Run & Pairing Cinema, Launch Curtain, Console Control Center Floating Dock & 6 Visual Cards, **Visual Console-Grade Controller Editor** (live diagram, sticks/curves, gyro, radial builder), Visual Preview-Card Settings, Modern Virtual Controller Studio (#151), Phone Safe Area / Reverse Landscape (#152), Desktop/Handheld/TV Parity, Honest `PERFORMANCE.md` Beat, Platforms & Hosts. |
| `client/scripts/demo/trailer.py` `[verify post-sprint code-map.md]` | ~430 | Updated orchestrator loading Wave 0 token palette from `client/design/tokens.json` (no hardcoded legacy 6.x colors), integrating `trailer_beats_70.py`, enforcing 9:16 safe areas (`y in [290, 1440]`, right `120 px` clear), `DEMO` captions, and emitting `trailer-16x9.mp4`, `trailer-9x16.mp4`, `teaser.webp`, and posters. |
| `client/protocol/desktop/src/ui/demo_70.rs` *(new)* | ~410 | Deterministic 7.0.0 desktop demo states matching Android/TV/Handheld parity: Full-Bleed Console Home (Night/Day/OLED/High Contrast), Console Control Center Floating Dock & all 6 visual cards, **Visual Console-Grade Controller Editor** (live diagram, Sticks & Triggers, Gyroscope Studio, Radial Menu Builder, Haptics & Lightbar), Visual Preview-Card Settings, First-Run & Pairing Cinema, Launch Curtain, Stream Doctor 2.0, Input Deck Radial, Coherence Workstation, DVR Clips, and Quick Suspend. |
| `client/protocol/desktop/src/shot_70_tests.rs` *(new)* | ~260 | CPU-rasterized screenshot suite invoking `shot::Canvas` to write verified 7.0.0 desktop PNGs across themes, input sources, and locales without needing a display or GPU. |
| `tools/check_demo_provenance.py` *(new)* | ~310 | Automated provenance & honesty gate verifying `covers.mjs` 1:1 procedural mapping, zero third-party image inputs, banned commercial title & mod-name list (`Nucleus Co-op`, `ANR`), zero `"Experimental"`, `DEMO` badge presence on every synthetic stream beat, and verbatim `rust/PERFORMANCE.md` citation match. |

### 3.3 Offline Provenance Pipeline Flow

```text
  covers.mjs + covers_scenes.mjs (100% procedural SVG + Figtree, 7.0 Console Art Direction, 0 external images)
     +--> client/app/src/debug/assets/demo/art/N.jpg  --> Android Demo70States -> capture-stills/video -> trailer.py
     +--> client/scripts/demo/demo-catalog.json       --> Desktop ui/demo_70.rs (via ui.rs) -> shot.rs (CPU PNGs)
     +--> rust/web/scripts/demo/art/*.webp            --> Playwright screenshots.cjs + console-tour.cjs (Console, /play, /guest)

  GenerateAssets.java (100% synthetic Java2D flight loop, 1920x1080/60)
     +--> client/app/src/debug/assets/demo/video/sample_*.mp4 --> Nightjar stream surfaces (#151, #152, Control Center)
     +--> capture_host_surfaces.py                            --> Host-Drawn Couch Menu + Installer QR Finish + Tray PNGs

  rust/PERFORMANCE.md (ONLY source of real benchmark numbers — RX 7900 XT / Radeon 780M)
     +--> tools/check_demo_provenance.py --> verifies trailer.py & render-console-tour.py stat cards match verbatim
```

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

Because showcase capture scripts temporarily mutate emulator/device state (display size, density, SystemUI demo mode, orientation including `ROTATION_270` Reverse Landscape, locale, `DemoHomeActivity` launcher component, theme), clean restoration on interruption or failure is mandatory.

### 4.1 Capture Runner State Machine

```text
  [IDLE]
     |
     v  Initialize-Capture: snapshot device wm size, wm density, night_mode,
        accelerometer_rotation, user_rotation, locale, home component -> capture-restore.json
  [DEVICE_CONFIGURED]
     |
     +--> [REHEARSE_SHOT]: launch DemoLauncherActivity state once (warm classes/shaders, dump UIAutomator XML)
     |          |
     |          v
     +--> [RECORD_TAKE] (up to 3 takes; select lowest dropped-frame count on 60 fps grid)
     |          |
     |          +-- UI element missing? --> save out/demo/capture/last-ui.xml --> [ABORT_CLEANUP]
     |          +-- FFmpeg / adb error? --> [ABORT_CLEANUP]
     |          v
     +--> [VALIDATE_OUTPUT]: check PNG exact WxH, ffprobe 60 fps + faststart + 0 audio, mpdecimate smoothness
                |
                v
  [RESTORE_DEVICE] (PowerShell `finally` block + crash-recovery re-entry from capture-restore.json)
     |  - force-stop com.butterpollo.client.debug (NEVER clears user app data)
     |  - disable DemoHomeActivity, restore original launcher
     |  - reset wm size/density, rotation, SystemUI demo exit, locale, theme
     |  - remove capture-restore.json
     v
  [CLEAN]
```

### 4.2 Failure, Re-Run & Interruption Matrix

| Failure / Interruption Mode | Handling | Guarantee |
| --- | --- | --- |
| PowerShell killed (`Ctrl+C`) mid-capture on Android emulator | `out/demo/capture/capture-restore.json` retains original device settings; next run (or `capture-stills.ps1 -RestoreOnly`) restores everything cleanly before starting. | Emulator is never left stuck in `DemoHomeActivity`, `ROTATION_270`, `1080×2400`, or non-English locale. |
| Software emulator (`-gpu swiftshader`) drops frames below 60 fps | `smoothness-*.txt` (`mpdecimate`) detects `< 50` distinct fps during motion beats and fails the shot; supports `-HostCapture` (FFmpeg `ddagrab` + `h264_amf` on idle host) per `client/docs/demo/README.md`. | Stuttering 30 fps footage never silently ships in the 7.0 trailer. |
| Missing cover scene in `covers_scenes.mjs` or extra unmapped game in `DemoConsole.LIBRARY` | `covers.mjs` exits non-zero immediately; never reuses one cover across two games. | 1:1 original cover integrity enforced at generation time. |
| Playwright route (Console, `/play`, or `/guest`) attempts any live host or external network call | `screenshots.cjs` and `console-tour.cjs` install a catch-all `page.route('**/*', ...)` that **aborts and fails the run** on any unmocked URL or live WebSocket (`wss://`). | 100% offline determinism; zero contact with an installed host or internet resource. |
| Commercial game title, mod name (`Nucleus Co-op` / `ANR`), or `"Experimental"` enters fixtures/scripts | `tools/check_demo_provenance.py` and `DemoFixtures.test.mjs` fail CI immediately with exact file + line number. | Zero copyright / trademark / GPL-3.0 §7(e) violations in published showcase media. |
| `shot.rs` desktop frame drifts in dimensions or clips text | `shot_70_tests.rs` asserts exact canvas dimensions, WCAG contrast regions, and non-empty rendered bounds; writes review PNGs to `out/demo/desktop/`. | Zero blank, clipped, or low-contrast desktop frames. |

---

## 5. Settings & 10/10 Craft Plan

- **Fewer-Settings Rule**: Zero new user-facing settings in Rubylight or Nightjar. All demo toggles live strictly in debug-only `DemoLauncherActivity` intent extras (`--es state ...`, `--es ui ...`, `--es look ...`, `--es glass ...`, `--ez reduced ...`) and script CLI/env vars (`SHOT_MODE`, `SHOT_PAGES`, `-Theme`, `-Locale`, `-Only`).
- **Default-Off Gate**: Showcase fixtures depict default-on 7.0.0 capabilities accurately; opt-in features (such as `coherence_workstation`, `away_wake_relay`, `ui_sounds`) appear clearly as opt-in cards/controls, never misrepresented as default-on.
- **Plain-English Error States**: Console 7.0 Diagnostics (`/help`) and Nightjar Stream Doctor 2.0 / `ErrorSheet` showcase shots depict Item 17's plain-English three-part copy (*What happened*, *What to do next*, collapsed *Technical details*) with zero raw `HRESULT` or stack traces in primary cards.
- **Dynamic Input Glyphs (Zero Hints for Touch/Mouse)**:
  - Driven by canonical `client/protocol/policy/src/input_glyphs.rs` (created in `02`, audited in `17`).
  - Showcase captures cover Handheld (Xbox `A`/`Y`), TV Remote (`OK`/`Hold OK`), PlayStation DualSense (`✕`/`△`), Nintendo (`B`/`X` when swapped), Steam Deck, and Keyboard (`Enter`/`F`).
  - Phone Touch captures — including **Modern Virtual Controller Studio (#151)**, **Phone Safe Area / Reverse Landscape (#152)**, and Phone Console Home — explicitly assert **zero key-cap button hints** on screen.
- **Zero Stream-Time Blur When Closed**:
  - Active stream shots (including Launch Curtain lift, Stats HUD, Modern Virtual Controller Studio #151, Phone Safe Area #152, Input Deck radial, and Stream Doctor 2.0 pill) verify zero live backdrop blur over active game video (`glass.menu.hud.*` only).
  - Console Control Center floating dock shots verify that blur activates ONLY while the dock is open (or falls back cleanly to `glass.menu.baked.*`) and releases immediately on close.
- **WCAG Contrast**: Every rendered showcase frame inherits Wave 0's `>= 4.5:1` text and `>= 3.0:1` focus/control contrast tokens (and `>= 7:1` in High Contrast captures), verified on worst-case light/dark procedural covers.
- **Zero `"Experimental"` & Zero Mod Names**:
  - Showcase fixtures forbid `"Experimental"` badges and forbid naming any mod (`Nucleus Co-op`, `ANR`) anywhere in Studio Console, `/play`, `/guest`, Couch Menu, Installer, Tray, or Nightjar UI captures.
  - Legacy 6.x commercial titles (`Portal 2`, `Forza Horizon 5`, `Fortnite`, `Diablo IV`, etc.) and the `Nucleus Co-op` string in `/api/coop` inside `rust/web/scripts/screenshots.cjs` are completely replaced in `SHOT_MODE=showcase` by the 100% original fictional catalog from `covers.mjs` and host-neutral co-op copy.
- **22 Host Locales & 15 Nightjar Locales**:
  - `screenshots.cjs` spot-checks representative locales from the **22** host locale files in `rust/assets/locale/*.json` (`en`, `de`, `ja`, `ru`, `zh`), and `capture-stills.ps1 -Locale <code>` + `shot_70_tests.rs` spot-check the **15** Nightjar locales (`de` for `client/site/de/`, plus `ja`, `ru`, `zh_hans` verifying Figtree Latin-1 vs. OS system-font switching and zero text clipping across `strings_70/<code>.rs`).
- **Strict Honesty & Provenance Captions**:
  - Every scripted stream overlay frame displays a visible `DEMO` pill (`119.9 FPS`, `6 ms decode`, `0.1% loss`, synthetic GPU readings).
  - Every published caption beside demo media explicitly states that telemetry numbers are scripted offline illustrations, and every real benchmark number quoted in trailers/tours cites its exact dated run in `rust/PERFORMANCE.md` (RX 7900 XT / Radeon 780M only; never claiming NVIDIA hardware runs).

---

## 6. Disjoint Implementer Slices (Phase 3 Staged `parallel()`)

Executed via `orchestrator_core.py`'s staged slice runner with strictly non-overlapping file ownership:
- **Stage 1**: Slice `18A` runs first to produce the upgraded 7.0 Console-Art procedural covers, synthetic flight videos, and canonical `demo-catalog.json`.
- **Stage 2**: Slices `18B`, `18C`, and `18D` run in parallel once Stage 1 completes, consuming `demo-catalog.json` and `sample_*.mp4`.
- **Stage 3**: Slice `18E` runs after Stage 2 to verify end-to-end provenance, banned-term absence, `DEMO` badges, and `rust/PERFORMANCE.md` citation integrity.

### Stage 1 — Slice 18A: Unified 7.0 Console-Art Procedural Covers & Synthetic Footage (`covers.mjs` + `GenerateAssets.java`)
- **Files (exclusive)**:
  - `client/scripts/demo/covers.mjs` `[verify post-sprint code-map.md]`
  - `client/scripts/demo/covers_scenes.mjs` *(new)*
  - `client/scripts/demo/demo-catalog.json` *(generated canonical schema consumed by Stage 2)*
  - `client/scripts/demo/GenerateAssets.java` `[verify post-sprint code-map.md]`
  - `client/app/src/debug/assets/demo/art/*.jpg` *(regenerated)*

### Stage 2 — Slice 18B: Rubylight 7.0 All-Surface Captures & 60 fps Interactive Tour *(parallel with 18C, 18D)*
*(Consumes `demo-catalog.json` from Stage 1; covers Command Stage, Display Topology Visualizer, Cover-Lit Per-Game Studio, QR Onboarding, `Ctrl+K`, Browser `/play` & `/guest`, Host-Drawn Couch Menu, Installer QR Finish Screen, and System Tray)*
- **Files (exclusive)**:
  - `rust/web/scripts/demo/fixtures_70.cjs` *(new)*
  - `rust/web/scripts/demo/art/*.webp` *(generated by `covers.mjs --web`)*
  - `rust/web/scripts/screenshots.cjs` `[verify post-sprint code-map.md]`
  - `rust/web/scripts/console-tour.cjs` *(new)*
  - `rust/web/scripts/render-console-tour.py` *(new)*
  - `rust/web/scripts/capture_host_surfaces.py` *(new)*
  - `rust/web/DemoFixtures.test.mjs` *(new)*
  - `docs/media/build_readme_media.py` `[verify post-sprint code-map.md]`

### Stage 2 — Slice 18C: Nightjar 7.0 Android Debug Demo States & 60 fps Capture/Trailer Pipeline *(parallel with 18B, 18D)*
*(Covers Cinematic Full-Bleed Home, Console Control Center floating dock & 6 visual cards, Visual Preview-Card Settings, First-Run & Pairing Cinema, Modern Virtual Controller Studio #151, Phone Safe Area / Reverse Landscape #152, Launch Curtain, Stream Doctor 2.0, Coherence)*
- **Files (exclusive)**:
  - `client/app/src/debug/java/com/limelight/demo/Demo70States.java` *(new)*
  - `client/app/src/debug/java/com/limelight/demo/DemoLauncherActivity.java` `[verify post-sprint code-map.md]`
  - `client/app/src/testDebug/java/com/limelight/demo/DemoFixturesCapabilitiesTest.java` *(new)*
  - `client/scripts/demo/capture-stills.ps1` `[verify post-sprint code-map.md]`
  - `client/scripts/demo/capture-video.ps1` `[verify post-sprint code-map.md]`
  - `client/scripts/demo/trailer_beats_70.py` *(new)*
  - `client/scripts/demo/trailer.py` `[verify post-sprint code-map.md]`
  - `client/docs/demo/shotlist.md` `[verify post-sprint code-map.md]`
  - `client/docs/demo/README.md` `[verify post-sprint code-map.md]`

### Stage 2 — Slice 18D: Nightjar 7.0 Desktop Offline CPU Demo Frames (`shot.rs` + `ui/demo_70.rs`) *(parallel with 18B, 18C)*
- **Files (exclusive)**:
  - `client/protocol/desktop/src/ui/demo_70.rs` *(new)*
  - `client/protocol/desktop/src/shot_70_tests.rs` *(new)*
  - `client/protocol/desktop/src/ui.rs` *(wire `#[cfg(any(feature = "demo-library", test))] pub(crate) mod demo_70;` right beside existing `mod demo;` — NEVER wire through grandfathered `main.rs` or non-existent `client/protocol/desktop/src/lib.rs`, keeping `ui.rs <= 1586` LOC and `ui/demo.rs` untouched)* `[verify post-sprint code-map.md]`

### Stage 3 — Slice 18E: Provenance & Honesty Automated Gate *(runs after Stage 2)*
- **Files (exclusive)**:
  - `tools/check_demo_provenance.py` *(new)*
  - `tools/tests/test_check_demo_provenance.py` *(new)*

---

## 7. Verification & Measurement Plan

### 7.1 Narrowest Automated Unit & Provenance Tests (No GPU / No Emulator Required)

1. **Provenance & Honesty Gate**:
   - `python tools/check_demo_provenance.py` and `python -m unittest tools/tests/test_check_demo_provenance.py`
   - Asserts: (a) 1:1 scene-to-file mapping in `covers.mjs` / `covers_scenes.mjs` with zero external image URLs/files; (b) zero commercial game titles, zero `"Experimental"`, and zero mod names (`Nucleus Co-op`, `ANR`) in `fixtures_70.cjs`, `Demo70States.java`, `demo_70.rs`, and `trailer_beats_70.py`; (c) explicit `DEMO` badge present on every synthetic stream beat (including Couch Menu, Virtual Controller Studio #151, Reverse Landscape #152, and Console Control Center); (d) every benchmark figure in `trailer.py` and `render-console-tour.py` matches its cited `rust/PERFORMANCE.md` anchor verbatim.
2. **Rubylight 7.0 Web & Companion Surface Checks**:
   - `cd rust/web && node --test DemoFixtures.test.mjs` — verifies offline fixture JSON integrity, zero unmocked routes, zero PII/IP leaks, and SSR stability across Studio Console, `/play`, and `/guest`.
   - `PLAYWRIGHT_MODULE=... SHOT_MODE=showcase node rust/web/scripts/screenshots.cjs out/console-shots v7` — verifies Command Stage, Display Topology Visualizer, Cover-Lit Per-Game Studio, QR Onboarding, `Ctrl+K`, `/play`, `/guest`, DVR, Devices, Settings, and Diagnostics across Desktop (`1440×900`) & Phone (`390×844`) × Light & Dark with zero console errors, zero horizontal overflow, and zero network leaks.
   - Offline host-surface render verification via `cargo test -p butterpollo-core --locked couch_menu`, `cargo test -p butterpollo-setup --locked ui_theme`, and `cargo test -p butterpollo-windows --locked tray` (zero live service or installer mutation).
3. **Nightjar Android Debug Demo Unit & Lint Tests**:
   - `.\gradlew.bat :app:testNonRootDebugUnitTest --tests "com.limelight.demo.DemoFixturesCapabilitiesTest" :app:lintNonRootDebug` — confirms strict `HostCapabilities` gating (never brand-gated) across all 7.0 demo states and verifies release APK (`nonRootRelease`) contains zero `assets/demo/` files.
4. **Nightjar Desktop Offline CPU Rasterizer Tests**:
   - `cargo test -p nightjar-desktop --locked shot_70` — rasterizes every 7.0.0 desktop demo state on CPU (Full-Bleed Home, Control Center floating dock & 6 visual cards, Visual Preview-Card Settings, First-Run Cinema, Launch Curtain, Stream Doctor 2.0, Input Deck Radial, Coherence, DVR, Quick Suspend) and asserts exact dimensions, non-blank pixels, zero stream-time blur pixels on `glass.menu.hud`, and zero egui layout panics.
5. **Architecture & Ratchet Guardrail**:
   - `python tools/arch_check.py` — confirms `ui.rs <= 1586`, `ui/demo.rs <= 859`, grandfathered `strings/*.rs <= 1750`, and all new `.rs`/`.java` files `<= 800` LOC with module doc headers.

### 7.2 Visual & Smoothness Inspection Criteria

- **Covers Contact Sheet**: inspect `out/demo/assets/covers-contact.png` at 100% zoom — zero cropped titles across 2:3 poster, square, and 16:9 full-bleed hero crops; strong ambient luminance balance in both Night and Day themes.
- **Rubylight 7.0 Stills & Tour**: inspect `out/console-shots/*.png` (1440 px & 390 px, Light & Dark), `out/host-surfaces/*.png` (Couch Menu, Installer QR Finish Screen, System Tray), and `out/console-tour/contact-*.png` — crisp 7.0 Studio typography and tokens, zero text overlap, `mpdecimate` smoothness `>= 55` distinct fps on active scroll/transition beats, `ffprobe` confirms H.264 High 60 fps, `moov` atom at front (`+faststart`), 0 audio streams.
- **Nightjar 7.0 Android Stills & Trailer**:
  - Run `pwsh client/scripts/demo/capture-stills.ps1` and `capture-video.ps1` on the caller's English Android emulator (`nj-demo2` / `nj-tv`, `-gpu host`).
  - Run `python client/scripts/demo/trailer.py --clips <dir> --out <dir>` and inspect `frames/`, `contact-*.png`, `smoothness-*.txt`, and 9:16 safe-area overlays (`y in [290, 1440]`, right `120 px` clear).
  - Verify `teaser.webp <= 2.5 MB`, `poster-16x9.webp` (`1280×720`), `poster-9x16.webp` (`720×1280`), zero touch-mode button hints on #151/#152 shots, and zero stream-time blur on closed-menu HUD shots.

### 7.3 Serialized Idle-Gated Hardware Criteria (Host Capture Only)

- Normal Playwright, CPU `shot.rs`, `covers.mjs`, `GenerateAssets.java`, offline Couch Menu/Installer/Tray renders, and Android `screenrecord` runs require **no host streaming session or GPU encode**.
- If `-HostCapture` (FFmpeg `ddagrab` + `h264_amf`) is used to record an emulator window on the RX 7900 XT host:
  1. Confirm the user is idle and `(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` shows `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0`.
  2. Never run concurrently with any `e2e.py` or GPU test batch.
  3. **Exit code 3 abort rule**: if any host encoder/capture validation returns **exit code 3** (AMD encoder hang), **stop immediately and investigate — never retry**.
  4. Never claim NVIDIA NVENC footage or benchmarks unless actually captured/measured on NVIDIA hardware.
