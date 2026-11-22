---
name: rubylight-verify
description: >-
  Use when verifying Rubylight host, Nightjar client, console, docs, architecture
  guardrails, or idle-gated hardware tests during inner-loop development, pre-merge
  gating, or release preparation.
---

# Rubylight & Nightjar Verification Ladder

Always run the narrowest fast loop while iterating, then escalate to full checks before merging.

> **Prerequisite (post-sprint rebase):** `tools/dev.ps1 -Fast`, `tools/arch_check.py`, and `tools/size_ratchet.json` come from the post-sprint `main` rebase. Wave 0 preflight in `orchestrator_core.py` verifies they exist before starting any coding wave.

## 1. Fast Inner Loop (Phase 3 Iteration)

```powershell
tools\dev.ps1 check -Fast -Base roadmap/redesign-and-waves-1-5
```

- Uses `tools/dev.py --fast` (static checks, check-only / `cargo nextest` targeted tests in warm target slots under `tools/build_lock.py` lanes).
- Add `-DryRun` first when verifying which checks your diff triggers.
- Never run `cargo fmt --all`; always use `cargo fmt -- --check` (without `--all`).

## 2. Full Pre-Merge Verification (Phase 4, Phase 5b & Post-Review)

Run all of the following and require exit code 0:

```powershell
tools\dev.ps1 check -Base roadmap/redesign-and-waves-1-5
python tools/arch_check.py
python tools/code_map.py check
python tools/generated_docs.py --check
python client/scripts/gen_design_tokens.py --check
git diff --check
```

Additional targeted checks when their trees changed:
- Console (`rust/web/`): `npm run check; node --test "*.test.mjs"; npm run build` (via web lane / `dev.ps1`).
- Nightjar docs/site (`client/`): `node client/scripts/check-doc-links.mjs`, `node client/scripts/device-docs.mjs --check`, `node client/site/check-links.mjs`, `python client/scripts/test_gen_design_tokens.py`.
- Host site (`site/`): `python site/build.py`.
- Service executable inputs (`rust/service/` or `update_recovery.rs` / `update_files.rs` / `state/file.rs` / `version_info.rs` / icon): bump `rust/service/Cargo.toml` and run `python rust/release/service_inputs.py --update`.
- Verify LF-only line endings on all changed files (no `\r\n`).
- Never run `python tools/arch_check.py --write-baselines` and never hand-edit `tools/size_ratchet.json` or `tools/arch_baselines.json`.

## 3. Full Package Gate (Exclusive Lock Only)

```powershell
python tools/build_lock.py -- pwsh rust/build.ps1
```

Never invoke `rust\build.ps1` without exclusive `tools/build_lock.py` (never pass `--lane` for `rust/build.ps1`).

## 4. Serialized Hardware, E2E & Soak Gate (Phase 7 & Wave 6)

All hardware runners are gated on `hw_lock`, absence of `.agents/GPU_HANG_HALT`, and `RUBYLIGHT_ALLOW_HW_GATE=1` in the process environment:
- `rust/release/e2e.py`, `rust/release/check.ps1`, `rust/release/soak.ps1`, `rust/release/soak.py`, `rust/release/focus_ab.py`
- `tools/hw/run.ps1`, `tools/hw/run.py`, `tools/hw/laptop-host.ps1`, `tools/hw/demo-stream.ps1`, `tools/hw/wave1_verify.py`
- Any `--ignored` / `--run-ignored` native GPU test

Procedure:
1. Verify installed host idle first:
   `(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` must report `RustHostSessionCount` = `0` and `RustHostPendingSessionCount` = `0`.
2. Run isolated test host / runner with `RUBYLIGHT_ALLOW_HW_GATE=1`.
3. **Exit code 3 from any hardware/stream/soak runner is an AMD encoder hang**: stop immediately, never retry, let `watch_gpu_hang.py` record `.agents/GPU_HANG_HALT`, and escalate for human GPU/driver inspection.
