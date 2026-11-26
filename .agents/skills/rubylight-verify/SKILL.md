---
name: rubylight-verify
description: >-
  Use when verifying Rubylight host, Nightjar client, console, docs, locales,
  architecture guardrails, firewall safety, shared PC+laptop builds, or idle-gated
  hardware tests during inner-loop development, pre-merge gating, or release preparation.
---

# Rubylight & Nightjar Verification Ladder (Post-Refactoring, Shared PC + Laptop Builds)

Always run the narrowest fast loop while iterating, use the shared warm target pool (`C:\src\rl-target-pool`) + lane locks (`tools/build_lock.py`), offload heavy full checks to the laptop when PC lanes are busy, and escalate to full checks before merging. Also consult `.agents/skills/isolated-e2e/SKILL.md`, `.agents/skills/check-nvenc/SKILL.md`, and `.agents/skills/release-prep/SKILL.md` for specialized runs.

## 1. Fast Inner Loop (Phase 3 Iteration — Always Shared Warm Pool!)

```powershell
$env:TEMP="D:\t"; $env:TMP="D:\t"
tools\dev.ps1 check -Fast -Base roadmap/redesign-and-waves-1-5
```

- **Shared warm pool (`C:\src\rl-target-pool`):** `tools\dev.ps1 check` automatically takes one of the PC's **4 Cargo lanes** (`cargo-1..4`), claims a warm target slot (`1..4` host / `client-1..2` client via `claim_target` with `sccache` + `lld` + `nextest` + `user_idle.py`), or takes the **1 Gradle lane** (`D:\CodexArtifacts\locks\gradle-lane`) / **1 Web lane** (`web-1`).
- For narrow single-crate host iterations, use `tools\dev.ps1 check -Fast -Paths <file>` OR `pwsh C:\src\rl-quick.ps1 -Repo . -Crates <crate>` (which also shares `C:\src\rl-target-pool` + `C:\src\rl-env.ps1` + `user_idle.py`).
- For direct Gradle runs outside `dev.ps1`, ALWAYS take the shared Gradle lane with `--no-daemon`:
  ```powershell
  python tools/build_lock.py --lane gradle --label nightjar -- cmd /c client\gradlew.bat -p client --no-daemon --max-workers=2 :app:testNonRootDebugUnitTest
  ```
- **Blocked by PreToolUse hook (`guard_commands.py`):** bare `cargo build/test/clippy/check` without a shared pool/wrapper (prevents 10–25 GB private `target/` pileups), direct unlocked writes to `C:\src\rl-target-pool\<n>`, bare `gradlew` without `--lane gradle` + `--no-daemon`, `cargo fmt --all` (use `cargo fmt -- --check`), and wildcard `http.server` / `vite --host 0.0.0.0`.
- **Firewall-prompt hygiene (`AGENTS.md`):** Bind ALL unit tests, probes, and local preview servers to loopback (`127.0.0.1` / `[::1]` / `LOCALHOST`) or query `GetExtendedTcpTable`/`GetExtendedUdpTable` so rebuilt Cargo test binaries NEVER trigger Windows Defender Firewall modal popups.

## 2. Full Pre-Merge Verification (Phase 4, Phase 5b & Post-Review)

Run all of the following and require exit code 0:

```powershell
$env:TEMP="D:\t"; $env:TMP="D:\t"
tools\dev.ps1 check -Base roadmap/redesign-and-waves-1-5
python .agents/tools/roadmap_hygiene.py check-firewall --base roadmap/redesign-and-waves-1-5
python tools/i18n.py check
python tools/arch_check.py
python tools/code_map.py check
python tools/generated_docs.py --check
python client/scripts/gen_design_tokens.py --check
git diff --check
```

Additional targeted checks when their trees changed:
- Host settings (`rust/core/config/settings.toml`): `python tools/generated_docs.py` (regenerates `catalog_gen.rs`, `retired_gen.rs`, `console_*_gen.rs`, `rust/web/src/lib/schema/*.gen.ts`, and `docs/configuration.md`).
- Client settings (`client/protocol/policy/settings.toml`): `python client/scripts/generate-settings.py` (regenerates `settings_catalog_gen.rs`, `defaults_gen.rs`, `SettingsCatalogGen.java`, and `client/docs/settings.md`).
- Console (`rust/web/`): `npm run check; node --test "*.test.mjs"; npm run build` (via web lane / `dev.ps1`).
- Nightjar docs/site (`client/`): `node client/scripts/check-doc-links.mjs`, `node client/scripts/device-docs.mjs --check`, `node client/site/check-links.mjs`, `python client/scripts/test_gen_design_tokens.py`.
- Host site (`site/`): `python site/build.py`.
- Service executable inputs (`rust/service/` or `rust/core/src/{update_recovery,update_files}.rs` / `rust/core/session/src/state/file.rs` / `rust/assets/version_info.rs` / icon): bump `rust/service/Cargo.toml` and run `python rust/release/service_inputs.py --update`.
- Verify LF-only line endings on all changed files (no `\r\n`).
- Never run `python tools/arch_check.py --write-baselines` and never hand-edit `tools/size_ratchet.json` or `tools/arch_baselines.json`.

