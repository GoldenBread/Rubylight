# Item 19: Rubylight & Nightjar Websites 10/10 — Ground-Up 7.0 Visual Redesign

Status: design, not implemented (Wave 5). Written against `main` `b3d0c26b9b` (`6.2.0`), `docs/designs/00-protocol-reservations.md`, and the Wave 0–4 ground-up **7.0 Console-Quality Complete Redesign**.

Item 19 executes a **complete ground-up 7.0 visual redesign** of BOTH static websites—**Rubylight 7.0** (`site/`, built by `site/build.py` into `_site/`) and **Nightjar 7.0** (`client/site/` + `de/`, plain static HTML/CSS/JS with zero runtime bundler)—around the brand-new **7.0 Console-Quality Brand Identity**. Both sites derive 100% of their CSS custom properties deterministically from Wave 0's canonical token sources (Wave 0's new `rust/web/src/styles/tokens.css` for Rubylight via `tools/sync_site_tokens.py`; Wave 0's new `client/design/tokens.json` for Nightjar via `client/scripts/gen_design_tokens.py` emitting `client/site/assets/design-tokens.css` — ZERO `sync-site-tokens.mjs`!), showcase verified Item 18 offline 7.0 captures across every redesigned surface, cite only real measured runs from `rust/PERFORMANCE.md`, enforce Nightjar's strict `< 6 MB` non-video asset ceiling via `client/site/check-links.mjs`, pass an automated `360 / 768 / 1440 px × Light / Dark` visual review matrix, and publish cleanly via history-preserving fast-forward `gh-pages` commits.

---

## 1. Summary & User Experience

### 1.1 Rubylight 7.0 Website (`https://ramazankara.github.io/Rubylight/`)

- **Ground-Up 7.0 Console-Quality Brand Identity (Zero Hardcoded 6.x Colors)**:
  - `site/style.css` is rebuilt around `site/tokens.css`, generated 1:1 from Wave 0's brand-new `rust/web/src/styles/tokens.css` by `tools/sync_site_tokens.py` (enforced with `--check`). **Zero legacy 6.x hex values are hardcoded in `site/style.css`, `site/index.html`, or `site/page.html`.**
  - Inherits every Wave 0 Studio Console 7.0 semantic role cleanly in both Light and Dark themes (`--bg`, `--panel`, `--panel-raised`, `--sunken`, `--ink`, `--ink-2`, `--muted`, `--line`, `--line-strong`, `--accent`, `--accent-strong`, `--accent-ink`, `--accent-soft`, `--accent-text`, `--brand`, `--brand-ink`, `--focus`, `--ok`, `--warn`, `--danger`, `--live`, `--media-*`, typography, radii, elevation, and motion scales), preserving `--brand` strictly for the BrandMark and `--accent` / `--accent-text` for primary actions and emphasis, with WCAG `>= 4.5:1` text contrast and `>= 3.0:1` focus/control contrast guaranteed by `rust/web/Tokens.test.mjs`.
- **Brand-New 7.0 Hero Banner, Social Card & Auto-Synced Download 7.0.0 CTA**:
  - New full-bleed 7.0 console-stage hero with refreshed `site/favicon.svg`, `site/og.png` (`1200×630` social card), `site/poster.jpg` (`1280×720`), and a `preload="none"` 60 fps Console 7.0 interactive tour video from Item 18.
  - `site/build.py::release()` automatically extracts `Download 7.0.0` and `https://github.com/RamazanKara/Rubylight/releases/tag/v7.0.0` directly from `README.md` into `{{download_label}}`, `{{download_url}}`, and `{{version}}`.
