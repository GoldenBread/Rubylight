---
name: nightjar-parity
description: >-
  Use whenever changing Nightjar Android, Nightjar Desktop, shared protocol/policy
  crates, design tokens, Console Control Center, settings, virtual controller studio,
  or translations to enforce 7.0 console-quality parity and product rules.
---

# Nightjar 7.0 Parity & Console-Quality Craft Rules (Post-Refactoring)

Nightjar (`client/`) is a host-neutral Moonlight-protocol client for Android (Phone, Handheld, Android TV / Fire TV) and Desktop (Windows, Linux / Steam Deck, macOS), mirrored to `RamazanKara/nightjar`.

## 1. Ground-Up 7.0 Console-Quality Redesign Freedom

- Wave 0 (`docs/designs/02-ui-nightjar-glass.md`) has **full creative freedom** to redesign Nightjar from the ground up as a flagship console-quality OS (PS5 × Apple tvOS × SteamOS OLED × studio hardware polish) — it is NOT constrained by the pre-7.0 `client/design/README.md` or `redesign-2026-11.md` aesthetic or layout choices.
- `client/design/tokens.json` may be cleanly rewritten from scratch for the 7.0 visual system, while keeping `client/scripts/gen_design_tokens.py` as the **single source of truth** emitting Android XML (`client/app/src/main/res/values/design_tokens.xml`), Desktop Rust (`client/protocol/desktop-ui/src/ui/design_tokens.rs`), and Nightjar site CSS (`client/site/assets/design-tokens.css`):
  ```powershell
  python client/scripts/gen_design_tokens.py
  python client/scripts/test_gen_design_tokens.py
  ```
- Update `client/design/README.md` in Wave 0 so documentation matches the new 7.0 design system.

## 2. Parity & Post-Refactoring Crate Ownership

- Land every feature on **both Android and Desktop in the same change** (touch controls and Modern Virtual Controller Studio #151 are Android-only).
- Respect the completed split-crate workspace in `client/protocol/` (Edition 2024, MSRV 1.94):
  - `rubylight-protocol` (`core/`), `rubylight-wire` (`wire/`), `rubylight-native-api` (`native-api/`), `rubylight-traverse` (`traverse/`)
  - `nightjar-policy` (`policy/`, including `http/capabilities.rs` and declarative `settings.toml`)
  - `nightjar-core` (`client/`, unified streaming/HTTP/mDNS/WoL/hosts/console/pin/update engine)
  - `nightjar-desktop-config` (`desktop-config/`, `config.rs`, `strings.rs`, `strings/*.rs`, `strings_70/*.rs`)
  - `nightjar-desktop-ui` (`desktop-ui/`, `ui/`, `shot.rs`, `theme.rs`, `glyphs.rs`, `stream_menu.rs`)
  - `nightjar-desktop-media` (`desktop-media/`, `stream/`, `media.rs`, `vulkan_video/`, `kms/`, `metal/`, `gamepad/`, `usb/`)
  - `nightjar-desktop` (`desktop/`, thin binary + `app/` + `hosts/` + `gui.rs`)
  - `nightjar-android` (`android/`, pure-JDK JNI holders `Nightjar*Jni.java` with `buildHostNightjarCore`)
- Gate host extras strictly on `HostCapabilities` / `Feature` (`client/protocol/policy/src/http/capabilities.rs` + Java `HostCapabilities.java`) — never on host brand or `Family::Rubylight` / `Family.RUBYLIGHT` (enforced by `tools/arch_check.py`).
- Desktop builds on Windows, Linux, and macOS: gate Linux-only code with `target_os = "linux"` (never unannotated `cfg(unix)`, which includes macOS).
- If any desktop `.spv` shader changes, regenerate its Metal translation with `python client/protocol/desktop-media/src/metal/generate.py`.

## 3. Hard Performance, Compositor & Contrast Invariants

- **Zero stream-time blur rule**: while a stream plays with the Console Control Center / dialogs closed, add ZERO blur, ZERO extra compositor layers, and ZERO GPU work. Persistent HUD elements (stats, notices, Input Deck radial, Launch Curtain) use blur-0 HUD tokens; Control Center blur windows are **removed** (never merely hidden) on close so post-close GPU frame cost equals pre-open cost.
- **Compositor & blur stability (#61)**: listen to live cross-window blur / thermal / power-saver callbacks and fall back cleanly to baked/opaque surfaces with zero flash or transparency leak; no nested blur passes.
- **Resolution-aware scaling (#144, #146)**: scale chrome, type, and hit targets (>= 48 dp touch, 1.5× TV) cleanly across 720p handhelds, 4K TVs, ultrawides, and foldables.
- **Contrast floor**: text >= 4.5:1 (>= 7:1 High-Contrast) and focus rings/outlines >= 3.0:1 over worst-case white (night) and black (day) covers across Night, Day, OLED, and High-Contrast themes (`check-tokens.mjs` + `check-contrast.py`).

## 4. Fewer Settings, Help Copy, Input Glyphs & 15 Locales

- Prefer a safe, measured default over adding a setting; visual preview-card Settings UX merges related switches and hides what does not apply to the current device/host.
- Declare client settings in `client/protocol/policy/settings.toml` and regenerate via `python client/scripts/generate-settings.py` (see `.agents/skills/add-setting/SKILL.md`).
- Every removed, merged, or renamed key MUST migrate or clear its stored values, and a retired key name is never reused (`SettingsKeysTest` + `PreferenceMigrations` on Android; serde alias + `migrate` in `client/protocol/desktop-config/src/config/migrate.rs`).
- Every setting's help says **what the player gets, what it costs, and when to change it** in **two short sentences**, across Android, Desktop, `client/protocol/policy/settings.toml`, and all 15 languages.
- **Dynamic input glyphs**: derive button hints from the last `InputEvent` / SDL event via canonical `client/protocol/policy/src/input_glyphs.rs` (refactored from `client/protocol/desktop-ui/src/glyphs.rs`, preserving Moonlight V+ port credits in code comments/`NOTICE`/`THIRD_PARTY.md`/README): Xbox, PlayStation, Nintendo, Keyboard, and TV Remote (`OK` / `Hold OK`), and **no button hints for Touch or Mouse**. Every action must be reachable via controller, TV remote, keyboard, touch, and TalkBack/screen reader.
- **All 15 locales** in the same change (use `python tools/i18n.py` + `tools/i18n_glossary.json`): `en` plus `de, es, fr, it, ja, ko, pl, pt-BR, ru, tr, uk, vi, zh-Hans, zh-Hant` in `client/app/src/main/res/values*/strings.xml` (and `-television` where overridden) and **`client/protocol/desktop-config/src/strings_70/<code>.rs`** for all new 7.0.0 Desktop translations (chained from `client/protocol/desktop-config/src/strings.rs` and verified by `strings/tests.rs` — NEVER grow grandfathered `client/protocol/desktop-config/src/strings/*.rs` past their 1,750-LOC ratchet!). Use system face fallback for non-Latin-1 scripts (`pl, tr, vi, ru, uk, ja, ko, zh-*`) so no word mixes two fonts.
- **Forbidden strings & identity**: NEVER put `"Experimental"` anywhere in UI; NEVER name any mod (Nucleus Co-op, ANR) in UI or Nightjar; use `"Quit game"` (never `"Close game"`). NEVER change `applicationId` `com.butterpollo.client` or the signing key; NEVER touch `keystore.properties`.
- **Release notes**: write fragments in `client/notes.d/<topic>.md` (`## en` and/or `## desktop`), keeping Google Play store changelog <= 500 characters (`tools/nightjar_notes.py`).