## 3. Effective PC + Laptop Shared Build Scheduling & Garbage Collection

Check live PC + laptop availability before kicking off heavy checks:

```powershell
python .agents/tools/roadmap_hygiene.py pick-lane
```

- **PC (`16 threads / 64 GB RAM`):** 4 Cargo lanes (4 warm Host slots + 2 warm Client slots in `C:\src\rl-target-pool`), 1 Gradle lane (`D:\CodexArtifacts\locks\gradle-lane`), 1 Web lane (`web-1`). Respects any other active agent/user build automatically via `tools/build_lock.py`.
- **Laptop (`ramaz@192.168.4.12`, `16 threads / 31 GB RAM`, 2 Cargo lanes + Gradle + Web):** Ideal for offloading heavy full `dev.ps1 check -KeepGoing` runs (Phase 4 / Phase 5b) while PC lanes run parallel `-Fast` inner-loop builds and independent Web/Gradle/Rust slices!
  ```powershell
  git push -u origin wip/roadmap/<id>-<slug>
  pwsh tools\remote-check.ps1 -Host laptop -Branch wip/roadmap/<id>-<slug>
  ```
- **Automatic graceful fallback:** If SSH is unreachable or `remote-check.ps1` exits **`4`** (laptop busy) or **`5`** (nobody / another user at console), fall back transparently to local `tools\dev.ps1 check`. Note: exit `3` from `remote-check.ps1` is a timeout, NOT an AMD encoder hang.
- **Mandatory Garbage Collection (between waves and whenever disk space tightens):**
  ```powershell
  python .agents/tools/roadmap_hygiene.py clean --apply --remote-laptop
  ```
  Cleans any accidental private worktree `target/`/`client/app/build/`/`.cxx/`/`_site/`/`dist/`, prunes merged temporary slice branches & dead worktrees, expires old `wip-roadmap-*` `dev-check` logs & temp diff dumps, runs `tools/build_lock.py --prune --min-free-gb 50` when `C:` dips below 50 GB, and safely removes finished detached `C:\src\rl-check\wip-roadmap-*` worktrees on the laptop when idle.

## 4. Full Package Gate (Exclusive Lock Only)

```powershell
$env:TEMP="D:\t"; $env:TMP="D:\t"
python tools/build_lock.py -- pwsh rust/build.ps1
```

Never invoke `rust\build.ps1` without exclusive `tools/build_lock.py` (never pass `--lane` for `rust/build.ps1`).

## 5. Serialized Hardware, E2E & Soak Gate (Phase 7 & Wave 6)

Idle-gated serialized hardware runners are **allowed by default** (blocked only if `.agents/GPU_HANG_HALT` exists or `RUBYLIGHT_DISABLE_HW_GATE=1` is explicitly set in the process environment):
- `rust/release/e2e.py`, `rust/release/check.ps1`, `rust/release/soak.ps1`, `rust/release/soak.py`, `rust/release/focus_ab.py`, `rust/tests/crash_restore_soak.py`
- `tools/hw/run.ps1`, `tools/hw/run.py`, `tools/hw/laptop-host.ps1`, `tools/hw/demo-stream.ps1`, `tools/hw/wave1_verify.py`
- Any `--ignored` / `--run-ignored` native GPU test

Mandatory Procedure (see also `.agents/skills/isolated-e2e/SKILL.md`):
1. Verify installed host idle first:
   `(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` must report `RustHostSessionCount` = `0` and `RustHostPendingSessionCount` = `0`.
2. Run serialized isolated test host / runner (`hw_lock`, never two GPU runs concurrently).
3. **Exit code 3 from any hardware/stream/soak runner is an AMD encoder hang**: stop immediately, never retry, let `watch_gpu_hang.py` record `.agents/GPU_HANG_HALT`, and escalate for human GPU/driver inspection.
