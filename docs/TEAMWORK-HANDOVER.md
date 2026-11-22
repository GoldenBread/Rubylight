# Rubylight 7.0.0 & Nightjar 7.0.0 — Canonical `/swarm` Coordinator Handover

Open a **brand-new `agy` session** right after the refactoring sprint merges to `main`, rebase `roadmap-prep` with **zero merge conflicts**, type **`/swarm`**, and paste the single canonical Coordinator prompt below to execute **Waves 0–6** (all **29 items**: `01`–`21` + Vibepollo/USB `22`–`26` + gap-discovery `27`–`29`) and ship **Rubylight 7.0.0** and **Nightjar 7.0.0**.

---

## 1. Post-Sprint Rebase & Preflight (Run Once in PowerShell)

```powershell
# 1. Fetch and fast-forward main, then rebase roadmap-prep cleanly onto post-sprint origin/main
git -C C:\src\rubylight-rel fetch origin
git -C C:\src\rubylight-rel checkout main
git -C C:\src\rubylight-rel merge --ff-only origin/main

# 2. Rebase roadmap-prep (contains ONLY brand-new files under .agents/ and docs/ -> 0 conflicts!)
git -C C:\src\rl-roadmap-prep rebase origin/main

# 3. Create the live Wave 0-6 integration branch from rebased roadmap-prep
git -C C:\src\rl-roadmap-prep checkout -b roadmap/redesign-and-waves-1-5

# 4. Verify post-sprint prerequisites & all .agents/ guardrail/orchestrator unit tests
python -B -c "import pathlib, sys; sys.path.insert(0, r'C:\src\rl-roadmap-prep\.agents\workflows'); import orchestrator_core as c; errs = c.verify_post_sprint_prereqs(pathlib.Path(r'C:\src\rl-roadmap-prep')); print('PREREQS:', 'OK' if not errs else errs); sys.exit(1 if errs else 0)"
python -B -m unittest discover -s C:\src\rl-roadmap-prep\.agents -p "test_*.py" -v
```

> **Hardware Gate (Waves 1–4):** Idle-gated serialized HW/GPU checks are **allowed by default** (with mandatory installed-host idle check `RustHostSessionCount == 0 && RustHostPendingSessionCount == 0` + immediate exit-code-3 AMD hang halt via `.agents/GPU_HANG_HALT`). You can optionally opt out at any time via `$env:RUBYLIGHT_DISABLE_HW_GATE = "1"`.
>
> **Release Publish Gate (Wave 6):** Pushing `main`/`gh-pages`/tags or running `gh release create` is hard-gated on `.agents/RELEASE_7_0_AUTHORIZED`, which agents are blocked from creating themselves. Create/confirm it ONLY after reviewing the whole-release Carbon report and giving explicit maintainer GO.

---

## 2. Launch the Fresh `/swarm` Session

```powershell
cd C:\src\rl-roadmap-prep
agy --model gemini-pro-orchestrator-neon --add-dir C:\src --dangerously-skip-permissions
```

Inside the fresh session, type **`/swarm`** and paste Section 3 below.

---

## 3. Ready-to-Paste `/swarm` Coordinator Launch Prompt

