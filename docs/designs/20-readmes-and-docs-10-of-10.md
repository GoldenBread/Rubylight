# Item 20: READMEs & Documentation 10/10 — Rubylight 7.0 & Nightjar 7.0 Launch

Status: design, not implemented (Wave 5). Written against `main` `b3d0c26b9b` (`6.2.0`), `docs/designs/00-protocol-reservations.md`, and the Wave 0–4 ground-up **7.0 Console-Quality Complete Redesign**.

Item 20 delivers the complete 7.0.0 overhaul of `README.md`, `client/README.md`, `rust/web/DESIGN.md`, `client/design/README.md`, `docs/`, `client/docs/`, `NOTICE`, and `rust/THIRD_PARTY.md` around the brand-new **7.0 Console-Quality Brand Identity**. Both READMEs launch with brand-new 7.0 hero banners, animated teasers, and refreshed stills verified legible at `1440 px` and `393 px` via `readme-preview.ps1`; both design-system specs reflect the ground-up 7.0 redesign; every protocol ID, all **11** Native API v1 feature strings (`stream.settings` included), every config key, error anchor, and module path is synchronized and verified by automated checks (`code_map.py check`, `generated_docs.py --check`, `check-doc-links.mjs`, `site/build.py`, and `check_docs_honesty.py`); and all GPL-3.0, Moonlight V+, upstream port, and contributor credits follow `AGENTS.md` strictly.

---

## 1. Summary & User Experience

### 1.1 `README.md` (Rubylight 7.0.0)

- **Brand-New 7.0 Above-the-Fold Hero (Scannable in 10 Seconds)**:
  - Brand-new **7.0 Console-Quality SVG Hero Banner** (`docs/media/banner.svg`) and matching download CTA artwork (`docs/media/readme/download.svg`) aligned with Wave 0's redesigned 7.0 Studio Console identity, one-sentence value proposition ("Every game. Every screen. Every player."), release/downloads/GPL-3.0 badges, and the canonical `**[Download 7.0.0](https://github.com/RamazanKara/Rubylight/releases/tag/v7.0.0)**` anchor parsed by `site/build.py::release()`.
  - Refreshed 60 fps Console 7.0 animated teaser (`docs/media/readme/console-tour.webp` linking to `console-tour.mp4`) captured strictly from Item 18's offline fixture mode with 100% original procedural 7.0 console-art covers and an explicit `(demo library and sample data)` caption.
