---
name: release-prep
description: Prepare and verify a Rubylight host or Nightjar client release (fragments, version bump, full review, checks, signing rules, tagging, and post-publish validation).
---

# Release Preparation (Host & Nightjar)

Use this skill when preparing a release, assembling release notes, checking service/signing invariants, or running post-release validation. **Never tag or publish without explicit maintainer approval, and never move a published tag.**

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
4. **Tagging (ONLY after maintainer OK and green main CI):**
   Push the tag by commit SHA (avoids colliding with Nightjar subtree `v4.x` tags in local clones):
   ```powershell
   git push origin <green-main-sha>:refs/tags/v<version>
   ```
5. **Post-publish workstation & laptop checks:**
   ```powershell
   pwsh rust/release/check.ps1                  # Radeon workstation (exit 3 = AMD hang, do not retry)
   pwsh rust/release/smoke.ps1 -Version v<version>   # Measurement laptop
   ```

## Nightjar release steps

1. Bump `versionName` and `versionCode` in `client/app/build.gradle`.
2. Assemble release notes & Google Play changelog (enforces 500-char Play Store limit):
   ```powershell
   python tools/nightjar_notes.py --out <scratch>\whats-new.md
   ```
3. **Signing (`client/AGENTS.md`):** Never change `applicationId` (`com.butterpollo.client`) or the signing key. Copy `keystore.properties` from outside the repo into `client/` only for the build, delete it immediately after, and never print or commit it. Verify `apksigner verify --print-certs` SHA-256 equals `312ed322c7c29d1aec4fae4a3979ebcb557af44d821d7f4746c0968548eb630b`.
4. **Mirroring:** CI mirrors `client/` to `RamazanKara/nightjar` via `nightjar-mirror.yml` once Nightjar checks pass (manual fallback: `tools\mirror-nightjar.ps1`).
