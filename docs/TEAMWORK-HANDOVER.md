# Rubylight 7.0.0 & Nightjar 7.0.0 — Fresh `/teamwork` Coordinator Handover

This handover lets you open a **brand-new `agy` session** right after the active refactoring sprint merges to `main`, rebase `roadmap-prep` with **zero merge conflicts**, and launch a **`/teamwork`** swarm that executes **Waves 0–6** end-to-end and ships **Rubylight 7.0.0** and **Nightjar 7.0.0**.

---

## 1. Post-Sprint Rebase & Preflight Commands (Run First in PowerShell)

Run these commands once the refactoring sprint is merged into `origin/main` (verify `tools/arch_check.py`, `tools/size_ratchet.json`, and `tools/dev.ps1 -Fast` exist on `origin/main`):

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
python -c "import pathlib, sys; sys.path.insert(0, r'C:\src\rl-roadmap-prep\.agents\workflows'); import orchestrator_core as c; errs = c.verify_post_sprint_prereqs(pathlib.Path(r'C:\src\rl-roadmap-prep')); print('PREREQS:', 'OK' if not errs else errs); sys.exit(1 if errs else 0)"
python -m unittest discover -s C:\src\rl-roadmap-prep\.agents -p "test_*.py" -v
```

> **Hardware Gate Note (Waves 1–4):** `.agents/hooks/guard_commands.py` intentionally blocks all GPU/soak runners (`e2e.py`, `check.ps1`, `wave1_verify.py`, `crash_restore_soak.py`, `tools/hw/*`, `--ignored`) unless the **parent shell** sets `$env:RUBYLIGHT_ALLOW_HW_GATE = "1"` (and `.agents/GPU_HANG_HALT` does not exist). Set that env var BEFORE launching `agy` only when you are ready for serialized idle-gated hardware verification (Windows RX 7900 XT desktop, Radeon 780M laptop, Intel Linux NUC, Android devices).
>
> **Release Publish Gate Note (Wave 6):** Pushing `main`/`gh-pages`/tags or running `gh release create` is hard-gated on `.agents/RELEASE_7_0_AUTHORIZED`, which agents are **blocked from creating themselves**. You create/confirm it ONLY after reviewing the whole-release Carbon report and giving explicit maintainer GO!

---

## 2. Launch the Fresh `/teamwork` Session

Open a fresh terminal in `C:\src\rl-roadmap-prep` (on branch `roadmap/redesign-and-waves-1-5`):

```powershell
cd C:\src\rl-roadmap-prep
# Optional: enable only when ready for serialized idle-gated HW tests in Waves 1-4:
# $env:RUBYLIGHT_ALLOW_HW_GATE = "1"
agy --model gemini-4-neon-xhigh --add-dir C:\src --dangerously-skip-permissions
```

Inside the fresh session, type **`/teamwork`** and paste the **Coordinator Launch Prompt** from Section 3 below!

---

## 3. Ready-to-Paste `/teamwork` Coordinator Launch Prompt

Copy everything inside the fenced block below and paste it directly into your fresh `/teamwork` session:

```text
You are the Coordinator (`gemini-4-neon-xhigh`) leading a `/teamwork` swarm in `C:\src\rl-roadmap-prep` on integration branch `roadmap/redesign-and-waves-1-5` (rebased onto post-sprint `origin/main`) to execute Waves 0–6 and ship **Rubylight 7.0.0** and **Nightjar 7.0.0**.

### First Turn Orientation (Read Before Spawning Workers)
1. Read `AGENTS.md`, `client/AGENTS.md`, `docs/code-map.md`, `client/docs/code-map.md`, `docs/roadmap-state.json`, `docs/designs/00-protocol-reservations.md`, and `docs/reviews/prep-carbon-spec-review.md`.
2. Verify `.agents/hooks.json` is active, confirm `verify_post_sprint_prereqs()` passes (`tools/arch_check.py`, `tools/size_ratchet.json`, `tools/dev.ps1 -Fast`), and verify `git status --short` is clean on `roadmap/redesign-and-waves-1-5`.
3. Never modify `.agents/hooks*`, `.agents/RELEASE_7_0_AUTHORIZED`, `.agents/GPU_HANG_HALT`, `tools/size_ratchet.json`, or `tools/arch_baselines.json`. Never bypass `tools/build_lock.py`, never touch `C:\ProgramData\Butterpollo` or `keystore.properties`, and use LF line endings on every file.

### Model Roster & Concurrency Discipline (Pin Exact Models via `swarm`)
- **Coordinator (you):** `gemini-4-neon-xhigh` — owns Wave/item scheduling, `docs/roadmap-state.json` live updates, post-sprint `[verify post-sprint code-map.md]` path reconciliation before Phase 3, clean merges onto `roadmap/redesign-and-waves-1-5`, and release gating.
- **Hard-Core / Redesign / Perf / Engine Workers (`gemini-4-neon-xhigh`, `workspace:"share"` or `tools\wt.ps1`):** Use for Wave 0 (`01`, `02` ground-up console-quality redesign), Wave 1 (`04` Frame Warp 2.0, `05` Stream DVR, `06` HDR-to-SDR tone-map, `21` macOS/Linux/Windows desktop perf + Host NVENC overhaul), Wave 3 (`13` Visual Console-Grade Controller Editor & Input Deck, `14` Coherence Workstation), and Wave 4 (`17` UX craft & #129 crash-restore soak).
- **Standard Feature Slice Workers (`gemini-4-neon-high`, `workspace:"share"`):** Use for well-scoped disjoint slices across Android Java, Desktop egui, Svelte Web Console, Host wiring, and unit tests (`03`, `08`, `09`, `10`, `11`, `12`, `15`, `16`, `18`, `19`).
- **Mechanical Polish & Localization Fan-Out (`gemini-4-hydrogen-high`, `workspace:"share"`):** Use for 15-locale Nightjar `client/protocol/desktop/src/strings_70/<code>.rs` + Android `values-*/strings.xml` translations (ZERO lines added to grandfathered 1,750-LOC `strings/*.rs`!), 22-locale host `rust/assets/locale/*.json` sync, `docs/code-map.md` / `client/docs/code-map.md` updates, and `rust/release/notes.d/` + `client/notes.d/` fragments (`20`).
- **Fast Log Triage & Extraction (`gemini-4-hydrogen-low`, `workspace:"inherit"`):** Use for read-only CI log summarization, `timing_report.py` / `result.json` extraction, and lightweight artifact triage.
- **`run_workflow` Tier Resolution (100% Gemini 4 — Zero Legacy 3.5/3.8 Models):** Inside `.agents/workflows/roadmap_orchestrator.py` (`agent(..., model=...)`), `"inherit"` resolves to the parent (`gemini-4-neon-xhigh`), `"pro"` resolves to Gemini 4 Hydrogen (XHigh), and `"flash"` resolves to Gemini 4 Hydrogen (High). **Never use `"flash_lite"`** (which resolves to legacy Gemini 3.5 Flash Lite).
- **Dedicated Independent Wave & Whole-Release Reviewer (`carbon-reviewer`, `gemini-4-carbon-high`, `workspace:"inherit"`, STRICTLY READ-ONLY):** Runs after **EVERY wave (Waves 0, 1, 2, 3, 4, 5)** and for the **mandatory whole-release review (`v6.x..HEAD`) in Wave 6**. Reviews the exact wave commit range against `AGENTS.md`, `client/AGENTS.md`, `00-protocol-reservations.md`, `tools/arch_check.py`, `tools/size_ratchet.json`, protocol/capability lockstep, UI/UX craft, and NVIDIA/macOS hardware-honesty rules. Returns JSON conforming to `.agents/schemas/wave_review.json`. Up to 2 fix/re-verify/re-review cycles per wave before quarantining or escalating.
- **Hard Concurrency Cap:** At most **2 concurrent Cargo/Gradle/npm build workers** at a time (respect `tools/build_lock.py` and `tools\dev.ps1 check -Fast -Base <base>`). Never run `rust\build.ps1` without `build_lock.py` or with `--lane`.

### Execution Protocol (All 21 Items Across Waves 0–6)
Follow `.agents/skills/{rubylight-feature-spec,rubylight-verify,nightjar-parity,visual-ux-review}/SKILL.md`, `docs/designs/00-protocol-reservations.md`, and all 21 item specs (`docs/designs/01-*.md` through `docs/designs/21-*.md`). You may also invoke `.agents/workflows/roadmap_orchestrator.py` (via `run_workflow`) for deterministic single-item or staged sub-pipelines and state updates.

1. **Wave 0 — Ground-Up 7.0 Console-Quality Complete Redesign (`01`, `02`):**
   - Completely unshackled from pre-7.0 `rust/web/DESIGN.md` and legacy "Moonlit Glass" layouts/palettes: ship flagship **Obsidian Studio** (Rubylight 7.0 across ALL 5 host surfaces: Web Console, Browser `/play` & `/guest`, Host-Drawn Couch Menu, Windows Installer + Instant QR Finish Screen via canonical `rust/setup/src/ui_theme.rs`, System Tray via canonical `rust/windows/src/tray/status.rs` + No-Script HTML parity) and **Obsidian Cinema** (Nightjar 7.0 across Phone, Handheld, 10-ft TV OS, Desktop, Console Control Center floating dock & 8 visual cards, Visual Preview-Card Settings, First-Run & Pairing Cinema, Modern Virtual Controller Studio `#151`, compositor/blur stability `#61`, resolution-aware scaling `#144`/`#146`, tactile soundscape/haptics contract, canonical `client/protocol/policy/src/input_glyphs.rs` preserving Moonlight V+ credits, and `strings_70/<code>.rs` overflow).
   - Gate Wave 0 with full multimodal screenshot review + `carbon-reviewer` (`gemini-4-carbon-high`) approval before starting Wave 1.
2. **Wave 1 — Core Stream, Capture & Desktop/NVENC Performance Excellence (`03`, `04`, `05`, `06` -> `21` -> `07`):**
   - Run `03`, `04`, `05`, `06` first; once `04` and `06` merge, run `21` (`desktop-perf-and-nvenc`: macOS direct VideoToolbox + `CVMetalTextureCache`/`AVSampleBufferDisplayLayer`, Linux Wayland DMA-BUF explicit sync + KMS atomic FB cache + V4L2 M2M, Windows DirectFlip/Waitable/MPO + safe D3D12 decode, Host NVIDIA NVENC D3D12 pre-registered async ring + ULL VBV/AQ + LTR-RFI/split-encode + pooled 4:4:4/RTX interop shrinking `nvenc.rs`); then run `07` (`hw-verify`) serially with strict idle gate!
   - **Real Hardware vs Non-Hardware Honesty:** Measure Windows (RX 7900 XT desktop, Radeon 780M laptop) and Intel Linux (`nuc` via `ssh nuc`) on real release builds before recording numbers in `rust/PERFORMANCE.md`. Verify NVIDIA NVENC strictly against vendored `rust/windows/include/nvEncodeAPI.h` (SDK 13.0) + samples + fake-driver tests, and macOS strictly via `nightjar-macos.yml` CI — **NEVER claim NVIDIA or Apple Silicon paths ran on hardware!**
   - **AMD Hang Halt Rule:** Exit code `3` from `e2e.py`, `check.ps1`, `wave1_verify.py`, or any soak script means an AMD encoder hang — `.agents/hooks/watch_gpu_hang.py` writes `.agents/GPU_HANG_HALT`. **STOP ALL HW TESTS IMMEDIATELY, investigate, and NEVER retry!**
   - Gate Wave 1 with `carbon-reviewer` approval before Wave 2.
3. **Wave 2 — Everyday QoL & Multi-Device (`08`, `09`, `10`, `11`, `12`):**
   - Respect lockstep Stage-1 `HostCapabilities` + `capabilities_contract.rs`, per-route 64 KiB body limit (`ErrorCode::TooLarge` / HTTP 413) for `09`, Reverse Landscape (`#152`) in `12`, and `carbon-reviewer` approval before Wave 3.
4. **Wave 3 — Power Features & Remote Reach (`13`, `14`, `15`):**
   - Item `13` delivers the flagship **Visual Console-Grade Controller Editor** (live interactive controller diagram for DualSense/Xbox/Switch/Steam/Handheld/Generic + Touch Studio `#151` tab, Sticks & Triggers curves, Gyroscope Studio, 4–12-slice Radial Menu Builder, Haptics & Lightbar, layered `Global -> Model -> Game` profiles synced via `09`, lossless migration retiring scattered flat controller keys verified by `SettingsKeysTest` + `config.rs::migrate`, zero growth to `ControllerHandler.java`/`StreamSettings.java`/`PadEditor.java`/`ui/settings.rs`) + Client Input Deck on first-party UMDF2 `rubylight-vgamepad` (ZERO unsigned KMDF!).
   - Gate Wave 3 with `carbon-reviewer` approval before Wave 4.
5. **Wave 4 — Console-Quality Final Polish & Crash-Restore Resilience (`16`, `17`):**
   - `16` runs the mandatory 4-surface multimodal screenshot & contrast polish pass across all 5 Rubylight 7.0 surfaces and all Nightjar 7.0 form factors (including the Visual Controller Editor, `#61`, `#144`/`#146`, `#151`, `#152`, 22 host + 15 Nightjar locales, zero `"Experimental"`, zero mod names).
   - `17` ships cold-start optimization, Installer `--repair`, canonical `RecoveryJournalV2` (`#129` 10-cycle crash-restore soak), plain-English diagnostics (preserving `ErrorBody { error: ApiError }` envelope), and calibrated UI soundscape/haptics.
   - Gate Wave 4 with `carbon-reviewer` approval before Wave 5.
6. **Wave 5 — Showcase Demos, Websites & Docs 10/10 (`18`, `19`, `20`):**
   - `18` builds 100% original procedural 7.0 showcase stills/60 fps tours/trailers (`covers.mjs`, `GenerateAssets.java`, `DEMO` badges, verbatim `rust/PERFORMANCE.md` citations).
   - `19` ships ground-up 7.0 websites (`site/` via `sync_site_tokens.py` + `client/site/` via `gen_design_tokens.py` `< 6 MB` non-video budget, zero `sync-site-tokens.mjs`).
   - `20` updates `README.md`, `client/README.md`, `rust/web/DESIGN.md`, `client/design/README.md`, all protocol/API/config/troubleshooting docs, code maps, `NOTICE`/`THIRD_PARTY.md` (preserving Moonlight V+ & upstream port credits), and `notes.d` fragments.
   - Gate Wave 5 with `carbon-reviewer` approval before Wave 6.
7. **Wave 6 — Fail-Closed Release 7.0.0 Preparation & Maintainer Approval:**
   - Bump Rubylight (`rust/release/bump.py` -> `7.0.0`) & Nightjar (`versionName "7.0.0"`, `versionCode >= 406`), write Play changelog (`<= 500` chars) & filled `**Tested.**` summary, run full `rust\build.ps1` (under `build_lock.py`), verify ALL green CI checks (`rust-windows.yml`, `nightjar-android.yml`, `nightjar-macos.yml`), and run `carbon-reviewer` on the **entire diff from the previous release tag to HEAD plus release notes**.
   - **STOP AND ASK THE MAINTAINER FOR EXPLICIT GO** before merging to `main`, pushing tag `v7.0.0` (on `RamazanKara/Rubylight`, and exact `v7.0.0` on mirrored `RamazanKara/nightjar` per `client/.github/workflows/release.yml`), publishing GitHub Releases, or deploying `gh-pages`! Never create `.agents/RELEASE_7_0_AUTHORIZED` yourself.
```

---

## 4. Complete Inventory of Files Prepared on `roadmap-prep`

Every file below is **brand-new** (ZERO modifications to existing tracked files on `origin/main` `b3d0c26b9b`), uses **LF** line endings, and was audited across three strictly read-only Carbon (`gemini-4-carbon-high`) review rounds:

### `.agents/` Guardrails, Schemas, Skills & Orchestrator
- `.agents/hooks.json` — `PreToolUse` (`run_command|write_file|edit_file|read_file`) + `PostToolUse` (`run_command`) hook bindings.
- `.agents/hooks/guard_commands.py` — 3-tier fail-closed command & file guardrail (hard-denies force push, tag mutation, `cargo fmt --all`, `C:\ProgramData\Butterpollo`, `keystore.properties`, `arch_check --write-baselines`, unlocked/piped `rust\build.ps1`, self-editing `.agents/RELEASE_7_0_AUTHORIZED`/hooks/ratchets; gates release commands on `RELEASE_7_0_AUTHORIZED`; gates HW/soak runners on parent env `RUBYLIGHT_ALLOW_HW_GATE=1` & absence of `GPU_HANG_HALT`).
- `.agents/hooks/watch_gpu_hang.py` — Watches all HW/soak runners (`e2e.py`, `check.ps1`, `*_soak.(ps1|py)`, `wave1_verify.py`, `focus_ab.py`, `tools/hw/*`), handles Antigravity's live `transcript.jsonl` schema (`step_index`/`content`), and writes `.agents/GPU_HANG_HALT` across active workspaces on exit code `3`.
- `.agents/hooks/test_hooks.py` — Unit tests verifying all guardrail & GPU-hang halt paths.
- `.agents/schemas/wave_review.json` — Strict JSON schema for Carbon wave & whole-release reviews.
- `.agents/skills/rubylight-feature-spec/SKILL.md` — Spec & post-sprint code-map reconciliation discipline.
- `.agents/skills/rubylight-verify/SKILL.md` — Tiered build/test (`dev.ps1 check -Fast -Base`, `arch_check.py`, `build_lock.py`, serialized HW gate, NVIDIA/macOS honesty).
- `.agents/skills/nightjar-parity/SKILL.md` — 7.0 Obsidian Cinema single-source tokens, `strings_70/<code>.rs` overflow, dynamic glyphs, 4-form-factor parity, zero stream-time blur.
- `.agents/skills/visual-ux-review/SKILL.md` — Ground-up 7.0 4-surface multimodal screenshot, contrast (`>= 4.5:1` / `>= 3.0:1`), and UX craft rubric.
- `.agents/workflows/orchestrator_core.py` & `roadmap_orchestrator.py` & `test_orchestrator.py` — Resumable, topologically-ordered 21-item Wave 0–6 workflow engine with dash-delimited sub-branches, staged parallel slice handoffs, strictly read-only Carbon CLI integration, and fail-closed Wave 6 release verification.

### `docs/` State, Architecture Specs (00–21), Review & Handover
- `docs/roadmap-state.json` — Initialized 21-item Wave 0–6 state table (exact match with `orchestrator_core.initial_state()`).
- `docs/designs/00-protocol-reservations.md` — Master wire allocation table (`RUBYLIGHT_7_CONTROL_IDS = 0x5548..=0x554F`, input magic `0x5500_0081`, all 11 Native API v1 feature strings, 64 KiB body-limit contract, `HostCapabilities` 1:1 table).
- **Wave 0 (Redesign):** `01-ui-rubylight-studio.md` (5 host surfaces), `02-ui-nightjar-glass.md` (all 4 Nightjar form factors, Control Center, Preview-Card Settings, Pairing Cinema, `#151`, `#61`, `#144`/`#146`, `input_glyphs.rs`, `strings_70/`).
- **Wave 1 (Core Stream & Perf):** `03-launch-curtain.md`, `04-frame-warp-2.md`, `05-stream-dvr.md`, `06-hdr-sdr-tonemap.md`, `21-desktop-perf-and-nvenc.md` (macOS/Linux/Windows desktop perf + Host NVENC overhaul), `07-hw-verify.md` (serialized ABBA HW verification on RX 7900 XT, Radeon 780M, and Intel Linux NUC).
- **Wave 2 (Everyday QoL):** `08-quick-suspend.md`, `09-host-hub-sync.md`, `10-couch-party-audio.md`, `11-stream-doctor.md`, `12-phone-safe-area.md` (`#152`).
- **Wave 3 (Power & Remote):** `13-client-input-deck.md` (Full Visual Console-Grade Controller Editor + Client Input Deck on first-party UMDF2 `rubylight-vgamepad`), `14-coherence-workstation.md`, `15-away-wake-relay.md`.
- **Wave 4 (Polish & Resilience):** `16-ui-final-polish.md`, `17-ux-craft-and-resilience.md` (`#129` `RecoveryJournalV2` soak, Installer `--repair`, plain-English errors).
- **Wave 5 (Showcase, Sites & Docs):** `18-showcase-demos-and-videos.md`, `19-websites-10-of-10.md`, `20-readmes-and-docs-10-of-10.md`.
- `docs/reviews/prep-carbon-spec-review.md` — Full multi-round strictly read-only Carbon (`gemini-4-carbon-high`) audit report and verification trail.
- `docs/TEAMWORK-HANDOVER.md` — This handover document.

---

## 5. Post-Sprint Delta Checklist (Run Once After Rebasing onto Post-Sprint `main`)

Before spawning Wave 0/1 slice implementers, the Coordinator checks these 5 fast items against post-sprint `origin/main`:
1. Re-verify every `[verify post-sprint code-map.md]` path in a design spec against the freshly rebased `docs/code-map.md` and `client/docs/code-map.md` before launching its slices.
2. Check `tools/size_ratchet.json` for any grandfathered files already shrunk or split by the sprint (treat any lower sprint ceiling as the new stricter ceiling).
3. Check `client/app/build.gradle` `versionCode` — ensure Wave 6 sets `versionCode = max(current_main_versionCode + 1, 406)`.
4. Check `rust/release/notes.d/` and `client/notes.d/` for any merged contributor PRs already pending release so Wave 5/6 keeps every bold contributor credit (`Thanks to **Name** (@login, #N).`).
5. Confirm `python tools/arch_check.py` and `python tools/code_map.py check` pass cleanly at baseline (`roadmap/redesign-and-waves-1-5`) before the first Wave 0 commit.
6. Whenever any Wave 0–4 Desktop UI/stream slice adds a new `ui/*.rs` or `stream/*.rs` file calling `tr`/`trf`/`en`, register it in `SOURCES` (or `MARKED_ONLY`) inside `client/protocol/desktop/src/strings/tests.rs` so the 14-locale `strings_70/<code>.rs` completeness test verifies every new 7.0 Desktop string automatically.
