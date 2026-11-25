# Independent Carbon (`gemini-4-carbon-high`) Roadmap Prep Spec Review

> **Historical Prep Audit Archive (2026-11-22):** Preserves the original 4-round pre-implementation spec audit. Live Waves 0–6 orchestration and post-wave / whole-release reviews were migrated on **2026-11-25** to **Claude Code** (`Opus 5.5` `xhigh`/`high`/`medium`/`low` + `Sonnet` + `Haiku`, with `.claude/agents/wave-reviewer.md` on Opus 5.5 `xhigh`); see `docs/TEAMWORK-HANDOVER.md`.

- **Reviewer Model:** `gemini-4-carbon-high` (strictly read-only CLI invocation via `agy --model gemini-4-carbon-high --output-format json --json-schema .agents/schemas/wave_review.json --print ...`)
- **Audited Worktree:** `C:\src\rl-roadmap-prep` (branch `roadmap-prep`, base `origin/main` `b3d0c26b9b`, compared read-only against sprint guardrails at `C:\src\rubylight-sprint\tools\{arch_check.py,size_ratchet.json,dev.ps1}`)
- **Final Verdict (Round 3):** **`approved: true` — `0` blocking findings (`GIT_UNCHANGED=True`, `CARBON_EXIT=0`)**

---

## Executive Summary

Across three rigorous, strictly read-only independent review rounds, Carbon (`gemini-4-carbon-high`) audited the complete Rubylight 7.0.0 & Nightjar 7.0.0 roadmap preparation tree:
- `.agents/` guardrails (`hooks.json`, `guard_commands.py`, `watch_gpu_hang.py`, `test_hooks.py`), schema (`wave_review.json`), skills (`rubylight-feature-spec`, `rubylight-verify`, `nightjar-parity`, `visual-ux-review`), and workflow orchestrator (`orchestrator_core.py`, `roadmap_orchestrator.py`, `test_orchestrator.py`);
- `docs/roadmap-state.json` (21-item Wave 0–6 DAG);
- `docs/designs/00-protocol-reservations.md` and all **21** engineering specs (`01-ui-rubylight-studio.md` through `21-desktop-perf-and-nvenc.md`), including the ground-up 7.0 Obsidian Studio & Obsidian Cinema Complete Redesign, Full Visual Console-Grade Controller Editor, and Desktop Client Performance & Host NVIDIA NVENC Low-Latency Overhaul;
- `docs/TEAMWORK-HANDOVER.md` (fresh-session `/teamwork` Coordinator handover using 100% Gemini 4 models).

Every single blocking finding from Round 1 (18 findings) and Round 2 (9 findings) was resolved, unit-tested, and independently re-verified in Round 3, which returned **`approved: true`** with **`0` blocking findings** and **5 minor non-blocking polish notes** (all 5 also reconciled immediately prior to commit).

---

## Audit Trail by Round

### Round 1 — Initial Comprehensive Audit (`approved: false`, 18 blocking findings — ALL RESOLVED)

Carbon caught 18 genuine security, orchestrator, protocol, and file-ratchet defects before a single line of production code was ever written:

