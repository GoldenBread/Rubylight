---
name: release-prep
description: Prepare and verify a Rubylight host or Nightjar client release (fragments, version bump, full review, checks, pre-release hardware validation, signing rules, tagging, and signed-release attestation).
---

# Release Preparation (Host & Nightjar)

Use this skill when preparing a release, assembling release notes, checking service/signing invariants, running pre-release hardware validation, or attaching signed-release attestations. **Never tag or publish without explicit maintainer approval, never burn a public tag before hardware validation passes, and never move a published tag.**

## Release-note & credit rules (`AGENTS.md`, `rust/release/notes.d/README.md`, `client/notes.d/README.md`)

- Put pending notes in topic fragments (`rust/release/notes.d/<topic>.md` for host; `client/notes.d/<topic>.md` for Nightjar) rather than editing `rust/RELEASE_NOTES.md` on feature branches.
- **Contributor PR credits:** Every merged PR by anyone other than RamazanKara MUST get a fragment ending with `Thanks to **Name** (@login, #N).`
- **Port credits stay OUT of release notes and UI** (they belong in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README credits).
- State user-facing impact with our own release-build measurements only; never add an "Experimental" label.

## Pre-release whole-range review (mandatory before tagging)

1. Review the full diff from the previous release tag to the candidate SHA (`git log <prev-tag>..<sha>`), together with the assembled release notes (host and Nightjar if shipping together). Fix every finding before tagging.
2. Verify main CI is green on the exact candidate SHA:
   ```powershell
   gh -R RamazanKara/Rubylight run list --branch main --limit 10
   ```
   Workflows: `rust-windows.yml` (host), `nightjar-android.yml` and `nightjar-macos.yml` (Nightjar).

## Host release steps

1. **Service version rule:** If anything in `rust/service/`, `rust/core/src/{update_recovery,update_files}.rs`, `rust/core/src/state/file.rs`, `rust/assets/version_info.rs`, or `rust/assets/butterpollo.ico` changed, bump `rust/service/Cargo.toml` and run:
   ```powershell
   python rust/release/service_inputs.py --update
   ```
   (`rust/release/test_release.py` fails if `rust/service/inputs.txt` is stale.)
2. **Bump version & fold fragments:**
   ```powershell
   python rust/release/bump.py <version>
   ```
   Updates `Cargo.toml`, `Cargo.lock`, READMEs/docs, `rust/web/package*.json`, and folds `rust/release/notes.d/*.md` into `rust/RELEASE_NOTES.md` under `## New in <version>`.
3. **Full verification gates:**
   ```powershell
   $env:TEMP="D:\t"; $env:TMP="D:\t"
   python tools/i18n.py check
   python tools/arch_check.py
   python tools/code_map.py check
   python tools/generated_docs.py --check
   git diff --check
   pwsh rust/build.ps1
   ```
   Also verify `cargo fmt -- --check` (without `--all`) and `python -m unittest discover -s tools -p "test_*.py"`.
4. **Pre-release hardware validation (MANDATORY before tagging):**
   Validate the exact green `main` release candidate on real hardware **before** pushing `v<version>` (exit code 3 = AMD encoder hang — stop and investigate, never retry; fix any failure on `main` and re-validate before tagging):
   ```powershell
   # Radeon workstation: full stream matrix (H.264/HEVC/VRR/AV1/1440p120/PyroWave) + e2e.py + protocol.py on an isolated host
   pwsh rust/release/check.ps1 <green-main-rc-tag> -NoInstall -NoUpload
   ```
   *(If no RC tag exists yet, run `rust/release/e2e.py`, `rust/release/protocol.py`, and `rust/release/soak.ps1 -Mode short` directly against the local `rust/build.ps1` package.)*
   Also run laptop & attached device checks against the candidate build per `.agents/skills/device-checks/SKILL.md` (e.g., `pwsh tools/hw/laptop-host.ps1 -Out <dir> -Package <pkg>`, `tools/hw/run.ps1 -Batch <device>` for `retroid`, `nuc`, TV).
5. **Tagging (ONLY after maintainer OK, green main CI, AND passing pre-release hardware validation):**
   Push the tag by commit SHA (avoids colliding with Nightjar subtree `v4.x` tags in local clones):
   ```powershell
   git push origin <green-main-sha>:refs/tags/v<version>
   ```
6. **Signed-release upgrade check & attestation upload (after `v<version>` publishes):**
   Verify the final signed installer upgrades cleanly over the installed host and upload `VALIDATION.json` and `REAL-CLIENT.json` to the published GitHub release:
   ```powershell
   pwsh rust/release/check.ps1 v<version>            # Radeon workstation signed upgrade + VALIDATION.json (exit 3 = AMD hang, do not retry)
   pwsh rust/release/smoke.ps1 -Version v<version>   # Measurement laptop real-client check + REAL-CLIENT.json
   ```

## Nightjar release steps

1. Bump `versionName` and `versionCode` in `client/app/build.gradle`.
2. Assemble release notes & Google Play changelog (enforces 500-char Play Store limit):
   ```powershell
   python tools/nightjar_notes.py --out <scratch>\whats-new.md
   ```
3. **Signing (`client/AGENTS.md`):** Never change `applicationId` (`com.butterpollo.client`) or the signing key. Copy `keystore.properties` from outside the repo into `client/` only for the build, delete it immediately after, and never print or commit it. Verify `apksigner verify --print-certs` SHA-256 equals `312ed322c7c29d1aec4fae4a3979ebcb557af44d821d7f4746c0968548eb630b`.
4. **Mirroring & pre-release device validation:** CI mirrors `client/` to `RamazanKara/nightjar` via `nightjar-mirror.yml` once Nightjar checks pass (manual fallback: `tools\mirror-nightjar.ps1`). Before pushing `v<version>` on `RamazanKara/nightjar`, run `release.yml` with `dry_run=true` and run pre-release device checks on available hardware (Android handheld/TV/phone, Desktop, NUC) against an isolated candidate host per `.agents/skills/device-checks/SKILL.md`.
