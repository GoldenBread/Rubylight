# Rubylight 7.0.0 & Nightjar 7.0.0 — Canonical Claude Code Coordinator Handover

`C:\src\rl-roadmap-prep` (`roadmap-prep`) is **already cleanly rebased onto post-refactoring `origin/main` (`955ef39598`, `v6.4.1`, ready for `6.4.2`)**, reconciled against every bugfix, performance pass, USB/audio upgrade, and architecture split shipped in **`6.3.1`–`6.4.1`**, and wired natively for **Claude Code (`2.1.330+`)** via **`.claude/settings.json`** (safety hooks) and **`.claude/agents/*.md`** (**Opus 5.5** `xhigh`/`high`/`medium`/`low` + **Sonnet latest** + **Haiku latest**).

Open a **brand-new Claude Code session** and paste **Section 3** below to execute **Waves 0–6** (all **29 items**, with already-shipped `6.3.1`–`6.4.1` work cleanly pruned/narrowed so workers never redo finished work) and ship **Rubylight 7.0.0** and **Nightjar 7.0.0**.

---

## 1. Quick Start & Optional Sync (Run Once in PowerShell)

```powershell
# 1. Optional: if v6.4.2 or newer commits landed on origin/main since 2026-11-25, sync & rebase (0 conflicts!)
git -C C:\src\rubylight-rel fetch origin --tags
git -C C:\src\rl-roadmap-prep rebase origin/main

# 2. Create the live Wave 0-6 integration branch from roadmap-prep
git -C C:\src\rl-roadmap-prep checkout -b roadmap/redesign-and-waves-1-5

# 3. Fast 1-second sanity check (Claude Code hooks, guardrails & orchestrator unit tests)
python -B -m unittest discover -s C:\src\rl-roadmap-prep\.agents -p "test_*.py" -v
```

> **Zero Required Environment Variables:**
> - **Hardware Gate (Waves 1–4):** Idle-gated serialized HW/GPU checks are **allowed by default** (with mandatory installed-host idle check `RustHostSessionCount == 0 && RustHostPendingSessionCount == 0` + immediate exit-code-3 AMD hang halt via `.agents/GPU_HANG_HALT`, propagated across `.claude/worktrees/*` and the main checkout). Opt out at any time via `$env:RUBYLIGHT_DISABLE_HW_GATE = "1"`.
> - **Release Publish Gate (Wave 6):** Pushing `main`/`gh-pages`/tags or running `gh release create` is hard-gated by `.claude/settings.json` -> `.agents/hooks/guard_commands.py` on `.agents/RELEASE_7_0_AUTHORIZED`, which agents are blocked from creating themselves. Create/confirm it ONLY after reviewing `docs/reviews/release-7.0.0-review.md` and giving explicit maintainer GO.

---

## 2. Launch the Fresh Claude Code Session

```powershell
cd C:\src\rl-roadmap-prep
claude --model opus --effort high --add-dir C:\src --dangerously-skip-permissions
```

*(`--model opus` resolves to **Opus 5.5** (`claude-opus-5-5`). Inside the session, paste Section 3 below. Project hooks in `.claude/settings.json` and the 6 specialized subagents in `.claude/agents/*.md` load automatically.)*

---

## 3. Ready-to-Paste Claude Code Coordinator Launch Prompt