- **Showcases All 5 Redesigned Rubylight 7.0 Surfaces + Waves 1–3 Features**:
  1. **Web Studio Console 7.0** — interactive spotlights featuring verified Item 18 dark/light captures of **Console Command Stage**, **Display Topology Visualizer**, **Cover-Lit Per-Game Studio**, **QR Onboarding**, **`Ctrl+K` Command Palette**, and Item 17 plain-English Diagnostics;
  2. **Browser `/play` & `/guest` Players** — PIN-paired TV/console browser player and zero-account WebRTC browser guest rooms;
  3. **Host-Drawn Couch Menu** — universal in-stream overlay working on stock Moonlight, Artemis, and Nightjar alike with zero closed-path GPU cost;
  4. **Windows Installer & QR Finish Screen** — `rust/setup/` (`butterpollo-setup`) consuming canonical `rust/setup/src/ui_theme.rs`, one-click `--repair`, integrated `rubylight-vgamepad` VHF driver (zero unsigned KMDF), and instant post-install QR pairing;
  5. **System Tray & No-Script HTML Console Parity** — canonical `rust/windows/src/tray/status.rs` DPI-aware status badges & driver repair menu plus synchronized zero-JS HTML console fallback.
  - Clear, scannable Waves 1–4 feature architecture: **VHF Virtual Gamepads (`rubylight-vgamepad`)**, **Launch Curtain**, **Frame Warp 2.0**, **Stream DVR**, **HDR-to-SDR Tone-Mapping**, **Desktop Client Performance & NVENC Low-Latency Overhaul** (Windows DirectFlip/Waitable/D3D12 decode & Intel Linux DMA-BUF Wayland/KMS measured on local hardware; macOS direct VideoToolbox/Metal CI-verified & NVIDIA NVENC SDK 13.0 header/fake-driver-verified), **Quick Suspend**, **Host Hub Sync**, **Couch Party Audio**, **Stream Doctor 2.0**, **Visual Controller Editor & Client Input Deck**, **Coherence Workstation**, **Away Wake Relay**, and **Issue #129 Crash-Restore Soak (`RecoveryJournalV2`)**.
  - Explicitly preserves Windows 10 & 11, physical displays, WGC, Desktop Duplication, D3D11, AMD Radeon (AMF), and NVIDIA (NVENC) compatibility statements—describing NVIDIA NVENC and macOS Apple Silicon honestly as header/sample/CI-verified without ever claiming local NVIDIA or Apple Silicon hardware benchmark runs.
- **Honest `PERFORMANCE.md` Benchmark Strip**:
  - Every metric card links directly to its exact dated run anchor in `rust/PERFORMANCE.html#<anchor>` and matches `README.md` and `docs/performance.md` verbatim.
  - Hardware disclosure (`RX 7900 XT` / `Radeon 780M`) appears directly beneath the strip; demo stills and video captions explicitly state that overlay numbers are offline demo illustrations.
- **Rendered Documentation Site (`_site/docs/`, `_site/rust/`)**:
  - `site/build.py` renders every public Markdown doc into `_site/` using the new 7.0 `site/tokens.css` + `site/style.css`, excludes internal planning specs (`docs/designs/`) via `INTERNAL`, keeps GitHub-compatible heading IDs (`github_slug`), responsive tables (`mark_long_columns`), and strict zero-broken-link / zero-broken-fragment enforcement.

### 1.2 Nightjar 7.0 Website (`https://ramazankara.github.io/nightjar/`)

- **Ground-Up 7.0 Console-Quality Brand Identity via `gen_design_tokens.py` (ZERO `sync-site-tokens.mjs`!)**:
  - Every Nightjar page loads `assets/design-tokens.css` immediately before `assets/site.css` (zero client-side bundler or runtime build step).
  - `client/site/assets/design-tokens.css` is generated strictly from Wave 0's brand-new `client/design/tokens.json` by **`client/scripts/gen_design_tokens.py`** — the single `--check`-enforced token generator emitting Android XML (`design_tokens.xml`), Desktop Rust (`client/protocol/desktop/src/ui/design_tokens.rs`), and Nightjar site CSS (`client/site/assets/design-tokens.css`). **No `sync-site-tokens.mjs` or duplicate JS token script exists anywhere.**
  - `client/site/assets/site.css` is redesigned from the ground up to consume `--nj-*` custom properties for Light (Day), Dark (Night), and High-Contrast (`prefers-contrast: more`) schemes — zero hardcoded legacy 6.x hex literals anywhere in `site.css`.