1. **Hook bypass via file tools & self-authorization (Critical, `.agents/hooks.json` / `guard_commands.py`):** Widened `PreToolUse` matcher to `run_command|write_file|edit_file|read_file`; hard-denied agent writes/edits/shell mutations to `.agents/RELEASE_7_0_AUTHORIZED`, `.agents/GPU_HANG_HALT`, `.agents/hooks*`, `tools/size_ratchet.json`, `tools/arch_baselines.json`, `keystore.properties`, and `C:\ProgramData\Butterpollo` (allowing `ButterpolloRelease`).
2. **Command-guard bypass vectors (High, `.agents/hooks/guard_commands.py`):** Hard-denied `.exe` variants (`git.exe`/`cargo.exe`/`gh.exe`), bare `git push`, `gh api .../releases`, `gh workflow run -f publish=true|dry_run=false`, unlocked/piped/subexpression/`--lane`/`cd rust` `rust\build.ps1`, and required parent process env `RUBYLIGHT_ALLOW_HW_GATE=1` for all HW/soak runners.
3. **GPU hang watcher coverage & multi-worktree sentinel (High, `.agents/hooks/watch_gpu_hang.py`):** Expanded watched runners (`e2e.py`, `check.ps1`, `*_soak.(ps1|py)`, `wave1_verify.py`, `focus_ab.py`, `tools/hw/*`), added `transcript.jsonl` fallback, and replicated `GPU_HANG_HALT` across `workspacePaths` and `cwd`.
4. **Post-sprint prerequisite check & skill accuracy (High, `.agents/skills/rubylight-verify/SKILL.md` & `orchestrator_core.py`):** Added fail-closed `verify_post_sprint_prereqs()` asserting `tools/arch_check.py`, `tools/size_ratchet.json`, and `tools/dev.ps1 -Fast` exist before Wave 0 starts.
5. **Broken `workspace="share"` git handoffs & CI branch naming (Critical, `orchestrator_core.py`):** Switched item branches to `wip/roadmap/<id>-<slug>` (triggering `wip/**` CI) and added explicit sub-branch commits and merges across Phases 3–7.
6. **Carbon CLI invocation, background await & read-only enforcement (Critical, `orchestrator_core.py`):** Fixed `--model ... --print` ordering, added `run_command_and_await()` via `manage_task`, enforced before/after `git rev-parse HEAD` + `git status --porcelain -uall` invariance, and halted waves before Phase 8 if any item is unmerged/quarantined.
7. **Wave 6 fail-closed release gate (Critical, `orchestrator_core.py`):** Required explicit maintainer authorization (`authorize_publish=True`), wired deterministic `verify_release_artifacts()` (`Cargo.toml == "7.0.0"`, `build.gradle` `versionName == "7.0.0"` & `versionCode >= 406`, Play `<= 500`-char changelog, filled `**Tested.**`), and ensured `try/finally` sentinel cleanup.
8. **Grandfathered 1,750-LOC `strings/*.rs` ceiling (Critical, `02-ui-nightjar-glass.md` & Desktop specs):** Introduced canonical chained `client/protocol/desktop/src/strings_70/<code>.rs` (`<= 800` LOC each) so **zero lines** are added to grandfathered `strings/{de..zh_hant}.rs`.
9. **Native API `ErrorBody` envelope preservation (Critical, `17-ux-craft-and-resilience.md`):** Preserved `ErrorBody { error: ApiError { ... } }` instead of flattening fields at top level, keeping `API_VERSION = 1` additive compatibility intact.
10. **Native API routes, 64 KiB body limit & HTTP status accuracy (High, `03`, `06`, `09`, `13`, `14`, `15`):** Registered `stream.settings` + `video.sdr_tonemap` in `00`/`06`/`20`; fixed `03` HTTP status & `MoonBridge.java`; added per-route `DefaultBodyLimit::disable()` + `to_bytes(..., 64 * 1024)` returning `ErrorCode::TooLarge` (HTTP 413) in `09`/`13`; wired `schema/input.ts` and registration points in `13`/`14`/`15`.
11. **Cross-slice compile ordering, duplicate module names & Wave 5 token sync (High, `01`, `02`, `05`, `08`, `16`, `17`, `18`, `19`):** Deduplicated canonical modules (`rust/setup/src/ui_theme.rs`, `rust/windows/src/tray/status.rs`, `client/protocol/policy/src/input_glyphs.rs`, `RecoveryJournalV2`), removed `sync-site-tokens.mjs` in favor of sole `gen_design_tokens.py`, fixed `ui/design_tokens.rs` ownership between `16C`/`16D`, wired `ui/demo_70.rs` through `ui.rs`, and scrubbed touch hints in `08`.