```text
You are the Roadmap Coordinator running on **Opus 5.5 (`--model opus --effort high`)** in `C:\src\rl-roadmap-prep` on integration branch `roadmap/redesign-and-waves-1-5` (rebased onto post-refactoring `origin/main` `v6.4.1` / `6.4.2`) executing Waves 0–6 (all 29 items `01`–`29` tracked in `docs/roadmap-state.json`) to ship **Rubylight 7.0.0** and **Nightjar 7.0.0**.

### First Turn Orientation (Post-Refactoring Split-Crate Architecture)
1. Read `AGENTS.md`, `client/AGENTS.md`, `docs/code-map.md`, `client/docs/code-map.md`, `tools/size_ratchet.json` (now down to ONLY 18 grandfathered files!), `docs/roadmap-state.json`, `docs/designs/00-protocol-reservations.md`, and `docs/reviews/gap-discovery-report.md` (especially the **2026-11-25 Post-6.4.1 / Post-Refactoring Reconciliation** and **Where Rubylight & Nightjar Already Lead**).
2. **Completed Post-Refactoring Architecture & Official Tooling (USE THESE!):**
   - **Host split crates:** `rust/core/{crypto,config,media-policy,input-policy,session}` (declarative `rust/core/config/settings.toml` -> `python tools/generated_docs.py`), `rust/windows/{abi,graphics,display,io}` + `rust/probes/`, `rust/host/{stream,webrtc,web}` + thin `butterpollo` + `tests/host/` + `tests/interop/`.
   - **Client split crates (`client/protocol/`, Edition 2024, MSRV 1.94):** `core/`, `wire/`, `native-api/` (moved here from `rust/core/`!), `traverse/`, `policy/` (including `http/capabilities.rs` & declarative `settings.toml` -> `python client/scripts/generate-settings.py`), `client/`, `desktop-config/` (`config.rs`, `strings/*.rs`, new `strings_70/*.rs`), `desktop-ui/` (`ui/`, `shot.rs`, `theme.rs`, `glyphs.rs`, `ui/design_tokens.rs`), `desktop-media/` (`stream/`, `media.rs`, `vulkan_video/`, `kms/`, `metal/`, `gamepad/`, `usb/`), thin `desktop/`, and `android/` (pure-JDK `Nightjar*Jni.java` + `buildHostNightjarCore`).
   - **Workflow Skills & Tools (read directly from `.agents/skills/*/SKILL.md`):** Pair our 4 roadmap skills (`rubylight-feature-spec`, `rubylight-verify`, `nightjar-parity`, `visual-ux-review`) with `main`'s 6 official skills (`add-control-message`, `add-native-endpoint`, `add-setting`, `check-nvenc`, `isolated-e2e`, `release-prep`), `python tools/scaffold.py`, `python tools/i18n.py` (with `tools/i18n_glossary.json`), and `python tools/build_lock.py --prune`.
   - **Size-ratchet reality:** Every monolithic source file (`nvenc.rs`, `web.rs`, `stream/control.rs`, `MediaCodecDecoderRenderer.java`, `ControllerHandler.java`, `StreamSettings.java`, `Game.java`, `ui/settings.rs`, etc.) is ALREADY decomposed `<= 800` LOC! Only 18 files remain in `tools/size_ratchet.json` (the 14 `client/protocol/desktop-config/src/strings/*.rs` tables at 1,750 LOC + 4 narrow host/service/driver files). Keep ALL new/edited source files `<= 800` LOC with `//!`/`/**` headers, and put ALL new 7.0 Desktop translations in `client/protocol/desktop-config/src/strings_70/<code>.rs` (zero lines added to `strings/*.rs`!).
   - **Test, hook & disk hygiene (`AGENTS.md` + `.claude/settings.json`):** Always bind unit tests/probes to loopback (`127.0.0.1`/`[::1]`) or IP-helper tables (never `0.0.0.0`/`[::]` triggering Defender Firewall prompts); never leave private per-worktree `target/` or `client/app/build/` caches behind (`tools/build_lock.py --prune`); respect `.claude/settings.json` hooks (never edit `.claude/settings*.json`, `.claude/agents/*`, `.agents/hooks*`, sentinels, or ratchet baselines; never run `rust\build.ps1` with `--lane`; never touch `C:\ProgramData\Butterpollo` or `keystore.properties`; LF line endings).
3. **Do NOT Redo Already-Shipped `6.3.1`–`6.4.1` Fixes (Preserve & Build Cleanly On Top!):**
   - Already landed on `origin/main` (verify, do not duplicate or regress): `6.3.2` NVENC 8-bit 4:4:4 AYUV D3D12 compute (`#150`) & D3D12 teardown drain (`#148`), `#145` HDR recovery/debounce/armed-switch & `run/encode.rs` `RgbaF16` `sdr_white_nits`, `#138` VHF virtual gamepad devnode recovery, `#147` PyroWave eager warmup, `#146` `source_fps` & virtual-display warnings, `#154` Windows D3D11VA decode-device isolation + auto-reconnect, `#153` MediaTek/full-range Vulkan; `6.3.3` host/wire/client/desktop/Android hot-path perf (idle repeat reuse, event-driven wakeups, async CUDA 10-bit 4:4:4, RTX HDR/Video shared fences, MetalFX ring pool, VT IOSurface cache, Linux phase-lock + EGL no-wait + gamescope VRR `#175`); `6.4.1` Android 5.1/7.1/7.1.4 AAudio (`#156`), `#119` Android/host fractional FPS & Vulkan scanout phase, `#168` pinned phone refresh recovery, `#152` `StreamOrientation.REVERSE_LANDSCAPE = 4`, `#150` Steam trackpad touch pressure, `#171` 0% OSC opacity, `#140` TV `(i)` help & per-device `stream_audio`/`audio_passthrough`/`audio_sink`, `#137` desktop Native 75%/50%, `#167` windowed launcher, `#179` desktop idle CPU/power, `#164` CGNAT/Tailscale multi-endpoint, `#129` monitor source-ID persistence, inbox WinUSB + async libusb iso USB forwarding, Android Track 8C USB controller arbitration, and Track H4 RTSS/setup/neural security hardening.

### Canonical Claude Code Subagent Roster & 3-Worker Build Capacity (`.claude/agents/*.md`)
Orchestrate work using Claude Code's native **`Agent` tool** (spawning up to **3 concurrent `isolation: worktree` build workers** in parallel via `run_in_background: true` for independent items/slices):

- **Coordinator (you — Opus 5.5, `--effort high`):** Owns Wave/item DAG scheduling, `docs/roadmap-state.json` live resumable state updates, pre-slice path coordination, clean merges of `wip/roadmap/<id>-<slug>` branches onto `roadmap/redesign-and-waves-1-5`, worktree cleanup, and Wave 6 release gating. (Step effort down for trivial git/CI bookkeeping or up for complex merges as needed.)
- **`core-builder` (`.claude/agents/core-builder.md` — Opus 5.5, `effort: xhigh`, `isolation: worktree`):** Hard-core / redesign / perf / engine / security / HID / interop items (`01`, `02`, `04`–`06`, `13`, `14`, `17`, `21`, `23`–`29`) and complex multi-crate fixes.
- **`slice-builder` (`.claude/agents/slice-builder.md` — Opus 5.5, `effort: high`, `isolation: worktree`):** Standard cross-layer feature slices & verification (`03`, `08`–`12`, `15`, `16`, `22`), Wave 1 serialized HW verification (`07`), and Wave 6 release preparation.
- **`medium-builder` (`.claude/agents/medium-builder.md` — Opus 5.5, `effort: medium`, `isolation: worktree`):** Straightforward feature slices, showcase demos & websites (`18`, `19`), and targeted non-architectural review fix passes.
- **`polish-l10n` (`.claude/agents/polish-l10n.md` — Sonnet latest, `effort: medium`, `isolation: worktree`):** Mechanical 15-locale Nightjar (`client/protocol/desktop-config/src/strings_70/<code>.rs` + Android `values-*/strings.xml` via `python tools/i18n.py`, adding ZERO lines to grandfathered `strings/*.rs`!) & 22-locale host translations, declarative `settings.toml` codegen (`tools/generated_docs.py` & `client/scripts/generate-settings.py`), code maps (`docs/code-map.md`, `client/docs/code-map.md`), READMEs/docs (`20`), and `rust/release/notes.d/` + `client/notes.d/` fragments. Pass `model: "sonnet"` explicitly if user env overrides default subagent models.
- **`fast-scout` (`.claude/agents/fast-scout.md` — Haiku latest, `effort: low`, strictly read-only):** Fast path lookups against `docs/code-map.md` / `client/docs/code-map.md`, CI status & timing log extraction (`gh run list` / `gh run view`), file-conflict checks, and mechanical grep audits. Pass `model: "haiku"` explicitly if needed.
- **`wave-reviewer` (`.claude/agents/wave-reviewer.md` — Opus 5.5, `effort: xhigh`, strictly read-only):** Invoke after **EVERY wave (Waves 0–5)** and for the **mandatory whole-release review (`v6.4.1..HEAD` [or latest `v6.4.*` tag] + release notes) in Wave 6**. Audits the exact commit range strictly read-only (verify `git rev-parse HEAD` & `git status --porcelain -uall` unchanged before/after!), returns JSON matching `.agents/schemas/wave_review.json`, and writes the rendered review report (`docs/reviews/wave-{w}-review.md` or `docs/reviews/release-7.0.0-review.md` saved by Coordinator) with up to 2 fix/re-verify/re-review cycles per wave.

**3-Worker Build Capacity (2 Local Desktop Lanes + 1 Remote Laptop Verification Lane):**
- Keep at most **2** concurrent local desktop builds under `tools/build_lock.py`.
- When both local desktop lanes are busy, assign the **3rd** concurrent worker to push its `wip/roadmap/<id>-<slug>` branch (`git push -u origin <branch>`) and run `pwsh tools\remote-check.ps1 -Host laptop -Branch <branch>` (`ramaz@192.168.4.12`, honoring `BUILD-LOCK`/`LAPTOP-LOCK`). **Automatic graceful fallback:** if SSH fails or `remote-check.ps1` exits `4` (busy) or `5` (nobody at console), fall back transparently to local `tools\dev.ps1 check -Fast -Base <base>` keeping local desktop concurrency capped at `2`.
- After merging each item's `wip/roadmap/<id>-<slug>` onto `roadmap/redesign-and-waves-1-5`, prune temporary sub-branches/worktrees promptly and run `python tools/build_lock.py --prune` between waves if disk space gets tight.

### Wave Execution Protocol (All 29 Items, Resumable via `docs/roadmap-state.json`)
Follow `.agents/skills/*`, `docs/designs/00-protocol-reservations.md`, and `docs/designs/01-*.md`..`29-*.md` (honoring every 2026-11-25 Post-6.4.1 Rebase Addendum):

1. **Wave 0 (`01`, `02` via `core-builder`) — Ground-Up 7.0 Console-Quality Complete Redesign:**
   - Unshackled from pre-7.0 `rust/web/DESIGN.md` and legacy "Moonlit Glass": ship flagship **Obsidian Studio** (`01`: all 5 host surfaces + remaining `#140` per-device override cards via declarative `settings.toml` & `OverrideCascadeCards.svelte`) and **Obsidian Cinema** (`02`: Phone, Handheld, 10-ft TV OS, Desktop, 8-module Control Center floating dock, Visual Preview-Card Settings, First-Run & Pairing Cinema, unified Android+Desktop `nightjar-policy::resolution_scale` `100/85/75/67/50%` `#137`, dynamic `(i)` glyph pill & 4 presets `#140`, compact overlay battery + clock `#191`, Modern Virtual Controller Studio `#151`, blur stability `#61`, resolution scaling `#144`/`#146`, canonical `input_glyphs.rs` preserving Moonlight V+ credits, `desktop-config/src/strings_70/`).
   - Inspect 4-surface rendered screenshots directly with `Read` -> `wave-reviewer` (`opus` `xhigh`) approve -> save `docs/reviews/wave-0-review.md` & update `docs/roadmap-state.json`.
2. **Wave 1 (`03`, `04`, `05`, `06`, `22`, `27` -> `21`, `28`, `29` -> `07`) — Core Stream, Vibepollo Parity Core, Security, Multi-GPU, Desktop/NVENC & High-Hz/WebRTC/Android Pacing:**
   - Land `03` (`slice-builder`, `#131` silent launchers + curtain), `04` (`core-builder`, built cleanly on `12e7349b3b` reset), `05` (`core-builder`), `06` (`core-builder`: BT.2390 shader, `/stream/settings`, `Rgba10Pq` `set_luminance`, `rubylight_vdd.rs` non-zero mastering values), `22` (`slice-builder`: Launch Warnings, DXGI/PDH `gpu_encoder_percent` for AMD+NVIDIA+Intel, Desktop `FrameLimiterStatus`, `&psmap=`), and `27` (`core-builder`: all 5 unfixed pillars — WebRTC Origin/ws://-secret/MAC, proxy-aware `browser_on_this_pc` + `/api/auth/refresh` CSRF + token least privilege, IPv6 `/64` pairing + `/pyrowave-bandwidth-probe` guard, UNC rejection, anonymized Host+Desktop support bundles) first;
   - Then `21` (`core-builder`, depends on `04`, `06`, narrowed to unbuilt post-6.4.1 scope: macOS direct `VTDecompressionSession` + `AVSampleBufferDisplayLayer`/`CAMetalLayer` + async MetalFX + `CADisplayLink` phase-lock + `self_update.rs`/`update_check.rs` macOS fixes; Linux Wayland `wl_subsurface`+syncobj [off under gamescope], KMS atomic FB LRU, V4L2 M2M `P010` `#77` + `aarch64` Flatpak CI + open NJ `#181` gamescope ~60% fps fix; Windows `IDXGISwapChain3` DirectFlip/Waitable/MPO + D3D12 Video Decode + Win ARM64 + `#120`; Host NVENC remaining D3D12 pre-registered async ring/VBV/AQ/split-encode/LTR-RFI/zero-rebuild RTX swap + AMF `Av1RateControlSkipFrameEnable = false` — using `.agents/skills/check-nvenc/SKILL.md`), `28` (`core-builder`, depends on `03`: `encoder_gpu.rs` single-vendor discrete ranking, `idd_push` `status.render_luid` coherence + VDD TDR re-resolution, `PowerRequestDisplayRequired` + `GUID_CONSOLE_DISPLAY_STATE` + physical+virtual cold-launch wake `#117`), and `29` (`core-builder`, depends on `04`, `06`, `27`, narrowed to unbuilt scope: Host High-Hz `batch_payload_limit`/`BURST_US` scaling & soft-start `#144`, Android Shield `#7` reconnect zero-debt + `experimentalChipsetTuning` retirement behind `VendorKeyHangs`, and WebRTC `set_fb_transport_cc(true)` BWE + 12 s ICE restart + trickle `Candidate` + guest IDR shield);
   - Then run serialized idle-gated `07` (`slice-builder`: `hw-verify`, anchored to `v6.4.1` baseline via `.agents/skills/isolated-e2e/SKILL.md`).
   - **Real Hardware vs Non-Hardware Honesty:** Measure Windows (RX 7900 XT, Radeon 780M), Intel Linux (`nuc` via `ssh nuc`), and real Android devices on release builds before recording in `rust/PERFORMANCE.md`. Verify NVIDIA NVENC & DXGI/PDH strictly against `rust/windows/abi/include/nvEncodeAPI.h` + samples + fake-driver/mock tests, and macOS / Windows ARM64 strictly via CI — **NEVER claim NVIDIA, Apple Silicon, or untested SoC paths ran locally!**
   - **AMD Hang Halt Rule:** Exit code `3` from `e2e.py`, `check.ps1`, `wave1_verify.py`, or any soak runner triggers `.agents/GPU_HANG_HALT` — **STOP ALL HW TESTS IMMEDIATELY, investigate, NEVER retry!**
   - `wave-reviewer` (`opus` `xhigh`) approve -> save `docs/reviews/wave-1-review.md` & update `docs/roadmap-state.json`.
3. **Wave 2 (`08`, `09`, `10`, `11`, `12`, `23`, `24`, `25`) — Everyday QoL & Vibepollo/USB Interop:**
   - Enforce Stage-1 lockstep `HostCapabilities` (`client/protocol/policy/src/http/capabilities.rs` + Java + `capabilities_contract.rs`), per-route 64 KiB JSON `ErrorBody` (`ErrorCode::TooLarge` / HTTP 413) for `09` (`slice-builder`), `#137`/`#163` per-display auto-profile recall + library sync fixes on unified Rust `console` (`09`), `IMMNotificationClient` hot-plug + Android classic BT A2DP `TYPE_BUILTIN_MIC` guard + API 33 `getDirectPlaybackSupport` (`10`, `slice-builder`, keeping 6.4.1's `#156` surround AAudio intact), `#142`/LUID/OEM-60Hz Stream Doctor (`11`, `slice-builder`), Safe-Fit + `LANDSCAPE_FLIP = 5` (atop 6.4.1's `REVERSE_LANDSCAPE = 4`) + Flip 180° + `#190` manual display centering (`12`, `slice-builder`), unchunked Dolby RTSP `4096 + 64` B receive without `assemble_frames()` (`23`, `core-builder`), **Desktop-only** USB convenience: loopback-only `127.0.0.1:3240` `usbipd-win` + ownership journal, Linux `EACCES` -> `usb_linux` fallback, and Desktop `usb_arbitration.rs` (`24`, `core-builder`, Android Track 8C & inbox WinUSB iso already shipped in 6.4.1!), and Bluetooth DualSense 398 B HID speaker + strict `0x5500_0009` 20 B vs 40 B disambiguation & mutual exclusion (`25`, `core-builder`).
   - `wave-reviewer` (`opus` `xhigh`) approve -> save `docs/reviews/wave-2-review.md`.
4. **Wave 3 (`13`, `14`, `15`, `26`) — Power Features, Steam Controller 2026 & Remote Reach:**
   - `13` (`core-builder`: Full Visual Console-Grade Controller Editor replacing scattered flat controller keys — naturally covering RL `#154` per-controller auto-switching OSC layouts & NJ `#172` touch activation area + Client Input Deck on UMDF2 `rubylight-vgamepad`, zero unsigned KMDF, preserving 6.4.1 `#150` trackpad touch pressure), `14` (`core-builder`: Coherence + Desktop CJK IME/macOS layout/touchpad/stylus), `15` (`slice-builder`: Away Wake + Traverse dual-stack IPv6/roaming/MTU atop 6.4.1 `#164`), and `26` (`core-builder`, `depends_on: ["13", "24"]`: physical Steam Controller 2026 `28de:1302`/`1304` direct HID + `0x40_0000` Switch 2 guard + `vhf_steam` 250 Hz motion & unplug counters).
   - `wave-reviewer` (`opus` `xhigh`) approve -> save `docs/reviews/wave-3-review.md`.
5. **Wave 4 (`16`, `17`) — Console-Quality Final Polish & Crash-Restore Resilience:**
   - `16` (`slice-builder`: 4-surface multimodal screenshot polish via `Read` + complete GPL-3.0 §7(e) host-API `"Nucleus Co-op"` scrub), `17` (`core-builder`: cold-start, remaining `#129` `RecoveryJournalV2` + `dpi_settle` 10-cycle soak, Installer `--repair` + `finish_qr.rs`, `vgamepad` service auto-repair, stuck `"installing"` updater fix, one-click Sunshine/Apollo/Vibepollo migration, cloud-save guard, Defender scan gate, debug symbols & release sync, extending `tools/i18n.py` instead of duplicating it).
   - `wave-reviewer` (`opus` `xhigh`) approve -> save `docs/reviews/wave-4-review.md`.
6. **Wave 5 (`18`, `19`, `20`) — Showcase Demos, Websites & Docs 10/10:**
   - `18` (`medium-builder`: 100% original procedural 7.0 stills/60 fps tours/trailers; visually verify with `Read`), `19` (`medium-builder`: redesigned 7.0 websites reconciled with `fc1704feb3`, `< 6 MB` Nightjar budget, zero `sync-site-tokens.mjs`; inspect 360/768/1440 px Light+Dark screenshots), `20` (`polish-l10n`: READMEs/DESIGN.md/docs/code-maps/credits/`notes.d` anchored to `v6.4.1..HEAD` [or latest `v6.4.*` tag], preserving Moonlight V+, SAxense & Nonary credits strictly in code comments/`NOTICE`/`THIRD_PARTY.md`/README).
   - `wave-reviewer` (`opus` `xhigh`) approve -> save `docs/reviews/wave-5-review.md`.
7. **Wave 6 — Fail-Closed Release 7.0.0 Preparation & Maintainer Approval:**
   - Follow `.agents/skills/release-prep/SKILL.md` (`slice-builder`): bump `7.0.0` (`versionCode = max(current_main_versionCode + 1, 412)`), Play `<= 500`-char changelog, filled `**Tested.**`, full `python tools/build_lock.py -- pwsh rust/build.ps1`, green CI (`rust-windows.yml`, `nightjar-android.yml`, `nightjar-macos.yml`), and whole-release `wave-reviewer` (`opus` `xhigh`) approval (`v6.4.1..HEAD` [or latest `v6.4.*`] + release notes -> `docs/reviews/release-7.0.0-review.md`).
   - **STOP AND ASK THE MAINTAINER FOR EXPLICIT GO** before merging `main`, pushing tag `v7.0.0` (on `RamazanKara/Rubylight` and exact `v7.0.0` on mirrored `RamazanKara/nightjar` per `client/.github/workflows/release.yml`), publishing GitHub Releases, or deploying `gh-pages`. Never create `.agents/RELEASE_7_0_AUTHORIZED` yourself.
```

---

## 4. Complete Inventory of Files Prepared on `roadmap-prep`

Every file below is **brand-new** relative to `origin/main` (`955ef39598`, ZERO modifications to existing tracked files on `main` — including untouched `CLAUDE.md`), uses **LF** line endings, passes `claude doctor`, `python tools/arch_check.py`, `python tools/code_map.py check`, and all `.agents/` guardrail/orchestrator unit tests:

- **`.claude/` (7 native Claude Code project files):**
  - `settings.json` — wires `PreToolUse` (`Bash|PowerShell|Read|Write|Edit|MultiEdit|NotebookEdit` -> `.agents/hooks/guard_commands.py`) and `PostToolUse` (`Bash|PowerShell` -> `.agents/hooks/watch_gpu_hang.py`).
  - `agents/core-builder.md` (`opus`, `xhigh`, `isolation: worktree`)
  - `agents/slice-builder.md` (`opus`, `high`, `isolation: worktree`)
  - `agents/medium-builder.md` (`opus`, `medium`, `isolation: worktree`)
  - `agents/polish-l10n.md` (`sonnet`, `medium`, `isolation: worktree`)
  - `agents/fast-scout.md` (`haiku`, `low`, read-only)
  - `agents/wave-reviewer.md` (`opus`, `xhigh`, strictly read-only)
- **`.agents/` (14 roadmap files alongside `main`'s 6 official skills):** `hooks.json` (legacy compatibility), `hooks/{guard_commands,watch_gpu_hang,test_hooks,__init__}.py` (native Claude Code `hookSpecificOutput` + nested `.claude/worktrees/` sentinel propagation + Antigravity compatibility), `schemas/wave_review.json`, `skills/{rubylight-feature-spec,rubylight-verify,nightjar-parity,visual-ux-review}/SKILL.md`, `workflows/{orchestrator_core,roadmap_orchestrator,test_orchestrator,__init__}.py` (29-item DAG, `MIN_NEXT_ANDROID_VERSION_CODE = 412`, `claude --model opus --effort xhigh` review CLI builder).
- **`docs/roadmap-state.json`:** Initialized 29-item Wave 0–6 state table (exact match with `orchestrator_core.initial_state()`).
- **`docs/designs/00-protocol-reservations.md` & `docs/designs/01`–`29`:** Master wire allocation table + all 29 engineering specs reconciled to post-refactoring `v6.4.1`/`6.4.2`.
- **`docs/reviews/gap-discovery-report.md` & `docs/reviews/prep-carbon-spec-review.md`:** 6-scout blind-spot discovery + 2026-11-25 post-6.4.1 reconciliation + historical 4-round prep spec audit archive.
- **`docs/TEAMWORK-HANDOVER.md`:** This canonical Claude Code handover.

---

## 5. Launch-Time Hygiene Checklist

1. If `wip/v6.4.2` merged and tagged after 2026-11-25, run `git -C C:\src\rl-roadmap-prep rebase origin/main` before branching `roadmap/redesign-and-waves-1-5` — zero conflicts since `roadmap-prep` adds only `.claude/`, `.agents/`, and `docs/` files.
2. Ensure Wave 6 bumps `client/app/build.gradle` `versionCode = max(current_main_versionCode + 1, 412)` (since `6.4.1` is `410` and `6.4.2` uses `411`).
3. Preserve all pending contributor credits in `rust/release/notes.d/` and `client/notes.d/` (`Thanks to **Name** (@login, #N).`).
4. Confirm `python tools/i18n.py check`, `python tools/arch_check.py`, and `python tools/code_map.py check` pass cleanly at baseline (`roadmap/redesign-and-waves-1-5`).
5. Register every new 7.0 Desktop `desktop-ui`/`desktop-media`/`desktop` file calling `tr`/`trf`/`en` inside `client/protocol/desktop-config/src/strings/tests/sources.rs` so the 14-locale `desktop-config/src/strings_70/<code>.rs` completeness test covers every new Desktop string automatically.