- **Brand-New 7.0 Banners, Social Card (`og.png`) & Complete 7.0 Surface Showcase**:
  - Refreshed brand mark, 7.0 social card `client/site/assets/og.png` (`1200×630`), and Item 18 WebP showcase frames covering every redesigned Nightjar 7.0 surface:
    - **Cinematic Full-Bleed Console Home** (TV, Handheld, Phone, Desktop parity);
    - **Console Control Center Floating Dock & Visual Cards** (Quick Actions, Picture/VRR/Frame-Warp 2.0, Audio Mixer + Party Voice, Instant Clip DVR, Controller/Radial, Power/Quick Suspend);
    - **Visual Console-Grade Controller Editor** (live interactive controller diagram for DualSense / Xbox / Switch / Steam / Handheld, Sticks & Triggers curves, Gyroscope Studio, 4–12-slice Radial Menu Builder, Haptics & Lightbar, layered `Global → Model → Game` profiles synced via Host Hub Sync, replacing dozens of scattered flat settings);
    - **Visual Preview-Card Settings** (4-section Recommended-first + persistent *More settings* fold);
    - **First-Run & Pairing Cinema** (Obsidian Cinema LAN discovery + optical QR/PIN pairing);
    - **Modern Virtual Controller Studio (#151)** (Touch tab inside the Controller Editor with freely resizable/reshapable controls, separable ABXY/D-pad, movable floating micro-toolbar, and `0%` stealth mode);
    - **Phone Safe Area & Reverse Landscape (#152)**;
    - **Launch Curtain**, **Stream Doctor 2.0**, and **Coherence Workstation**.
- **All 8 HTML Pages Synced in Lockstep**:
  1. `client/site/index.html` (EN landing)
  2. `client/site/de/index.html` (DE landing — full 7.0 copy, feature, and caption parity with EN)
  3. `client/site/download.html` (EN download — 7.0.0 APK, SHA-256 checksums, preserved APK signing cert SHA-256 `312ed322c7c29d1aec4fae4a3979ebcb557af44d821d7f4746c0968548eb630b`, Windows/Linux/macOS instructions)
  4. `client/site/de/download.html` (DE download — full parity)
  5. `client/site/docs.html` (EN documentation hub linking every refreshed 7.0 guide)
  6. `client/site/devices.html` (sortable/filterable measured decoder results rendered from `data/device-results.json` via `assets/devices.js`)
  7. `client/site/privacy.html` (synced 1:1 with `client/docs/PRIVACY.md`, including 7.0 Host Hub Sync & Away Wake Relay local-first privacy guarantees)
  8. `client/site/404.html` (clean relative recovery page working from nested paths)
- **Zero-Autoplay 60 fps Responsive Trailer**:
  - Native `<video controls preload="none" playsinline>` referencing refreshed 60 fps `assets/poster-16x9.webp` and `assets/trailer-16x9.mp4` in static markup; `assets/site.js` swaps cleanly to `assets/poster-9x16.webp` and `assets/trailer-9x16.mp4` on `(max-width: 600px)` only while `currentTime === 0 && paused`. Zero bytes of video download until the visitor presses Play; zero autoplay.
- **Strict `< 6 MB` Non-Video Asset Budget**:
  - Every non-video file in `client/site/assets/` (`.webp`, `.png`, `.svg`, `.css`, `.js`) is budgeted to `<= 4.2 MiB` (target) and hard-capped strictly below `6 * 1024 * 1024` bytes (`6144 KiB`), enforced automatically by `client/site/check-links.mjs`.
- **Host-Neutral Messaging**:
  - Nightjar's website treats Sunshine, Apollo, Vibepollo, Rubylight, and GameStream as first-class hosts, framing 7.0 host-backed features (Launch Curtain, Stream DVR, Quick Suspend, Host Hub Sync, Couch Party Audio, Stream Doctor 2.0, Input Deck, Coherence, Away Wake Relay) honestly as capability-activated extras driven strictly by `HostCapabilities` (never host brand).

---

## 2. Protocol, Native API & Capability Contract

Per `docs/designs/00-protocol-reservations.md` §7, **Item 19 requires ZERO protocol, control-stream, or Native API wire changes.** `PROTOCOL_VERSION` and `rubylight_native_api::API_VERSION = 1` stay unchanged.

### 2.1 Site-to-Repository Contracts

| Contract | Authoritative Source of Truth | Consumer / Enforcer |
| --- | --- | --- |
| Rubylight Download Version & Tag | `README.md` (`[Download 7.0.0](https://github.com/RamazanKara/Rubylight/releases/tag/v7.0.0)`) | `site/build.py::release()` -> `{{download_label}}`, `{{download_url}}`, `{{version}}` |
| Rubylight Benchmark Figures | `rust/PERFORMANCE.md` dated run sections (RX 7900 XT / Radeon 780M only) | `site/index.html` stat cards + `site/build.py::check_landing()` fragment verification |
| Rubylight 7.0 Visual Tokens | Wave 0's new `rust/web/src/styles/tokens.css` `[verify post-sprint code-map.md]` (zero legacy 6.x hardcoded colors) | `site/tokens.css` generated by `tools/sync_site_tokens.py` (`--check` enforced in CI) and consumed by `site/style.css` |
| Nightjar 7.0 Visual Tokens | Wave 0's new `client/design/tokens.json` `[verify post-sprint code-map.md]` (zero legacy 6.x hardcoded colors) | `python client/scripts/gen_design_tokens.py --check` (the SOLE generator of Android XML, Desktop `ui/design_tokens.rs`, and `client/site/assets/design-tokens.css`; ZERO `sync-site-tokens.mjs`!) |
| Nightjar Privacy Policy | `client/docs/PRIVACY.md` | `client/site/privacy.html` (section-heading & policy-point sync verified in review checklist) |
| Nightjar Device Results | `client/site/data/device-results.json` (format v1 via `client/scripts/add-device-results.mjs`) | `client/site/devices.html` + `client/docs/devices.md` (via `tools/generated_docs.py`) |
| Nightjar Asset Budget, Links & Honesty | `client/site/check-links.mjs` | Enforces 0 broken local/repo files, 0 missing/duplicate `#ids`, HTTPS-only external links, `preload="none"` + zero autoplay on `<video>`, zero `"Experimental"` / mod names, and total non-video `assets/` `< 6 MB` (`6144 KiB`). |

---

## 3. Architecture & <=800-LOC Module Plan

All paths below must be re-verified against `docs/code-map.md` and `client/docs/code-map.md` in Phase 2 (`[verify post-sprint code-map.md]`).

### 3.1 Grandfathered Files in `tools/size_ratchet.json` (DO NOT GROW)

Item 19 modifies only HTML, CSS, JS, Python, SVG, PNG, WebP, and MP4 assets under `site/`, `client/site/`, `client/scripts/`, and `tools/`. **Zero grandfathered `.rs`, `.java`, `.ts`, or `.svelte` files in `tools/size_ratchet.json` are modified by Item 19.** (Coordination note: `client/site/assets/design-tokens.css` is produced by Wave 4 Slice `16C` via `gen_design_tokens.py`; Wave 5 Slice `19B` extends `gen_design_tokens.py` only if additional site-level semantic CSS aliases from `tokens.json` are required, keeping `--check` green across all 3 generated outputs.)

### 3.2 File & Module Plan (Every Script/Style Module Kept Modular & Compact)

| Path | Target LOC | Responsibility |
| --- | ---: | --- |
| `tools/sync_site_tokens.py` *(new)* | ~190 | Deterministically extracts Wave 0's new 7.0 light/dark/reduced-motion CSS custom properties from `rust/web/src/styles/tokens.css` into `site/tokens.css`, validates zero legacy hardcoded palette drift in `site/style.css`, and supports `--check` for CI. |
| `site/tokens.css` *(new, generated)* | ~120 | Generated 1:1 from Wave 0's `rust/web/src/styles/tokens.css` (surfaces, typography, radii, elevation, `--accent*`, `--brand*`, `--focus`, status & media tokens for Light and Dark schemes). |
| `site/build.py` `[verify post-sprint code-map.md]` | ~490 | Adds `site/tokens.css` & `site/og.png` to `STATIC`, excludes `docs/designs/` via `INTERNAL`, runs token-sync and banned-term checks (forbidding `"Experimental"` and mod names in landing copy), builds `_site/`, and verifies every link + GitHub heading `#fragment`. |
| `site/style.css` `[verify post-sprint code-map.md]` | ~600 | Ground-up 7.0 Console-Quality redesign consuming `site/tokens.css` exclusively (zero raw legacy 6.x hex values); responsive console-stage hero, 5-surface spotlight grid, benchmark strip, high-contrast & `@media (prefers-reduced-motion: reduce)` compliance across `360 / 768 / 1440 px`. |
| `site/index.html` `[verify post-sprint code-map.md]` | ~420 | Ground-up Rubylight 7.0 landing page showcasing all 5 redesigned surfaces (Web Studio Console 7.0, Browser `/play` & `/guest`, Host-Drawn Couch Menu, Installer QR Finish Screen, System Tray & No-Script parity), Waves 1–3 capabilities, `PERFORMANCE.md`-anchored strip, `preload="none"` 60 fps tour video, auto-synced `Download 7.0.0` CTA, and Nightjar cross-link. |
| `site/page.html` `[verify post-sprint code-map.md]` | ~95 | Documentation shell loading `site/tokens.css` + `site/style.css` with skip-link, accessible landmarks, and updated 7.0 brand header. |
| `site/favicon.svg`, `site/og.png`, `site/poster.jpg` | assets | Refreshed 7.0 BrandMark favicon, `1200×630` social card, and `1280×720` tour video poster derived from Item 18. |
| `client/scripts/gen_design_tokens.py` `[verify post-sprint code-map.md]` | ~195 | Extended sole `--check`-enforced token generator from Wave 0's new `client/design/tokens.json` to Android XML, Desktop Rust (`client/protocol/desktop/src/ui/design_tokens.rs`), and `client/site/assets/design-tokens.css` (adding Light/Dark/High-Contrast CSS custom property blocks directly from `tokens.json`; ZERO `sync-site-tokens.mjs`!). |
| `client/scripts/test_gen_design_tokens.py` `[verify post-sprint code-map.md]` | ~140 | Unit tests verifying deterministic LF generation and Light/Dark/High-Contrast CSS custom properties emitted into `client/site/assets/design-tokens.css`. |
| `client/site/assets/design-tokens.css` *(generated by `gen_design_tokens.py`)* | ~115 | Generated 7.0 Nightjar CSS custom properties from `client/design/tokens.json` (zero hand edits). |
| `client/site/assets/site.css` `[verify post-sprint code-map.md]` | ~240 | Ground-up 7.0 Nightjar Console-Quality redesign consuming `--nj-*` variables from `design-tokens.css` (zero legacy 6.x hardcoded hex colors); full-bleed showcase cards, Control Center feature grid, `@media (prefers-contrast: more)` & `@media (prefers-reduced-motion: reduce)`, zero overflow at `360 / 768 / 1440 px`. |
| `client/site/assets/site.js` `[verify post-sprint code-map.md]` | ~48 | Zero-autoplay 16:9 ↔ 9:16 responsive trailer swap (`preload="none"`, never swaps once playback starts). |
| `client/site/check-links.mjs` `[verify post-sprint code-map.md]` | ~160 | Enforces `< 6 MB` non-video asset ceiling, local/anchor/repo-doc links, `gen_design_tokens.py --check` alignment, `preload="none"` + zero-`autoplay` on `<video>`, zero raw legacy 6.x hex overrides in `site.css`, and banned-term checks (`Experimental` / mod names). |
| `client/site/index.html`, `de/index.html` `[verify post-sprint code-map.md]` | ~170 each | Ground-up Nightjar 7.0 EN & DE landing pages showcasing Full-Bleed Console Home, Console Control Center floating dock & 6 visual cards, Visual Preview-Card Settings, First-Run & Pairing Cinema, Modern Virtual Controller Studio (#151), Phone Safe Area / Reverse Landscape (#152), Desktop/Handheld/TV parity, refreshed WebP shots, and 60 fps trailer. |
| `client/site/download.html`, `de/download.html` `[verify post-sprint code-map.md]` | ~135 each | Updated 7.0.0 Android APK (preserving SHA-256 signing fingerprint `312ed322c7c29d1aec4fae4a3979ebcb557af44d821d7f4746c0968548eb630b`) + Windows/Linux/macOS downloads in EN & DE. |
| `client/site/docs.html`, `devices.html`, `privacy.html`, `404.html` `[verify post-sprint code-map.md]` | ~115 each | Updated 7.0 docs hub, `device-results.json` table, `PRIVACY.md`-synced privacy page, and 404 recovery page, all loading `design-tokens.css` before `site.css`. |
| `tools/site_review.mjs` *(new)* | ~300 | Playwright automated visual review runner capturing Rubylight `_site/` and all 8 Nightjar `client/site/` pages at **`360×800`, `768×1024`, and `1440×900`** in **Light & Dark** themes + reduced-motion & contrast assertions, writing PNGs into gitignored `_review/` folders and failing on any horizontal overflow, broken image, or autoplaying video. |

### 3.3 Nightjar Non-Video Asset Budget Allocation (`< 6 MB` Hard Cap)

| Asset Group (`client/site/assets/`) | Target Budget | Format & Constraints |
| --- | ---: | --- |
| CSS + JS (`design-tokens.css`, `site.css`, `site.js`, `devices.js`) | `<= 45 KiB` | Plain static files, zero external CDN fonts or scripts. |
| Brand marks & 7.0 social card (`launcher.png`, `favicon.png`, `og.png`) | `<= 240 KiB` | Brand-new 7.0 `og.png` (`1200×630` optimized PNG); `launcher.png` (`192×192`). |
| Trailer posters (`poster-16x9.webp`, `poster-9x16.webp`) | `<= 320 KiB` | `1280×720` and `720×1280` WebP (quality 82). |
| Redesigned 7.0 App Showcase Stills (`shot-home-cinema.webp`, `shot-control-center.webp`, `shot-settings-cards.webp`, `shot-first-run.webp`, `shot-virtual-pad.webp`, `shot-reverse-landscape.webp`, `shot-pcs.webp`, `shot-desktop-parity.webp`, `shot-doctor.webp`) | `<= 3,400 KiB` | Crisp 7.0 WebP stills (`720 px` portrait / `1600×900` landscape, quality 78–84, zero legacy unused 6.x shots retained). |
| **Total Non-Video `assets/`** | **`<= 4.0 MiB`** | **Comfortably below the `6.0 MiB` (`6144 KiB`) hard limit enforced by `client/site/check-links.mjs` (~2.0 MiB headroom).** |
| 60 fps Video (`trailer-16x9.mp4`, `trailer-9x16.mp4`) | Excluded from 6 MB cap | `preload="none"`, H.264 High 60 fps `+faststart`, silent, never autoplayed. |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

Static websites have no streaming reconnect loop; this section defines the **Token-Sync, Build, Visual-Matrix & Safe History-Preserving `gh-pages` Deployment State Machine** and rollback invariants so neither GitHub Pages branch is ever broken, force-pushed, or re-orphaned.

### 4.1 Safe History-Preserving `gh-pages` Deployment State Machine

```text
  [CLEAN_MAIN_SHA] (CI passed on main; release tag or approved deploy SHA)
         |
         v  Step 1: Token Sync, Build & Link Verification (Local + CI)
            - Rubylight: python tools/sync_site_tokens.py --check && python site/build.py
            - Nightjar:  python client/scripts/gen_design_tokens.py --check
                         node client/site/check-links.mjs && node --check client/site/assets/site.js
  [BUILD_VERIFIED]
         |
         v  Step 2: 360 / 768 / 1440 px × Light / Dark Visual Matrix Review
            - node tools/site_review.mjs --site rubylight --out _site_review
            - node tools/site_review.mjs --site nightjar  --out client/site/_review
            - Zero horizontal overflow, zero broken images, preload="none" + zero autoplay verified
  [VISUAL_APPROVED] (maintainer confirms publish)
         |
         v  Step 3: Clone Existing `gh-pages` Branch into Fresh Temp Dir
            git clone --branch gh-pages --single-branch <remote> <tempDir>
            (NEVER `git init` a new orphan branch when `gh-pages` already exists!)
  [STAGED_ON_HISTORY]
         |
         v  Step 4: Sync Output into Temp Dir (preserving `.git/` and `.nojekyll`,
            excluding `_review`, `.gitignore`, `README.md`, `preview.mjs`, `check-links.mjs`)
            Inspect `git status` and `git diff --stat`
  [DIFF_REVIEWED]
         |
         +--> unexpected deletion / asset bloat? --> [ABORT_DELETE_TEMP] (remote untouched!)
         |
         v  Step 5: Normal Fast-Forward Commit & Push (NEVER `--force`!)
            git commit -m "docs(site): publish 7.0.0 redesign"
            git push origin gh-pages
  [PUBLISHED] --> smoke-check live URL + delete temp dir
```

### 4.2 Failure & Rollback Matrix

| Failure Mode | Detection | Safe Recovery |
| --- | --- | --- |
| Broken internal link or renamed Markdown heading `#fragment` | `python site/build.py` or `node client/site/check-links.mjs` exits 1 with exact file & URL. | Fix link/anchor in source Markdown or HTML before build succeeds. |
| `README.md` Download 7.0.0 link missing or mismatched | `site/build.py::release()` aborts build immediately. | Sync `README.md` `[Download 7.0.0](.../releases/tag/v7.0.0)` first. |
| Token drift between Wave 0 app tokens (`rust/web/src/styles/tokens.css` / `client/design/tokens.json`) and site CSS | `python tools/sync_site_tokens.py --check` or `python client/scripts/gen_design_tokens.py --check` exits 1. | Re-run the canonical token generator and commit generated CSS (never hand-patch hex values). |
| Non-video Nightjar assets exceed 6 MB | `client/site/check-links.mjs` exits 1 printing exact byte count. | Re-encode/resize oversized WebP shots or prune unused 6.x stills; never raise the 6 MB cap. |
| Horizontal overflow at `360 px` mobile viewport | `tools/site_review.mjs` fails assertion `scrollWidth <= innerWidth`. | Fix CSS grid wrapping / table scroll container before publish. |
| `git push origin gh-pages` rejected (non-fast-forward) | Another publish landed on `gh-pages`. | **NEVER force-push.** Re-clone latest `gh-pages`, re-sync clean output on top, inspect diff, push normally. |
| Regression discovered after live publish | Post-publish smoke check fails. | `git revert HEAD` on `gh-pages` clone and push normally — preserving full audit history. |

---

## 5. Settings & 10/10 Craft Plan

- **Fewer-Settings Rule**: Both websites follow OS `prefers-color-scheme: light | dark`, `prefers-contrast: more`, and `prefers-reduced-motion: reduce` natively with zero theme-picker cookies, zero localStorage bloat, and zero third-party analytics or trackers.
- **Default-Off Gate & Performance Honesty**:
  - Every benchmark figure on `site/index.html` and `client/site/index.html` comes strictly from measured release runs in `rust/PERFORMANCE.md` (with run date, RX 7900 XT / Radeon 780M disclosure, and direct link).
  - Embedded Item 18 stills and trailer captions explicitly label scripted telemetry overlays as offline demo illustrations.
- **WCAG Contrast, Plain-English Copy & Accessibility**:
  - Every text/surface pair meets WCAG `>= 4.5:1` (`>= 7:1` under `prefers-contrast: more`), and every `:focus-visible` outline, button border, and control meets `>= 3.0:1`.
  - Semantic HTML5 landmarks (`<header>`, `<nav aria-label>`, `<main id="main" tabindex="-1">`, `<footer>`), keyboard skip-links (`Skip to content`), minimum `44×44` CSS px hit targets on all links/buttons/controls, descriptive `alt` text on every image, and `aria-label` on every `<video>`.
  - `@media (prefers-reduced-motion: reduce)` sets `scroll-behavior: auto` and disables decorative motion on both sites.
- **Dynamic Input Glyphs & Zero Stream-Time Blur Fidelity**:
  - Embedded Item 18 stills accurately depict dynamic Xbox/PlayStation/Nintendo/Keyboard/TV-Remote input glyphs (and zero button hints on phone touch/Virtual Controller Studio #151/Reverse Landscape #152 frames) plus zero stream-time blur when menus are closed.
  - Both static sites keep CSS lightweight without expensive full-page blur layers; sticky header glass falls back gracefully when `backdrop-filter` or transparency is disabled.
- **Zero `"Experimental"` & Zero Mod Names**:
  - Neither site's UI copy nor any embedded screenshot contains `"Experimental"` or any mod name (`Nucleus Co-op` / `ANR` stay strictly out of UI visuals and Nightjar site copy; Rubylight's site describes co-op and frame generation in plain user terms and links to `docs/` where appropriate).
- **Multi-Language Parity**:
  - Nightjar site preserves full EN (`index.html`, `download.html`) ↔ DE (`de/index.html`, `de/download.html`) parity with `hreflang="en"`, `hreflang="de"`, and `hreflang="x-default"` links, and states clearly that Nightjar ships in all **15** languages and Rubylight's host console ships in all **22** locale files.

---

## 6. Disjoint Implementer Slices

Every slice below owns a strictly non-overlapping file list for Wave 5 `parallel()` execution (all depend on Wave 0 tokens settling, Wave 4 Slice `16C` finalizing `tokens.json` outputs, and Item 18 media assets being finalized first).

### Slice 19A: Rubylight 7.0 Site (`site/`) Wave 0 Token Sync & Ground-Up Redesign
- **Files (exclusive)**:
  - `tools/sync_site_tokens.py` *(new)*
  - `site/tokens.css` *(new, generated from Wave 0's `rust/web/src/styles/tokens.css`)*
  - `site/build.py` `[verify post-sprint code-map.md]`
  - `site/style.css` `[verify post-sprint code-map.md]`
  - `site/page.html` `[verify post-sprint code-map.md]`
  - `site/index.html` `[verify post-sprint code-map.md]`
  - `site/favicon.svg`, `site/og.png`, `site/poster.jpg` *(refreshed 7.0 brand & Item 18 tour media)*

### Slice 19B: Nightjar 7.0 Site (`client/site/` + `de/`) `gen_design_tokens.py` Sync & 8-Page Ground-Up Redesign
- **Files (exclusive)**:
  - `client/scripts/gen_design_tokens.py` *(extend sole `--check`-enforced generator of `client/site/assets/design-tokens.css`; ZERO `sync-site-tokens.mjs`!)* `[verify post-sprint code-map.md]`
  - `client/scripts/test_gen_design_tokens.py` `[verify post-sprint code-map.md]`
  - `client/site/assets/design-tokens.css` *(regenerated strictly by `client/scripts/gen_design_tokens.py`)*
  - `client/site/assets/site.css` `[verify post-sprint code-map.md]`
  - `client/site/assets/site.js` `[verify post-sprint code-map.md]`
  - `client/site/check-links.mjs` `[verify post-sprint code-map.md]`
  - `client/site/README.md` `[verify post-sprint code-map.md]`
  - `client/site/index.html`, `client/site/download.html`, `client/site/docs.html`, `client/site/devices.html`, `client/site/privacy.html`, `client/site/404.html`
  - `client/site/de/index.html`, `client/site/de/download.html`
  - `client/site/assets/*.webp`, `client/site/assets/og.png`, `client/site/assets/launcher.png`, `client/site/assets/favicon.png`, `client/site/assets/trailer-*.mp4` *(refreshed 7.0 brand & Item 18 media)*

### Slice 19C: Automated 360 / 768 / 1440 px × Light / Dark Visual Review Harness
- **Files (exclusive)**:
  - `tools/site_review.mjs` *(new)*
  - `tools/tests/test_site_review.mjs` *(new)*

---

## 7. Verification & Measurement Plan

### 7.1 Build, Token-Sync & Link Checks (No GPU / No Hardware Required)

1. **Rubylight 7.0 Site**:
   - `python tools/sync_site_tokens.py --check` — asserts `site/tokens.css` matches Wave 0's `rust/web/src/styles/tokens.css` byte-for-byte and `site/style.css` contains zero hardcoded legacy 6.x palette values.
   - `python site/build.py` — runs built-in `self_test()`, builds `_site/`, excludes `docs/designs/`, validates every link and GitHub heading `#fragment` in `README.md`, `site/index.html`, and all rendered `docs/*.md` + `rust/*.md` pages, verifies `Download 7.0.0` extraction, and rejects `"Experimental"` or mod names in landing copy.
2. **Nightjar 7.0 Site**:
   - `python client/scripts/gen_design_tokens.py --check && python client/scripts/test_gen_design_tokens.py` — proves zero drift between Wave 0's `client/design/tokens.json` and Android XML, Desktop `ui/design_tokens.rs`, and `client/site/assets/design-tokens.css`.
   - `node client/site/check-links.mjs` — asserts 0 broken local/repo files, 0 missing/duplicate `#ids`, HTTPS-only external links, `preload="none"` + zero autoplay on `<video>`, zero banned terms, and total non-video `assets/` strictly `< 6144 KiB`.
   - `node --check client/site/assets/site.js` and `node --check client/site/assets/devices.js`.
3. **Architecture & Code-Map Sanity**:
   - `python tools/arch_check.py` (verifies zero unintended source-file regressions).

### 7.2 Visual Review Matrix (Automated Playwright + Human Sign-Off)

Run `tools/site_review.mjs` against local preview servers (`python -m http.server` over `_site/` and `node client/site/preview.mjs` at `http://127.0.0.1:4317/nightjar/`):

| Site | Pages Checked | Viewports | Color Schemes | Automated Assertions |
| --- | --- | --- | --- | --- |
| Rubylight 7.0 (`_site/`) | `index.html`, `docs/index.html`, `docs/getting-started.html`, `rust/RELEASE_NOTES.html` | `360×800`, `768×1024`, `1440×900` | `light`, `dark` | `scrollWidth <= innerWidth`; all `<img>` `complete && naturalWidth > 0`; `<video>` `preload === "none"` & `paused === true`; skip-link focusable; all interactive targets `>= 44×44 px`; zero console errors; reduced-motion sets `scroll-behavior: auto`. |
| Nightjar 7.0 (`client/site/`) | All 8 pages (`index`, `de/index`, `download`, `de/download`, `docs`, `devices`, `privacy`, `404`) | `360×800`, `768×1024`, `1440×900` | `light`, `dark` | Same as above + at `360 px` `<video>` swaps to `trailer-9x16.mp4` / `poster-9x16.webp` without starting playback; at `768 / 1440 px` uses `trailer-16x9.mp4`; `EN` ↔ `DE` switcher preserves route; `design-tokens.css` loaded before `site.css`; `devices.html` filter/sort works cleanly on empty and populated JSON. |

Review PNGs are saved into gitignored `_site_review/` and `client/site/_review/` and visually inspected at 100% zoom before pushing `gh-pages`.

### 7.3 Hardware & Release Gate Criteria

- Item 19 requires **zero GPU execution, zero host streaming sessions, and zero live hardware runs**.
- Should any cited benchmark in `rust/PERFORMANCE.md` be refreshed before publication, that measurement MUST follow the serialized idle-gated hardware procedure (`RustHostSessionCount == 0`, isolated test host on port `48123`, **immediate stop on exit code 3** AMD encoder hang without retry) and update `rust/PERFORMANCE.md`, `README.md`, `docs/performance.md`, and `site/index.html` in the same commit.
- Live `gh-pages` deployment happens **only** after maintainer OK on a CI-green `main` SHA, strictly via history-preserving fast-forward push (never force-pushing or re-orphaning).

---

## 8. Post-Discovery & 6.3.0 (`fc1704feb3`) Rebase Addendum

- **Build Cleanly Atop `6.3.0` (`fc1704feb3`) Site Structure:** Commit `fc1704feb3` already redesigned `site/index.html`, `site/style.css`, and `client/site/index.html` around `.pillars` and the 5-stage `.pipeline` with an explicit policy against stale release-specific static charts. Wave 5 Slice `19A`/`19B` MUST reconcile against `fc1704feb3` cleanly: keep the crisp 7.0 Obsidian Studio / Obsidian Cinema token pipeline (`sync_site_tokens.py` + `gen_design_tokens.py`), and for performance references link directly to dated `rust/PERFORMANCE.html#<anchor>` runs (or clearly dated, auto-checked excerpts from `rust/PERFORMANCE.md` — updating any contradictory HTML comment in `site/index.html` cleanly). Also highlight Items `27` (local-first privacy & anonymized support bundles), `28` (multi-GPU + locked-PC wake), and `29` (high-refresh & Android pacing) honestly alongside `21`.