### Round 2 — Deep Re-Audit (`approved: false`, 9 blocking findings — ALL RESOLVED)

1. **Live Antigravity `transcript.jsonl` schema & `crash_restore_soak.py` (Critical, `watch_gpu_hang.py`):** Updated parser to match Antigravity's actual `entry["step_index"]` + `entry["content"]` schema and added `crash_restore_soak.py` to watched runners (`test_hooks.py` verified).
2. **Git D/F ref collision & multi-slice stage handoffs (Critical, `orchestrator_core.py`):** Changed slash-delimited `{branch}/s*` sub-branches to dash-delimited `{branch}-s{stage}-{name}`, `{branch}-stage-{stage}`, `{branch}-p5-{kind}` and added intermediate stage merges whenever a non-final stage has `>= 2` slices.
3. **Phase 6 read-only lenses reading unmerged `{branch}` & `versionCode` floor (High, `orchestrator_core.py`):** Switched all 5 Phase 6 lenses to `type_name="self"` + `workspace="share"` strictly read-only on interpolated `{branch}`, and raised `MIN_NEXT_ANDROID_VERSION_CODE` to `406` (above `origin/main` `6.3.0`'s `405`).
4. **Lockstep `HostCapabilities` + `capabilities_contract.rs` Stage-1 ownership (High, `09`, `10`, `11`, `13`, `15`):** Aligned Rust `Feature`, Java `HostCapabilities.Feature`, and `capabilities_contract.rs` into Stage 1 across all five specs.
5. **Remaining `strings/*.rs` citations (Medium, `16-ui-final-polish.md` & `nightjar-parity/SKILL.md`):** Updated all references to canonical `strings_70/<code>.rs`.

### Round 3 — Final Comprehensive Audit (`approved: true`, `0` blocking findings)

Covered all Round 2 fixes PLUS all four major user product additions:
- **Ground-Up 7.0 Complete Redesign** (Obsidian Studio across all 5 Rubylight host surfaces + Obsidian Cinema across all 4 Nightjar form factors, Control Center floating dock & visual cards, Preview-Card Settings, Pairing Cinema, `#151`, `#61`, `#144`/`#146`, `input_glyphs.rs`, `strings_70/`);
- **Full Visual Console-Grade Controller Editor** (`02`, `09`, `13`, `16`, `18`, `19`, `20` — live diagram telemetry, Sticks & Triggers, Gyroscope Studio, 4–12-slice Radial Builder, Haptics & Lightbar, Touch `#151` tab, layered profiles, lossless flat-key migration);
- **Wave 1 Item `21` (`21-desktop-perf-and-nvenc.md`) & updated `07-hw-verify.md` / `00-protocol-reservations.md`** (macOS direct VideoToolbox + `CVMetalTextureCache`/`AVSampleBufferDisplayLayer`, Linux Wayland `wl_subsurface` DMA-BUF explicit sync + KMS atomic FB LRU cache + optional V4L2 M2M, Windows DirectFlip/Waitable/MPO + safe D3D12 decode, Host NVENC D3D12 pre-registered async ring + ULL VBV/AQ + LTR-RFI/split-encode + pooled 4:4:4/RTX shrinking `nvenc.rs` below `3711` LOC; strict Windows/Intel-NUC measured vs macOS-CI/NVENC-header honesty);
- **`docs/TEAMWORK-HANDOVER.md`** (100% Gemini 4 roster: `gemini-4-neon-xhigh`, `gemini-4-neon-high`, `gemini-4-hydrogen-high`, `gemini-4-hydrogen-low`, `gemini-4-carbon-high`; zero `flash_lite` / 3.5 / 3.8).

#### Verbatim Round 3 Carbon Summary

> Round 3 (Final Verification) of `C:\src\rl-roadmap-prep` passed with ZERO blocking findings. Verified: (1) all 9 Round 2 blocking fixes across `.agents/hooks/{guard_commands,watch_gpu_hang,test_hooks}.py`, `.agents/hooks.json` (22/22 unit tests passing), `.agents/workflows/{orchestrator_core,roadmap_orchestrator,test_orchestrator}.py`, and `docs/roadmap-state.json` (21-item DAG with 21 depending on 04/06 and 07 depending on 03/04/05/06/21, dash-delimited sub-branches eliminating Git D/F ref collisions, multi-slice intermediate stage merges, read-only Phase 6 share lenses, `MIN_NEXT_ANDROID_VERSION_CODE = 406 > sprint 405`, zero `flash_lite`, Stage-1 lockstep `HostCapabilities` contracts, and `strings_70/<code>.rs` routing keeping grandfathered `strings/*.rs <= 1,750` LOC); (2) Ground-Up 7.0 Obsidian Studio & Obsidian Cinema redesign and Full Visual Console-Grade Controller Editor across `01`, `02`, `09`, `13`, `16`, `17`, `18`, `19`, `20`, skills, and orchestrator (respecting all `size_ratchet.json` ceilings, `<= 800`-LOC new modules, Moonlight V+ port credits, zero mod names in UI/Nightjar per GPL-3.0 §7(e), zero `'Experimental'` in UI, zero stream-time blur when closed, and 22 host / 15 Nightjar locales); (3) Wave 1 Item `21` (`21-desktop-perf-and-nvenc.md`), `07-hw-verify.md`, and `00-protocol-reservations.md` against real codebase headers/modules and post-sprint diffs (ratchet-safe `nvenc.rs` shrink, disjoint Stage 1/Stage 2 slices, Linux `cfg` safety for macOS CI, GPL-3.0 attribution rules, and strict hardware honesty with zero Apple Silicon or NVIDIA GPU hardware execution claims); and (4) `docs/TEAMWORK-HANDOVER.md` (rebase/setup gates, `/teamwork` prompt, 100% Gemini 4 model roster, hook/env security boundaries, LF line endings across all 38 roadmap files, and untouched `b3d0c26b9b` tracked tree).

#### All 5 Round 3 Non-Blocking Polish Notes (Reconciled Immediately Before Commit)

1. **Nightjar release tag name (`docs/TEAMWORK-HANDOVER.md`):** Clarified that the release tag on mirrored `RamazanKara/nightjar` is exact `v7.0.0` (matching `client/.github/workflows/release.yml` and `orchestrator_core.py`).
2. **Canonical setup QR module (`docs/designs/17-ux-craft-and-resilience.md` Slice 17B):** Reconciled `qr_finish.rs` to extend Wave 0 Slice `01-F`'s canonical `rust/setup/src/finish_qr.rs` in place (zero duplicate setup QR file).
3. **64 KiB JSON `ErrorBody` prose consistency (`00`, `09`, `13`):** Aligned high-level prose with slice specs to specify `.layer(DefaultBodyLimit::disable())` + `axum::body::to_bytes(req.into_body(), 64 * 1024)` so >64 KiB bodies return structured JSON `ErrorBody` (`ErrorCode::TooLarge`, HTTP `413`).
4. **Explicit stage order (`10-couch-party-audio.md`, `12-phone-safe-area.md`):** Added explicit stage labels (`10-A` + `10-B` Stage 1 -> `10-C`/`10-D`/`10-E` Stage 2; `12-A` + `12-D` Stage 1 -> `12-B` Stage 2 -> `12-C` Stage 3) so compile dependencies land cleanly in order.
5. **Desktop `strings/tests.rs` `SOURCES` checklist (`docs/TEAMWORK-HANDOVER.md` §5):** Added a checklist reminder to register every new 7.0 Desktop `ui/*.rs` / `stream/*.rs` calling `tr`/`trf`/`en` inside `SOURCES` in `client/protocol/desktop/src/strings/tests.rs`.

### Round 4 — Final Post-Discovery + Vibepollo/USB Audit (`approved: true`, `0` blocking findings)

Covered all 29 items (`00`–`29`), `docs/reviews/gap-discovery-report.md`, Vibepollo/USB specs `22`–`26` (PR 1–5 / Tracks 1–8), gap-discovery Tier A specs `27`–`29` + 15 Tier B `§8` addenda, dynamic post-sprint path discovery in `orchestrator_core.py`, and the single canonical `/swarm` handover (`docs/TEAMWORK-HANDOVER.md`).

#### Verbatim Round 4 Carbon Summary

> Round 4 strictly read-only Carbon (`gemini-4-carbon-high`) audit of `C:\src\rl-roadmap-prep` against `origin/main` `6.3.0` (`995f2540c8`) and `C:\src\rubylight-sprint` guardrails APPROVED with zero blocking findings. Verified: (1) airtight protocol & collision contracts across `00-protocol-reservations.md` (§1–§8) and Vibepollo/USB specs `22`–`26` (`0x5500_0009` 20 B `declared==16` vs 40 B `declared==36` length + capability/encryption disambiguation, `0x5602` 992 B unreliable ENet ch `0x08` + mutual exclusion vs `0x5533` and Foundation `0x10`, `0x4000` PlayStation `type==0x02` vs Steam `type==0x04` scoping, `0x40_0000` Switch 2 C-button vs SC26 `LEFT_GRIP_TOUCH_FLAG` guard, reuse of `0x5500_0008`/`0x5504`/`0x5505`, unchunked Dolby `4096 + 64` B receive/RS cap without `assemble_frames()`, strict launch-warning XML/Unicode sanitizer & `&psmap=`, loopback-only `127.0.0.1:3240` `usbipd-win` + ownership journal + Track 8B/8C arbitration, and zero collision with `0x5548..=0x554F` / `0x5500_0081`); (2) acyclic, wave-consistent 29-item DAG in exact sync between `orchestrator_core.py` and `docs/roadmap-state.json` (22/22 unit tests passing, `07` gating all Wave 1 items, dynamic post-sprint `code-map.md` path discovery with zero manual pre-sprint PR workflows); and (3) full `AGENTS.md` compliance (SAxense/Nonary/Vibepollo/Moonlight V+ provenance strictly in code comments/`NOTICE`/`THIRD_PARTY.md`/README, complete GPL-3.0 §7(e) mod-name scrub, NVIDIA/Apple Silicon/Win ARM64/SC26/untested-SoC hardware honesty, 100% Gemini 4 roster, LF line endings, and zero edits to existing tracked files).

#### All 4 Round 4 Non-Blocking Polish Notes (Reconciled Immediately Before Commit)

1. **`PcmTraffic::observe` & `RESERVED_INPUT_MAGICS` (`25-ps-speaker-pcm-and-bt.md`):** Noted passing `controller_audio::SPEAKER` (or mapping `0x5602 => 1`) into `PcmTraffic::observe`, narrowing `RESERVED_INPUT_MAGICS` to `[0x5500_000A]` (or adding `ADOPTED_INPUT_MAGICS`) for `input_magics_are_unique()`, and pre-decode `0x0206` session-gated disambiguation in `stream/control.rs` ahead of `rust/core/src/input.rs`.
2. **`webrtc/server.rs` ownership (`29-android-pacing-and-transport.md`):** Included `rust/host/src/webrtc/server.rs` explicitly in Slice `29-B` (conflict-free because `29` depends on `27`).
3. **Slice naming (`17-ux-craft-and-resilience.md`):** Renamed §8's CI/symbol slice to `Slice 17H` (distinct from §6's `Slice 17G`).
4. **DAG & dry-run consistency (`gap-discovery-report.md`, `orchestrator_core.py`):** Synced `29` `depends_on: ["04", "06", "27"]` in `gap-discovery-report.md` §2 and updated `orchestrator_core.py`'s `dry_run` `prev_tag` default to `"v6.3.0"`.
