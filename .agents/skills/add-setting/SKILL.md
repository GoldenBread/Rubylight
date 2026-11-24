---
name: add-setting
description: Add, modify, or retire a Rubylight host setting or Nightjar client setting across config, UI schema, locales, docs, and tests.
---

# Add or Change a Setting

Use this skill when adding, changing, renaming, or retiring a setting in Rubylight (host) or Nightjar (Android + desktop).

## Core rules

- **Never read raw `Config` inside `rust/host/stream/src/stream/**`.** Always read effective stream settings through `rust/host/stream/src/stream/config.rs` so override precedence (`app + device > app > device > host > preset`) applies (`tools/arch_check.py` enforces `raw-config-read`).
- **Never drop a key silently.** Rename or retire keys via `RENAMED_KEYS` / `REMOVED_KEYS` in `rust/core/config/src/config/retired.rs` (host), serde aliases / `migrate` in `client/protocol/desktop-config/src/config.rs` (desktop), or preference migration + `SettingsKeysTest` (Android).
- **Copy style:** State what the user gets, what it costs (delay, quality, network, load, or what breaks), and when to change it ("Leave it on unless ..."). Put break-streaming warnings first. Never label shipping features "Experimental".
- **Fewer settings on Nightjar:** Prefer a safe measured default over a new switch; ship Android and desktop parity in one change.

## Host setting checklist

1. **Scaffold starter snippets** (optional, dry-run by default):
   ```powershell
   python tools/scaffold.py setting <key> --type bool --default true --label "..." --description "..." --dry-run
   ```
2. **Core accessor & validation:** Add the typed accessor and default on `Config` in `rust/core/config/src/config.rs` (using parsers in `rust/core/config/src/config/values.rs`). Add unit tests in `rust/core/config/src/config/tests.rs` (or its scenario modules).
   - If it belongs to a 3.0 extension, add it to `SETTINGS_30` in `rust/core/config/src/config/settings30.rs`.
   - If per-app or per-device overrides apply, update `override_allowed` in `rust/core/config/src/config/overrides.rs` (resolution lives in `rust/core/session/src/device_overrides.rs`).
   - If renaming or retiring, register it in `rust/core/config/src/config/retired.rs`.
3. **Web console schema:** Add the entry (matching the Rust default) to its category file in `rust/web/src/lib/schema/` (`basics.ts`, `display.ts`, `video.ts`, `library.ts`, `extensions.ts`) and assign its tier in `rust/web/src/lib/schema/tiers.ts` (everyday settings need no tier entry and may not depend on non-everyday settings; folded groups live in `FOLDED_GROUPS` in `rust/web/src/lib/schema/main.ts`).
4. **No-JS console & host locales:** Add the row in `rust/host/web/src/console/settings.rs` and add translations across all 22 `rust/assets/locale/*.json` files via:
   ```powershell
   python tools/i18n.py apply <spec.json> --dry-run
   python tools/i18n.py apply <spec.json>
   ```
5. **Docs & release note:** Update the topic section and key table in `docs/configuration.md` (search/slice — never read whole) and add a user-facing bullet fragment in `rust/release/notes.d/<topic>.md`.

## Nightjar setting checklist (Desktop + Android)

1. **Shared policy:** Put any decision shared by both apps in `client/protocol/policy/src/`.
2. **Desktop:** Add field + default to `Settings` (and `Overrides` if per-PC/game) in `client/protocol/desktop-config/src/config.rs`, row in `client/protocol/desktop-ui/src/ui/settings.rs`, tests in `client/protocol/desktop-ui/src/ui/settings_tests.rs`, and all 14 tables in `client/protocol/desktop-config/src/strings/*.rs` via `tools/i18n.py apply`.
3. **Android:** Add preference in `client/app/src/main/res/xml/preferences.xml`, reading/migration in `client/app/src/main/java/com/limelight/preferences/PreferenceConfiguration.java`, strings across `client/app/src/main/res/values*/` via `tools/i18n.py apply` (cross-fills shared Desktop<->Android strings automatically), and update `client/scripts/generate-settings.py` then run `python client/scripts/generate-settings.py` to refresh `client/docs/settings.md`.

## Verification

Always set `$env:TEMP="D:\t"; $env:TMP="D:\t"` and wrap Cargo/Web builds with `tools/build_lock.py`:

```powershell
python tools/build_lock.py --lane cargo -- cargo test -p butterpollo-config --locked
python tools/build_lock.py --lane web -- pwsh -c "cd rust/web; node --test SettingsSchema.test.mjs SettingsHome.test.mjs SettingsTiers.test.mjs"
python tools/i18n.py check
python tools/arch_check.py
python tools/code_map.py check
tools\dev.ps1 check
```