- **Honest 7.0.0 Feature Matrix & Redesigned Surface Spotlights**:
  - Refreshed light/dark `<picture>` stills showcasing all 5 redesigned Rubylight 7.0 surfaces (**Console Command Stage**, **Display Topology Visualizer**, **Cover-Lit Per-Game Studio**, **QR Onboarding & `Ctrl+K`**, **Browser `/play` & `/guest`**, **Host-Drawn Couch Menu**, **Installer QR Finish Screen**, and **System Tray**), verified legible at **`1440 px`** and **`393 px`** via `client/scripts/demo/readme-preview.ps1`.
  - Updates the "What you get" grid and detailed sections to describe 7.0.0 accurately:
    - **VHF Virtual Gamepads (`rubylight-vgamepad`)**: native Windows Virtual HID Framework DualSense, Switch Pro, Xbox, and Steam Controller emulation with zero unsigned KMDF drivers (ViGEmBus stays optional for tools such as reWASD).
    - **Launch Curtain** (`0x5548`), **Frame Warp 2.0** (`0x5549`), **Stream DVR** (`0x554A`), **HDR-to-SDR Tone-Mapping** (`rlSdrToneMap` + `stream.settings` / `video.sdr_tonemap`), **Desktop Client Performance & NVENC Low-Latency Overhaul (`21`)** (Windows DirectFlip/Waitable/D3D12 decode, Linux Wayland DMA-BUF explicit sync & KMS atomic, macOS direct VideoToolbox/Metal/AVSampleBufferDisplayLayer, Host NVENC D3D12 pre-registered async ring + ULL VBV/AQ + LTR-RFI + pooled 4:4:4/RTX interop), **Quick Suspend** (`0x554B`), **Host Hub Sync** (`hub.sync`), **Couch Party Audio** (`0x554C`), **Stream Doctor 2.0** (`stream.doctor_v2`), **Visual Controller Editor & Client Input Deck** (`0x554D` + `0x5500_0081`), **Coherence Workstation** (`0x554E`), and **Away Wake Relay** (`wake.relay`).
    - **Crash-Restore & Craft Resilience (Item 17)**: plain-English diagnostics, one-click `setup.exe --repair` (`-p butterpollo-setup`), and 10-cycle crash-restore DPI/display/Quick-Suspend soak verification on canonical `RecoveryJournalV2` (`recovery-journal.json` + `--recover-session`, #129).
  - Benchmark summary quotes ONLY real measured runs from `rust/PERFORMANCE.md` (with run date, GPU/client model, and direct link), clearly stating AMD AMF / Windows Desktop (measured on RX 7900 XT / Radeon 780M), Intel Linux Desktop (measured on Intel NUC), macOS Apple Silicon (verified in `nightjar-macos.yml` CI only), and NVIDIA NVENC (supported on Windows 10 & 11; checked against vendor SDK 13.0 headers/samples and fake-driver tests without claiming local NVIDIA hardware runs).

### 1.2 `client/README.md` (Nightjar 7.0.0)

- **Brand-New 7.0 Host-Neutral Hero & Capability Table**:
  - Brand-new **7.0 Console-Quality Hero Banner** (`client/docs/assets/banner.png`) and matching `client/docs/assets/download.svg` aligned with Wave 0's redesigned Nightjar 7.0 brand identity.
  - Keeps Nightjar's host-neutral promise front and center: works out-of-the-box with Sunshine, Apollo, Vibepollo, Rubylight, NVIDIA GameStream, and every Moonlight-compatible host; 7.0 extras activate strictly via `HostCapabilities` (never from host brand).
  - Refreshed animated teaser (`client/docs/assets/readme/nightjar-tour.webp` -> `nightjar-tour.mp4`, plus `client/docs/assets/nightjar-trailer.webp`) and refreshed 7.0 stills across EVERY redesigned surface: **Cinematic Full-Bleed Console Home**, **Console Control Center Floating Dock & 6 Visual Cards** (Quick Actions, Picture/VRR/Frame-Warp 2.0, Audio Mixer + Party Voice, Instant Clip DVR, Controller/Radial, Power/Quick Suspend), **Visual Console-Grade Controller Editor** (live DualSense/Xbox/Switch/Steam/Handheld diagram, Sticks & Triggers Studio, Gyroscope Studio, Radial Menu Builder, Haptics & Lightbar, layered profiles replacing scattered flat controller keys), **Visual Preview-Card Settings**, **First-Run & Pairing Cinema**, **Modern Virtual Controller Studio (#151)**, **Phone Safe Area / Reverse Landscape (#152)**, Launch Curtain, Stream Doctor 2.0, Coherence Workstation, and **Desktop / Handheld / TV parity** (Night, Day, OLED Black, High Contrast).
  - Verified legible at **`1440 px`** and **`393 px`** via `client/scripts/demo/readme-preview.ps1`.

### 1.3 Complete Design-System & Reference Documentation Synchronization

Every design spec, protocol reference, user guide, code map, and attribution file is brought into exact 7.0.0 sync with zero stale paths, zero hand-edited generated blocks, and zero unmeasured claims:
- **Design System Specs (Complete Ground-Up 7.0 Sync)**:
  - **`rust/web/DESIGN.md`**: Updated completely to document Wave 0's ground-up **Rubylight 7.0 Console-Quality Design System** — the new `rust/web/src/styles/tokens.css` hierarchy, WCAG `>= 4.5:1` text / `>= 3.0:1` focus-and-control contrast enforcement (`Tokens.test.mjs`, `RawCssValues.test.mjs`), Console Command Stage, Display Topology Visualizer, Cover-Lit Per-Game Studio, QR Onboarding, `Ctrl+K`, Browser `/play` & `/guest`, Host-Drawn Couch Menu, Installer QR Finish Screen (`rust/setup/src/ui_theme.rs`), System Tray (`rust/windows/src/tray/status.rs`), No-Script Console parity, and all **22** host locales (`rust/assets/locale/*.json`).
  - **`client/design/README.md`**: Updated completely to document Wave 0's ground-up **Nightjar 7.0 Console-Quality Design System** — the new `client/design/tokens.json` schema generated into Android XML, Desktop Rust (`ui/design_tokens.rs`), and Nightjar site CSS (`client/site/assets/design-tokens.css`) by `client/scripts/gen_design_tokens.py`, 4-tier glass (`lens`/`live`/`baked`/`solid`) with shared `nightjar-policy::glass_probe`, full Night / Day / OLED / High-Contrast parity across **both Android and Desktop** (removing stale *"Desktop is night only"* text), Cinematic Full-Bleed Console Home, Console Control Center floating dock & 6 visual cards, Visual Preview-Card Settings, First-Run & Pairing Cinema, Modern Virtual Controller Studio (#151), Phone Safe Area (#152), hard zero stream-time blur invariant when closed, dynamic input glyphs via canonical `client/protocol/policy/src/input_glyphs.rs` (zero hints for touch/mouse), and 15-locale Figtree Latin-1 vs. OS system font rules + canonical chained `client/protocol/desktop/src/strings_70/<code>.rs`.
- **Protocol & API Contracts**: `docs/control-messages.md`, `docs/native-api.md` (explicitly documenting all **11** native feature strings including `stream.settings` and the preserved `ErrorBody { error: ApiError { ... } }` envelope), and `docs/api.md` match `docs/designs/00-protocol-reservations.md` and the landed Rust/Java code byte-for-byte.
- **Configuration & Settings**: `docs/configuration.md` (host, hand-authored with the exact "what you get / what it costs / when to change it" formula) and `client/docs/settings.md` + `client/docs/devices.md` (strictly regenerated via `python tools/generated_docs.py` and verified with `--check`, never hand-edited inside generated markers).
- **Features & Troubleshooting**: `docs/features.md` (clear 7.0.0 matrix across Stock Moonlight, Nightjar any-host, Nightjar + Rubylight 7.0, AMD AMF vs. NVIDIA NVENC, Win10 vs. Win11, physical displays/WGC/DDX/D3D11) and `docs/troubleshooting.md` (every heading anchor matched 1:1 to `rust/core/src/log_analysis/` rules, `RecoveryJournalV2` / `--recover-session` #129 DPI recovery, and `setup.exe --repair`).
- **Code Maps**: `docs/code-map.md` and `client/docs/code-map.md` updated for every Wave 0–4 crate, module, symbol, checklist, and narrowest test — passing `python tools/code_map.py check` with zero warnings.
- **Legal & Port/Contributor Credits**: `NOTICE` and `rust/THIRD_PARTY.md` updated for every 7.0.0 dependency, ported file, shader, and font (`third-party/licenses/`), explicitly preserving **Moonlight V+** port credits (for canonical `client/protocol/policy/src/input_glyphs.rs` + `client/protocol/desktop/src/glyphs.rs`) and all upstream credits in code comments/`NOTICE`/`THIRD_PARTY.md`/README credits (NEVER in UI text or release notes), while external contributor PRs are credited strictly in `rust/release/notes.d/` and `client/notes.d/` (`Thanks to **Name** (@login, #N).`).

---

## 2. Protocol, Native API & Capability Contract

Per `docs/designs/00-protocol-reservations.md` §7, **Item 20 requires ZERO wire, protocol, or Native API code changes** — its contract is **100% documentation accuracy** against `00-protocol-reservations.md` and the implemented Wave 0–4 code. `PROTOCOL_VERSION` and `rubylight_native_api::API_VERSION = 1` remain unchanged.

### 2.1 Documentation-to-Code Source-of-Truth Matrix

| Documentation File | Authoritative Code / Design Source | Exact 7.0.0 Synchronization Required |
| --- | --- | --- |
| `rust/web/DESIGN.md` `[verify post-sprint code-map.md]` | `rust/web/src/styles/tokens.css`, `Tokens.test.mjs`, `RawCssValues.test.mjs`, `rust/windows/src/tray/status.rs`, `rust/setup/src/ui_theme.rs` | Full ground-up 7.0 Studio Console design spec covering all 5 host surfaces (Web Studio Console 7.0, Browser `/play` & `/guest`, Host-Drawn Couch Menu, Installer QR Finish Screen, System Tray + No-Script Console parity), WCAG `>= 4.5:1` / `>= 3.0:1` rules, and 22-locale discipline. |
| `client/design/README.md` `[verify post-sprint code-map.md]` | `client/design/tokens.json`, `client/scripts/gen_design_tokens.py`, `client/protocol/policy/src/glass_probe.rs`, `client/protocol/policy/src/input_glyphs.rs`, `client/protocol/desktop/src/strings_70/` | Full ground-up 7.0 Nightjar Console-Quality design spec covering 4-tier glass, Night/Day/OLED/High-Contrast parity across Android AND Desktop, Full-Bleed Console Home, Console Control Center floating dock & 6 visual cards, Visual Preview-Card Settings, First-Run Cinema, Virtual Controller Studio (#151), Phone Safe Area (#152), zero stream-time blur, dynamic glyphs (zero touch/mouse hints), and 15-locale font + `strings_70` policy. |
| `docs/control-messages.md` `[verify post-sprint code-map.md]` | `client/protocol/core/src/ids.rs` + `rust/core/src/extensions.rs` + `00-protocol-reservations.md` | Document `RUBYLIGHT_7_CONTROL_IDS` (`0x5548` Launch Curtain, `0x5549` Frame Warp 2.0, `0x554A` Stream DVR, `0x554B` Quick Suspend, `0x554C` Couch Party Audio, `0x554D` Input Deck, `0x554E` Coherence Window, `0x554F` reserved headroom), input magic `0x5500_0081` (`INPUT_DECK_VIRTUAL_AXIS_MAGIC`), `/serverinfo` elements, `DESCRIBE` lines, launch opt-ins, and `PROTOCOL_VERSION` invariance. |
| `docs/native-api.md` `[verify post-sprint code-map.md]` | `rust/core/native-api/src/` + `rust/host/src/native_api/` | Document `API_VERSION = 1` additive invariant, **all 11 new native feature strings** (`session.launch_curtain`, `stream.dvr`, `stream.settings`, `video.sdr_tonemap`, `session.quick_suspend`, `hub.sync`, `audio.party`, `stream.doctor_v2`, `input.deck`, `session.coherence`, `wake.relay`), every route including per-route 64 KiB body limits (`PUT /hub/profiles/{id}`, `PUT /input/decks/{id}` returning `ErrorCode::TooLarge` / HTTP `413`), permission scopes (`view`/`list`/`launch`/`host_control`), and preserved `ErrorBody { error: ApiError { code, message, setting, retry_after_ms, title?, next_step?, technical_detail? } }` envelope. |
| `docs/api.md` `[verify post-sprint code-map.md]` | `rust/host/src/web.rs` + `rust/host/src/web/` | Document 7.0 Console endpoints (`/api/system/driver-health`, `/api/system/repair`, `/api/dvr/**`, `/api/session/suspend`, `/api/stream/doctor`, display topology & QR onboarding payloads, `/play` & `/guest` routes), CSRF/token scopes, and error formats. |
| `docs/configuration.md` `[verify post-sprint code-map.md]` | `rust/core/src/config.rs`, `overrides.rs`, `retired.rs`, `settings30.rs` | Document all 7.0.0 host config keys, defaults, override precedence (`app+device > app > device > host`), retired/renamed keys, and measurement links. |
| `docs/features.md` `[verify post-sprint code-map.md]` | `rust/core/src/extensions.rs` + `client/protocol/client/src/http/capabilities.rs` | Update 7.0.0 matrix: Stock Moonlight vs. Nightjar (any host) vs. Nightjar + Rubylight 7.0; AMD AMF vs. NVIDIA NVENC; Win10 vs. Win11; physical displays/WGC/DDX/D3D11; Android vs. Desktop (Win/Linux/macOS). |
| `docs/troubleshooting.md` `[verify post-sprint code-map.md]` | `rust/core/src/log_analysis/` + `rust/core/src/recovery_journal.rs` (Item 17) | Match every `anchor` constant in `rules_encoder.rs`, `rules_display.rs`, and `rules_driver_net.rs` 1:1 to GitHub-slugged headings, plus plain-English guides for `RecoveryJournalV2` (`recovery-journal.json` / `--recover-session` #129) and `setup.exe --repair` (`-p butterpollo-setup`). |
| `docs/code-map.md` & `client/docs/code-map.md` `[verify post-sprint code-map.md]` | Repository tree + `tools/code_map.py` | Update crate tables, stream hop tables, feature areas, checklists, and narrowest-test commands for all Wave 0–4 modules (including `tray/status.rs`, `ui_theme.rs`, `recovery_journal.rs`, `input_glyphs.rs`, `strings_70/`, `ui/demo_70.rs` via `ui.rs`, and `-p butterpollo-setup`). |
| `client/docs/settings.md` & `client/docs/devices.md` `[verify post-sprint code-map.md]` | `client/scripts/generate-settings.py`, `client/scripts/device-docs.mjs`, `tools/generated_docs.py` | Regenerate strictly via `python tools/generated_docs.py`; verify zero drift via `python tools/generated_docs.py --check`. |
| `NOTICE` & `rust/THIRD_PARTY.md` `[verify post-sprint code-map.md]` | `third-party/licenses/` + Cargo/Gradle/npm manifests | Complete file-by-file upstream port credit, original author, license, and GPL-3.0 §5/§7(e) compliance — explicitly preserving **Moonlight V+** (`client/protocol/policy/src/input_glyphs.rs` + `client/protocol/desktop/src/glyphs.rs`), Spleen font, Figtree, and all 7.0 additions. |

---

## 3. Architecture & <=800-LOC Module Plan

All paths below must be re-verified against `docs/code-map.md` and `client/docs/code-map.md` in Phase 2 (`[verify post-sprint code-map.md]`).

### 3.1 Grandfathered Files in `tools/size_ratchet.json` (DO NOT GROW)

Item 20 modifies only Markdown documentation, SVG/PNG/WebP README assets, PowerShell preview helpers, and Python/Node doc-checking tools. **Zero grandfathered `.rs`, `.java`, `.ts`, or `.svelte` source files in `tools/size_ratchet.json` are modified.**

### 3.2 Documentation & Verification Tooling Plan

| File / Tool Path | Target LOC | Responsibility |
| --- | ---: | --- |
| `README.md` `[verify post-sprint code-map.md]` | ~400 | Rubylight 7.0.0 root README: new 7.0 banner (`docs/media/banner.svg`), `Download 7.0.0` CTA, 60 fps Console 7.0 tour, 5-surface & Wave 1–4 feature matrix, refreshed light/dark `<picture>` stills, honest `PERFORMANCE.md` figures, Nightjar section, preserved credits. |
| `client/README.md` `[verify post-sprint code-map.md]` | ~350 | Nightjar 7.0.0 README: new 7.0 banner (`client/docs/assets/banner.png`), host-neutral hero, refreshed 7.0 stills across Full-Bleed Home, Control Center Dock & 6 Visual Cards, Preview-Card Settings, First-Run Cinema, Virtual Controller Studio (#151), Reverse Landscape (#152), Desktop/Handheld/TV parity, install & privacy summary. |
| `rust/web/DESIGN.md` `[verify post-sprint code-map.md]` | ~140 | Complete ground-up 7.0 Studio Console design system reference (all 5 host surfaces, Wave 0 tokens, WCAG rules, `RawCssValues.test.mjs`, 22 locales). |
| `client/design/README.md` `[verify post-sprint code-map.md]` | ~240 | Complete ground-up 7.0 Nightjar Console-Quality design system reference (`tokens.json`, `gen_design_tokens.py`, 4-tier glass, full Android+Desktop theme parity, zero stream-time blur, `input_glyphs.rs`, 15 locales + `strings_70`). |
| `docs/control-messages.md`, `docs/native-api.md`, `docs/api.md` `[verify post-sprint code-map.md]` | updated in place | Complete 7.0.0 protocol, Native API v1 (all 11 feature strings including `stream.settings` + `ErrorBody { error: ApiError }` envelope), and Console 7.0 web API docs. |
| `docs/configuration.md`, `docs/features.md`, `docs/troubleshooting.md` `[verify post-sprint code-map.md]` | updated in place | Targeted 7.0.0 section updates (search-first per `AGENTS.md`, never rewriting unrelated historical entries unnecessarily). |
| `docs/code-map.md`, `client/docs/code-map.md` `[verify post-sprint code-map.md]` | ~225 / ~185 | Synchronized architecture, crate, and symbol maps passing `python tools/code_map.py check`. |
| `client/docs/settings.md`, `client/docs/devices.md` `[verify post-sprint code-map.md]` | generated | Strictly regenerated via `python tools/generated_docs.py` (with `client/scripts/generate-settings.py` updated for `ui_sounds`, #152 options, and Wave 1–3 Nightjar keys). |
| `NOTICE`, `rust/THIRD_PARTY.md` `[verify post-sprint code-map.md]` | updated in place | Complete 7.0.0 third-party & ported-code attribution audit preserving Moonlight V+ (`input_glyphs.rs` + `glyphs.rs`) and every upstream credit. |
| `rust/release/notes.d/7.0.0-summary.md` & `client/notes.d/7.0.0-summary.md` *(new fragments)* | ~45 each | 7.0.0 release-note fragments (external contributor PR credits in bold `Thanks to **Name** (@login, #N).`; zero port credits; zero unmeasured claims; zero mod names). |
| `client/scripts/demo/readme-preview.ps1` `[verify post-sprint code-map.md]` | ~110 | Enhanced to accept `-Docs .` (Rubylight `README.md`) and `-Docs client` (Nightjar `client/README.md`) at `1440 px` and `393 px`, plus optional `-Theme dark` so `<picture>` dark-mode README images are visually verified alongside light-mode ones. |
| `tools/check_docs_honesty.py` *(new)* | ~280 | Automated prose & compliance linter checking `README.md`, `client/README.md`, `rust/web/DESIGN.md`, `client/design/README.md`, `docs/`, `client/docs/`, `site/`, and `client/site/` for banned superlatives/unmeasured claims, unanchored benchmark numbers, NVIDIA HW run claims, UI `"Experimental"` labels, forbidden mod names in Nightjar/UI/release notes, all 11 native feature strings present in `docs/native-api.md`, Moonlight V+ credit preservation in `NOTICE`/`THIRD_PARTY.md`, and 100% `log_analysis/` -> `docs/troubleshooting.md` anchor coverage. |
| `tools/tests/test_check_docs_honesty.py` *(new)* | ~160 | Unit tests verifying every honesty/compliance rule (true positives + false-positive exclusions for code blocks/historical notes). |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

Documentation has no runtime streaming state; this section defines the **Generated-Doc Merge, Preview & Pre-Release Review Gate** so parallel Wave 5 documentation work never produces stale tables, merge conflicts inside generated blocks, or mismatched release links.

### 4.1 Documentation Pipeline & Pre-Release Review State Machine

```text
  [WAVE_4_CODE_FROZEN] (all 7.0.0 tokens, symbols, config keys, control IDs, 11 native features landed)
           |
           v  Step 1: Regenerate Derived Docs & Verify Token Generators
              python tools/generated_docs.py          (updates client/docs/settings.md & client/docs/devices.md)
              python client/scripts/gen_design_tokens.py --check
              python tools/sync_site_tokens.py --check
  [GENERATED_CURRENT]
           |
           v  Step 2: Author Hand-Written Docs, Design Specs & READMEs (Parallel Slices 20A–20E)
              - Merge driver `rubylight-generated` resolves generated-table conflicts automatically
                (.gitattributes: merge=rubylight-generated -> re-run `python tools/generated_docs.py`)
  [DOCS_AUTHORED]
           |
           v  Step 3: Automated Verification Gate (ALL must exit 0)
              1. python tools/code_map.py check
              2. python tools/generated_docs.py --check
              3. node client/scripts/check-doc-links.mjs
              4. python site/build.py
              5. python tools/check_docs_honesty.py
  [STATIC_VERIFIED]
           |
           v  Step 4: Visual README Legibility Check (1440 px & 393 px, Light & Dark)
              pwsh client/scripts/demo/readme-preview.ps1 -Docs .      -Desktop out/rl-1440.png -Mobile out/rl-393.png
              pwsh client/scripts/demo/readme-preview.ps1 -Docs client -Desktop out/nj-1440.png -Mobile out/nj-393.png
  [VISUAL_VERIFIED]
           |
           v  Step 5: Full-Release Review (per AGENTS.md)
              Review git diff from previous release tag (v6.2.0) to candidate 7.0.0 SHA + assembled release notes
              Fix any findings BEFORE tagging; tag/publish ONLY with maintainer OK
  [READY_FOR_7_0_0_TAG]
```

### 4.2 Conflict & Drift Recovery Matrix

| Failure / Drift Scenario | Automated Catch | Resolution |
| --- | --- | --- |
| Symbol renamed or file moved after Wave 0–4 refactor | `python tools/code_map.py check` fails naming exact broken path or missing symbol. | Fix path/symbol in `docs/code-map.md` or `client/docs/code-map.md` immediately. |
| Nightjar setting added/changed without regenerating `settings.md` | `python tools/generated_docs.py --check` exits 1 listing stale file. | Run `python tools/generated_docs.py` and commit result (never hand-edit inside generated markers). |
| Git merge conflict inside generated table of `settings.md` or `devices.md` | `tools/generated_docs.py merge` driver auto-takes `--ours` when conflict is solely inside markers and instructs regen. | Run `python tools/generated_docs.py` after merge. |
| Broken relative path, wrong filename case, or missing heading anchor in `client/docs/` | `node client/scripts/check-doc-links.mjs` exits 1 (including exact filename case check on Windows!). | Fix path case or heading anchor. |
| Broken link/anchor in `README.md` or `docs/*.md`, or bad `Download 7.0.0` link | `python site/build.py` exits 1 listing every broken URL or missing GitHub `#fragment`. | Fix link/anchor or `README.md` release link. |
| `log_analysis/` `anchor` missing from `docs/troubleshooting.md` | `tools/check_docs_honesty.py` + `cargo test -p butterpollo-core log_analysis` fail. | Add matching heading to `docs/troubleshooting.md`. |
| Missing any of the 11 native feature strings in `docs/native-api.md` or missing Moonlight V+ credit in `NOTICE` | `tools/check_docs_honesty.py` exits 1 naming the missing identifier/credit. | Sync `docs/native-api.md` or `NOTICE` / `rust/THIRD_PARTY.md`. |
| Unmeasured superlative ("fastest", "zero latency", "lag-free") or NVIDIA HW run claim | `tools/check_docs_honesty.py` fails with exact file + line. | Replace with factual mechanism description or link a real measured run from `rust/PERFORMANCE.md`. |

---

## 5. Settings & 10/10 Craft Plan

- **Fewer-Settings Documentation Discipline**:
  - Every host setting in `docs/configuration.md` and every Nightjar setting in `client/scripts/generate-settings.py` (`client/docs/settings.md`) follows the exact two-sentence formula: **(1) what the player gets, (2) what it costs, and (3) when to change it** ("Leave it on unless…"). Internal code/struct names stay out of user-facing descriptions.
  - Retired and renamed keys are documented transparently with migration behavior and never reused.
- **Default-Off Gate & Honest Claims**:
  - Every default-off / opt-in feature (e.g., `coherence_workstation`, `away_wake_relay`, `ui_sounds`, `encoder_focus`) is explicitly marked **Off by default (opt-in)** with clear criteria for when to turn it on.
  - **Zero superlatives or unmeasured claims**: banned phrases (`"world's fastest"`, `"zero latency"`, `"100% lag-free"`, `"revolutionary"`, `"best-in-class"`, `"blazing fast"`, `"most beautiful"`) are blocked by `tools/check_docs_honesty.py`. Every millisecond, FPS, or VMAF number must cite a real dated run in `rust/PERFORMANCE.md`.
- **Plain-English Troubleshooting**:
  - `docs/troubleshooting.md` leads every entry with the symptom in everyday words, explains the cause in 1–2 sentences, gives numbered recovery steps, and puts raw log/error codes in a secondary "What the log says" line.
- **Dynamic Input Glyphs, Zero Stream-Time Blur & Accessibility Docs**:
  - `client/design/README.md`, `client/docs/features.md`, and `client/README.md` accurately document dynamic Xbox / PlayStation / Nintendo / Steam Deck / Keyboard / TV Remote glyph switching (**and zero button hints for touch/mouse**), zero stream-time blur when menus are closed, WCAG `>= 4.5:1` / `>= 3.0:1` contrast, TalkBack/AccessKit keyboard accessibility, high contrast, reduced motion, all **15** Nightjar locales (`en`, `de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`), and all **22** host locales (`rust/assets/locale/*.json`).
- **Zero `"Experimental"` in UI & GPL-3.0 Attribution Discipline**:
  - Docs never instruct users to look for an `"Experimental"` badge in the UI.
  - Upstream ports — explicitly including **Moonlight V+** (`client/protocol/policy/src/input_glyphs.rs` + `client/protocol/desktop/src/glyphs.rs`), Spleen bitmap font, Figtree, and all 7.0 third-party ports — are credited accurately in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README credits, and NEVER placed in UI text or release notes.
  - External contributor PRs merged for 7.0.0 are credited in bold (`Thanks to **Name** (@login, #N).`) strictly in `rust/release/notes.d/` or `client/notes.d/`.
  - Host reference docs (`docs/`) state honestly that Rubylight bundles neither Nucleus Co-op nor ANR (and ANR is downloaded only at user request), while zero mod names appear anywhere in Nightjar docs/README, Studio Console UI text, or release notes (GPL-3.0 §7(e)).

---

## 6. Disjoint Implementer Slices

Every slice below owns a strictly non-overlapping file list for Wave 5 `parallel()` execution (all depend on Wave 4 code and Item 18 media assets finalizing first).

### Slice 20A: Root & Client READMEs, 7.0 Hero Banners & Preview Script
- **Files (exclusive)**:
  - `README.md` `[verify post-sprint code-map.md]`
  - `client/README.md` `[verify post-sprint code-map.md]`
  - `client/scripts/demo/readme-preview.ps1` `[verify post-sprint code-map.md]`
  - `docs/media/banner.svg`, `docs/media/readme/*` *(new 7.0 hero banner, download CTA, refreshed stills & tour from Item 18)*
  - `client/docs/assets/banner.png`, `client/docs/assets/download.svg`, `client/docs/assets/nightjar-trailer.webp`, `client/docs/assets/readme/*` *(new 7.0 hero banner, download CTA, refreshed stills & tour from Item 18)*

### Slice 20B: Host Protocol, Native API (All 11 Features) & Web API Reference Docs
- **Files (exclusive)**:
  - `docs/control-messages.md` `[verify post-sprint code-map.md]`
  - `docs/native-api.md` *(documents all 11 new native feature strings including `stream.settings`, 64 KiB routes, and preserved `ErrorBody { error: ApiError }` envelope)* `[verify post-sprint code-map.md]`
  - `docs/api.md` `[verify post-sprint code-map.md]`

### Slice 20C: 7.0 Design-System Specs + Host Configuration, Features & Troubleshooting Docs
- **Files (exclusive)**:
  - `rust/web/DESIGN.md` *(complete ground-up 7.0 Studio Console design spec sync)* `[verify post-sprint code-map.md]`
  - `client/design/README.md` *(complete ground-up 7.0 Nightjar Console-Quality design spec sync)* `[verify post-sprint code-map.md]`
  - `docs/configuration.md` `[verify post-sprint code-map.md]`
  - `docs/features.md` `[verify post-sprint code-map.md]`
  - `docs/troubleshooting.md` `[verify post-sprint code-map.md]`

### Slice 20D: Code Maps, Generated Client Docs, Legal Notices & Release-Note Fragments
- **Files (exclusive)**:
  - `docs/code-map.md` `[verify post-sprint code-map.md]`
  - `client/docs/code-map.md` `[verify post-sprint code-map.md]`
  - `client/scripts/generate-settings.py` `[verify post-sprint code-map.md]`
  - `client/docs/settings.md` *(regenerated via `python tools/generated_docs.py`)* `[verify post-sprint code-map.md]`
  - `client/docs/devices.md` *(regenerated via `python tools/generated_docs.py`)* `[verify post-sprint code-map.md]`
  - `client/docs/PRIVACY.md` *(synced for Away Wake Relay / Host Hub Sync local-first notes)* `[verify post-sprint code-map.md]`
  - `NOTICE` *(preserves Moonlight V+ & every upstream port credit)* `[verify post-sprint code-map.md]`
  - `rust/THIRD_PARTY.md` *(preserves Moonlight V+ & every upstream port credit)* `[verify post-sprint code-map.md]`
  - `rust/release/notes.d/7.0.0-summary.md` *(new fragment)*
  - `client/notes.d/7.0.0-summary.md` *(new fragment)*

### Slice 20E: Automated Honesty, Design-Spec Sync & Cross-Anchor Linter
- **Files (exclusive)**:
  - `tools/check_docs_honesty.py` *(new)*
  - `tools/tests/test_check_docs_honesty.py` *(new)*

---

## 7. Verification & Measurement Plan

### 7.1 Mandatory Automated Verification Suite (All Must Exit 0)

Run in a single verification chain after Slices 20A–20E merge:

```powershell
python tools/code_map.py check; `
python tools/generated_docs.py --check; `
python client/scripts/gen_design_tokens.py --check; `
python tools/sync_site_tokens.py --check; `
node client/scripts/check-doc-links.mjs; `
python site/build.py; `
python tools/check_docs_honesty.py; `
python -m unittest tools/tests/test_check_docs_honesty.py; `
python tools/arch_check.py
```

Each check enforces a distinct invariant:
1. `python tools/code_map.py check` — every file path and linked symbol name in `docs/code-map.md` and `client/docs/code-map.md` exists in the repo.
2. `python tools/generated_docs.py --check` — `client/docs/settings.md` and `client/docs/devices.md` match their generators byte-for-byte.
3. `python client/scripts/gen_design_tokens.py --check` & `python tools/sync_site_tokens.py --check` — confirms zero token drift between Wave 0 design sources (`client/design/tokens.json`, `rust/web/src/styles/tokens.css`), generated code/CSS, and synced design documentation.
4. `node client/scripts/check-doc-links.mjs` — every relative path (exact filename case!) and heading `#fragment` across `client/README.md` and `client/docs/**/*.md` resolves cleanly.
5. `python site/build.py` — every link and GitHub `#fragment` across `README.md`, `site/index.html`, `docs/*.md`, and `rust/*.md` resolves cleanly, and `Download 7.0.0` parses validly.
6. `python tools/check_docs_honesty.py` — zero banned superlatives/unmeasured claims, zero unanchored performance stats, zero NVIDIA hardware run claims, zero UI `"Experimental"` references, zero mod names in UI/Nightjar/release notes, all 11 native feature strings (`stream.settings` included) present in `docs/native-api.md`, Moonlight V+ port credit intact in `NOTICE` and `rust/THIRD_PARTY.md`, and 100% `log_analysis/` -> `docs/troubleshooting.md` anchor coverage.
7. `python tools/arch_check.py` — confirms zero unintended source-file violations.

### 7.2 Visual README Legibility Verification (`1440 px` & `393 px`)

Render both READMEs with GitHub's GFM renderer via headless Edge and visually inspect every PNG:

```powershell
pwsh client/scripts/demo/readme-preview.ps1 -Docs . -Desktop out/readme-rubylight-1440.png -Mobile out/readme-rubylight-393.png
pwsh client/scripts/demo/readme-preview.ps1 -Docs client -Desktop out/readme-nightjar-1440.png -Mobile out/readme-nightjar-393.png
```

Pass criteria:
- Brand-new 7.0 hero banners, Download CTAs, badges, and animated teasers stay crisp, legible, and unclipped at both `1440 px` desktop and `393 px` mobile widths.
- Every `<picture>` dark/light image renders cleanly with descriptive `alt` text and explicit `DEMO` / `(demo library and sample data)` captions where applicable.
- Feature tables wrap cleanly with zero horizontal scrollbar distortion at `1440 px` and readable cell stacking at `393 px`.

### 7.3 Hardware & Pre-Tag Release Gate Criteria

- Item 20 itself runs **zero GPU workloads, zero builds, and zero hardware streaming sessions**.
- Before tagging `v7.0.0` (host or Nightjar), per `AGENTS.md`:
  1. Confirm the candidate SHA passed all GitHub Actions workflows (`rust-windows.yml`, `nightjar-android.yml`, `nightjar-macos.yml`).
  2. Perform a complete release review from the previous release tag (`v6.2.0`) to the candidate SHA together with the assembled release notes from `rust/release/notes.d/` and `client/notes.d/`, fixing any discrepancy before tagging.
  3. If any fresh hardware benchmark is required to substantiate a number in `README.md` or `rust/PERFORMANCE.md`, execute it strictly under the serialized idle-gated protocol (`RustHostSessionCount == 0`, isolated test host on port `48123`, **immediate abort on exit code 3** AMD encoder hang without retry).
  4. Never create or push a release tag without explicit maintainer approval.