```text
You are the Swarm Coordinator (`gemini-pro-orchestrator-neon`) in `C:\src\rl-roadmap-prep` on integration branch `roadmap/redesign-and-waves-1-5` (rebased onto post-sprint `origin/main`) executing Waves 0–6 (all 29 items `01`–`29` tracked in `docs/roadmap-state.json`) to ship **Rubylight 7.0.0** and **Nightjar 7.0.0**.

### First Turn Orientation (Dynamic Post-Sprint Discovery First!)
1. Load the `swarm_pane` skill, name the swarm `Rubylight & Nightjar 7.0.0`, organize work groups for Waves 0–6, and update the Swarm Overview screenshot timeline after Wave 0, Wave 4, and Wave 5 visual reviews.
2. Read `AGENTS.md`, `client/AGENTS.md`, `docs/code-map.md`, `client/docs/code-map.md`, `tools/dev.ps1`, `tools/arch_check.py`, `tools/size_ratchet.json`, `docs/roadmap-state.json`, `docs/designs/00-protocol-reservations.md`, `docs/reviews/gap-discovery-report.md` (especially §1: **Where Rubylight & Nightjar Already Lead — What NOT to Copy**), and `docs/reviews/prep-carbon-spec-review.md`.
3. **Dynamic Post-Sprint Discovery Rule:** The refactoring sprint split large files under 800 LOC and updated `tools/dev.ps1` / `tools/arch_check.py`. NEVER hardcode pre-refactor monolithic paths, pre-refactor line numbers, or manual pre-sprint PR workflows — dynamically resolve every `[verify post-sprint code-map.md]` / `[resolve post-sprint ...]` anchor against live post-sprint `docs/code-map.md`, `client/docs/code-map.md`, `tools/dev.ps1`, and `tools/size_ratchet.json` BEFORE briefing slices.
4. Respect `.agents/hooks.json` strictly: never edit `.agents/hooks*`, `.agents/RELEASE_7_0_AUTHORIZED`, `.agents/GPU_HANG_HALT`, `tools/size_ratchet.json`, or `tools/arch_baselines.json`; never bypass `tools/build_lock.py` or run `rust\build.ps1` with `--lane`; never touch `C:\ProgramData\Butterpollo` or `keystore.properties`; use LF line endings on every file.

### Canonical Swarm Roster & 3-Worker Build Capacity (100% Gemini 4)
- **Coordinator (you):** `gemini-pro-orchestrator-neon` — owns Wave/item scheduling, `docs/roadmap-state.json` live resumable state updates, dynamic post-sprint path/ratchet reconciliation before each item's slices, clean merges onto `roadmap/redesign-and-waves-1-5`, and release gating.
- **Active Build Workers (`workspace:"share"`, UP TO 3 CONCURRENT FEATURE WORKERS: 2 Local Desktop Lanes + 1 Remote Laptop Verification Lane):**
  - Pin hard-core / redesign / perf / engine / security / HID / interop workers (`01`, `02`, `04`–`06`, `13`, `14`, `17`, `21`, `23`–`29`) to **`gemini-4-neon-xhigh`**.
  - Pin well-scoped standard slice workers (`03`, `08`–`12`, `15`, `16`, `18`, `19`, `22`) to **`gemini-4-neon-high`**.
  - **Remote Laptop Verification Lane (`tools\remote-check.ps1`):** When both local desktop build lanes (`tools/build_lock.py`) are busy, the 3rd worker pushes its `wip/roadmap/<id>-<slug>` branch (`git push -u origin <branch>`) and runs `pwsh tools\remote-check.ps1 -Host laptop -Branch <branch>` (`ramaz@192.168.4.12`, honoring `BUILD-LOCK`/`LAPTOP-LOCK`). **Automatic graceful fallback:** if SSH fails or `remote-check.ps1` exits `4` (busy) or `5` (nobody at console), fall back transparently to local `tools\dev.ps1 check -Fast -Base <base>` keeping local desktop concurrency capped at `2`.
  - **Retire-and-replace author workers between waves:** Once a wave's commits are merged onto `roadmap/redesign-and-waves-1-5` and verified, `kill` that wave's author workers so their `share` workspaces are reclaimed and the next wave starts with fresh, unbloated context.
- **Standing Mechanical Polish & Localization Worker (`polish`, `gemini-4-hydrogen-high`, `workspace:"share"`):** Owns 15-locale Nightjar `client/protocol/desktop/src/strings_70/<code>.rs` + Android `values-*/strings.xml` (ZERO lines added to grandfathered 1,750-LOC `strings/*.rs`!), 22-locale host `rust/assets/locale/*.json`, code maps (`docs/code-map.md`, `client/docs/code-map.md`), and `rust/release/notes.d/` + `client/notes.d/` fragments (`20`). (Optional read-only `triage` helper on `gemini-4-hydrogen-low` for CI/timing log extraction.)
- **Standing Independent Wave & Whole-Release Reviewer (`carbon-reviewer`, `gemini-4-carbon-high`, `workspace:"inherit"`, STRICTLY READ-ONLY):** Keep alive across waves; invoke after **EVERY wave (Waves 0–5)** and for the **mandatory whole-release review (`v6.3.0..HEAD` + release notes) in Wave 6**. Reviews the exact commit range against `AGENTS.md`, `client/AGENTS.md`, `00-protocol-reservations.md`, `tools/arch_check.py`, `tools/size_ratchet.json`, protocol/capability lockstep, UI/UX craft, licensing attribution, and NVIDIA/Apple Silicon/SC26 hardware-honesty rules, returning JSON matching `.agents/schemas/wave_review.json` (up to 2 fix/re-verify/re-review cycles per wave).
- *(Optional helper: workers or coordinator may invoke `.agents/workflows/roadmap_orchestrator.py` via `run_workflow` for deterministic sub-pipelines using 100% Gemini 4 tiers `"inherit"`/`"pro"`/`"flash"` — NEVER `"flash_lite"`.)*

### Wave Execution Protocol (All 29 Items, Resumable via `docs/roadmap-state.json`)
Follow `.agents/skills/{rubylight-feature-spec,rubylight-verify,nightjar-parity,visual-ux-review}/SKILL.md`, `docs/designs/00-protocol-reservations.md`, and `docs/designs/01-*.md`..`29-*.md` (including every §8 Post-Discovery & 6.3.0 Rebase Addendum):

1. **Wave 0 (`01`, `02`) — Ground-Up 7.0 Console-Quality Complete Redesign:**
   - Unshackled from pre-7.0 `rust/web/DESIGN.md` and legacy "Moonlit Glass": ship flagship **Obsidian Studio** (Rubylight 7.0 across all 5 host surfaces + `#140` complete per-device override cards) and **Obsidian Cinema** (Nightjar 7.0 across Phone, Handheld, 10-ft TV OS, Desktop, 8-module Control Center floating dock, Visual Preview-Card Settings, First-Run & Pairing Cinema, `#137` native-aspect relative resolutions, `#140` TV `(i)` info key & 4 presets, Modern Virtual Controller Studio `#151`, blur stability `#61`, resolution scaling `#144`/`#146`, canonical `input_glyphs.rs` preserving Moonlight V+ credits, `strings_70/` overflow).
   - Post 4-surface screenshots to Swarm Overview timeline -> `carbon-reviewer` approve -> retire Wave 0 author workers.
2. **Wave 1 (`03`, `04`, `05`, `06`, `22`, `27` -> `21`, `28`, `29` -> `07`) — Core Stream, Vibepollo Parity Core, Security, Multi-GPU, Desktop/NVENC & Android/Transport Pacing:**
   - Land `03` (`#131`), `04`, `05`, `06` (incl. 6.3.0 `sdr_white_nits` fix), `22` (Launch Warnings, DXGI/PDH `gpu_encoder_percent` for AMD+NVIDIA+Intel, Desktop `FrameLimiterStatus`, `&psmap=`), and `27` (WebRTC/CSRF/proxy/UNC security + anonymized support bundles) first; then `21` (depends on `04`, `06`: macOS direct VT/Metal, Linux Wayland DMA-BUF+syncobj/KMS/V4L2 P010 `#77`/Gamescope `#139`, Windows DirectFlip/D3D12/ARM64/`#120`, Host NVENC overhaul shrinking `nvenc.rs`), `28` (depends on `03`: hybrid/multi-GPU `idd-push` LUID + `#117` locked-PC `PowerRequestDisplayRequired` wake), and `29` (depends on `04`, `06`, `27`: high-Hz burst pacing + Android MediaCodec/ARR/Shield `#144`/`#7`/`#119`/`#116` + WebRTC TWCC/ICE/guest-IDR shield); then run serialized idle-gated `07` (`hw-verify`).
   - **Real Hardware vs Non-Hardware Honesty:** Measure Windows (RX 7900 XT, Radeon 780M), Intel Linux (`nuc` via `ssh nuc`), and real Android devices on release builds before recording in `rust/PERFORMANCE.md`. Verify NVIDIA NVENC & DXGI/PDH strictly against vendor headers/samples + fake-driver/mock tests, and macOS / Windows ARM64 strictly via CI — **NEVER claim NVIDIA, Apple Silicon, or untested SoC paths ran locally!**
   - **AMD Hang Halt Rule:** Exit code `3` from `e2e.py`, `check.ps1`, `wave1_verify.py`, or any soak runner triggers `.agents/GPU_HANG_HALT` — **STOP ALL HW TESTS IMMEDIATELY, investigate, NEVER retry!**
   - `carbon-reviewer` approve -> retire Wave 1 author workers.
3. **Wave 2 (`08`, `09`, `10`, `11`, `12`, `23`, `24`, `25`) — Everyday QoL & Vibepollo/USB Interop:**
   - Enforce Stage-1 lockstep `HostCapabilities` + `capabilities_contract.rs`, per-route 64 KiB JSON `ErrorBody` (`ErrorCode::TooLarge` / HTTP 413) for `09`, `#137` per-monitor recall + library sync fixes (`09`), `IMMNotificationClient` + Android BT A2DP/multichannel AAudio (`10`), `#142` transport + LUIDs + OEM 60 Hz in Stream Doctor (`11`), Reverse Landscape `#152` (`12`), unchunked Dolby RTSP `4096 + 64` B receive without `assemble_frames()` (`23`), loopback-only `127.0.0.1:3240` `usbipd-win` + ownership journal + Track 8B/8C arbitration (`24`), and Bluetooth DualSense 398 B HID speaker (verify SAxense/Nonary `b72d0c2` provenance) + strict `0x5500_0009` 20 B vs 40 B length disambiguation & mutual exclusion (`0x5533` / Foundation `0x10`) (`25`).
   - `carbon-reviewer` approve -> retire Wave 2 author workers.
4. **Wave 3 (`13`, `14`, `15`, `26`) — Power Features, Steam Controller 2026 & Remote Reach:**
   - `13` (Full Visual Console-Grade Controller Editor replacing scattered flat controller keys + `#150` + Client Input Deck on UMDF2 `rubylight-vgamepad`, zero unsigned KMDF), `14` (Coherence + Desktop CJK IME/macOS layout/touchpad/stylus), `15` (Away Wake + Traverse dual-stack IPv6/roaming/MTU), and `26` (`depends_on: ["13", "24"]`: physical Steam Controller 2026 `28de:1302`/`1304` direct HID + `0x40_0000` Switch 2 guard + `vhf_steam` 250 Hz motion & unplug counters).
   - `carbon-reviewer` approve -> retire Wave 3 author workers.
5. **Wave 4 (`16`, `17`) — Console-Quality Final Polish & Crash-Restore Resilience:**
   - `16` (4-surface multimodal screenshot polish + complete GPL-3.0 §7(e) host-API `"Nucleus Co-op"` scrub; post screenshots to Swarm Overview timeline), `17` (cold-start, `#129` `RecoveryJournalV2` soak, Installer `--repair` + `finish_qr.rs`, `vgamepad` service auto-repair, one-click Sunshine/Apollo/Vibepollo migration, cloud-save guard, Defender scan gate, debug symbols & release sync).
   - `carbon-reviewer` approve -> retire Wave 4 author workers.
6. **Wave 5 (`18`, `19`, `20`) — Showcase Demos, Websites & Docs 10/10:**
   - `18` (100% original procedural 7.0 stills/60 fps tours/trailers; post to Swarm Overview timeline), `19` (redesigned 7.0 websites reconciled with `fc1704feb3`, `< 6 MB` Nightjar budget, zero `sync-site-tokens.mjs`), `20` (READMEs/DESIGN.md/docs/code-maps/credits/`notes.d` anchored to `v6.3.0..HEAD`, preserving Moonlight V+, SAxense & Nonary credits strictly in code comments/`NOTICE`/`THIRD_PARTY.md`/README).
   - `carbon-reviewer` approve -> retire Wave 5 author workers.
7. **Wave 6 — Fail-Closed Release 7.0.0 Preparation & Maintainer Approval:**
   - Bump `7.0.0` (`versionCode >= 406`), Play `<= 500`-char changelog, filled `**Tested.**`, full `rust\build.ps1` under `build_lock.py`, green CI (`rust-windows.yml`, `nightjar-android.yml`, `nightjar-macos.yml`), and whole-release `carbon-reviewer` approval (`v6.3.0..HEAD` + release notes).
   - **STOP AND ASK THE MAINTAINER FOR EXPLICIT GO** before merging `main`, pushing tag `v7.0.0` (on `RamazanKara/Rubylight` and exact `v7.0.0` on mirrored `RamazanKara/nightjar` per `client/.github/workflows/release.yml`), publishing GitHub Releases, or deploying `gh-pages`. Never create `.agents/RELEASE_7_0_AUTHORIZED` yourself.
```

---

## 4. Complete Inventory of Files Prepared on `roadmap-prep`

Every file below is **brand-new** (ZERO modifications to existing tracked files on `origin/main` `b3d0c26b9b`), uses **LF** line endings, and was audited across four strictly read-only Carbon (`gemini-4-carbon-high`) review rounds:

- **`.agents/` (14 files):** `hooks.json`, `hooks/{guard_commands,watch_gpu_hang,test_hooks,__init__}.py`, `schemas/wave_review.json`, `skills/{rubylight-feature-spec,rubylight-verify,nightjar-parity,visual-ux-review}/SKILL.md`, `workflows/{orchestrator_core,roadmap_orchestrator,test_orchestrator,__init__}.py` (29-item DAG, dynamic post-sprint Phase 2 discovery, 22/22 unit tests PASS).
- **`docs/roadmap-state.json`:** Initialized 29-item Wave 0–6 state table (exact match with `orchestrator_core.initial_state()`).
- **`docs/designs/00-protocol-reservations.md`:** Master wire allocation table (`0x5548..=0x554F`, `0x5500_0081`, all 11 Native API v1 features, 64 KiB JSON `ErrorBody` contract, `HostCapabilities` table, plus §8 Vibepollo contracts `0x5602`, `0x5500_0009` 20 B vs 40 B, `x-ml-general.featureFlags & 0x08`, `x-ss-audio-codec-*`, `0x5500_0008`, `0x5504`/`0x5505`).
- **`docs/designs/01`–`29` (29 engineering specs):**
  - Wave 0: `01-ui-rubylight-studio.md`, `02-ui-nightjar-glass.md`
  - Wave 1: `03-launch-curtain.md`, `04-frame-warp-2.md`, `05-stream-dvr.md`, `06-hdr-sdr-tonemap.md`, `21-desktop-perf-and-nvenc.md`, `22-vibepollo-parity-core.md`, `27-security-and-privacy-hardening.md`, `28-host-capture-gpu-and-power.md`, `29-android-pacing-and-transport.md`, `07-hw-verify.md`
  - Wave 2: `08-quick-suspend.md`, `09-host-hub-sync.md`, `10-couch-party-audio.md`, `11-stream-doctor.md`, `12-phone-safe-area.md`, `23-vibepollo-dolby-rtsp.md`, `24-usb-forwarding-convenience.md`, `25-ps-speaker-pcm-and-bt.md`
  - Wave 3: `13-client-input-deck.md`, `14-coherence-workstation.md`, `15-away-wake-relay.md`, `26-steam-controller-2026.md`
  - Wave 4: `16-ui-final-polish.md`, `17-ux-craft-and-resilience.md`
  - Wave 5: `18-showcase-demos-and-videos.md`, `19-websites-10-of-10.md`, `20-readmes-and-docs-10-of-10.md`
- **`docs/reviews/gap-discovery-report.md`:** 6-scout blind-spot discovery & strict value-vs-bloat synthesis (Where Rubylight & Nightjar Lead + Rejected Competitor Anti-Patterns + Tier A/B/C + open-issue/6.3.0 coverage).
- **`docs/reviews/prep-carbon-spec-review.md`:** Full multi-round strictly read-only Carbon (`gemini-4-carbon-high`) audit record.
- **`docs/TEAMWORK-HANDOVER.md`:** This canonical `/swarm` handover.

---

## 5. Post-Sprint Delta Checklist (Run Once After Rebasing onto Post-Sprint `main`)

1. Dynamically re-verify every `[verify post-sprint code-map.md]` / `[resolve post-sprint ...]` path against freshly rebased `docs/code-map.md`, `client/docs/code-map.md`, `tools/dev.ps1`, and `tools/size_ratchet.json` before assigning slices.
2. Check `client/app/build.gradle` `versionCode` — ensure Wave 6 sets `versionCode = max(current_main_versionCode + 1, 406)`.
3. Preserve all pending contributor credits in `rust/release/notes.d/` and `client/notes.d/` (`Thanks to **Name** (@login, #N).`).
4. Confirm `python tools/arch_check.py` and `python tools/code_map.py check` pass cleanly at baseline (`roadmap/redesign-and-waves-1-5`).
5. Register every new 7.0 Desktop `ui/*.rs` / `stream/*.rs` calling `tr`/`trf`/`en` inside `SOURCES` (or `MARKED_ONLY`) in `client/protocol/desktop/src/strings/tests.rs` so the 14-locale `strings_70/<code>.rs` completeness test covers every new Desktop string automatically.
