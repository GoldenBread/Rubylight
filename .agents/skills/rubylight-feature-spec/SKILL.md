---
name: rubylight-feature-spec
description: >-
  Use when writing, critiquing (Phase 1.5), or verifying post-refactoring paths (Phase 2)
  for any Rubylight or Nightjar feature design spec before writing production code.
---

# Rubylight Feature Spec Procedure (Post-Refactoring Split-Crate Layout)

Follow `docs/encoder-focus-design.md` as the engineering depth bar and never start Phase 3 coding until Phases 1.5 and 2 pass. Always pair this skill with `origin/main`'s official narrow workflow skills in `.agents/skills/` when applicable:
- `.agents/skills/add-control-message/SKILL.md` (new control messages + `tools/scaffold.py control-message`)
- `.agents/skills/add-native-endpoint/SKILL.md` (additive v1 Native API routes + `tools/scaffold.py native-endpoint`)
- `.agents/skills/add-setting/SKILL.md` (declarative `settings.toml` catalogs + `tools/i18n.py`)
- `.agents/skills/check-nvenc/SKILL.md` (NVIDIA NVENC / CUDA / NGX header & fake-driver audits)
- `.agents/skills/isolated-e2e/SKILL.md` (isolated host streaming / soak / HW runners)
- `.agents/skills/release-prep/SKILL.md` (version bump, fragments, signing, and release review)

## Required Spec Sections

Every spec under `docs/designs/<NN>-<slug>.md` must contain:

1. **Summary & User Experience**: Plain-language user value, zero internal jargon, zero "Experimental" labels, zero mod names (GPL-3.0 §7(e)).
2. **Protocol / Native API / Capability Contract**:
   - Preserve `PROTOCOL_VERSION` and stock Moonlight compatibility (clients asking for nothing new see nothing new — `tests/host/tests/reconnect_tests/extension_tests.rs`).
   - Gate all client extras via `HostCapabilities` (`client/protocol/policy/src/http/capabilities.rs` + Java `HostCapabilities.java`) — never host brand or version string.
   - Reserve every control ID, input magic, serverinfo field, launch param, and native feature string in `docs/designs/00-protocol-reservations.md` first (`client/protocol/core/src/ids.rs`, `client/protocol/native-api/src/`).
3. **Architecture & <=800-LOC Module Plan (Post-Refactoring Split Crates)**:
   - Keep pure decisions in `rust/core/{crypto,config,media-policy,input-policy,session}/` (`butterpollo-*`) and `client/protocol/policy/` (`nightjar-policy`); Windows/GPU calls in `rust/windows/{abi,graphics,display,io}/` (`butterpollo-win-*`); host wiring in `rust/host/{stream,webrtc,web}/` (`butterpollo-{stream,webrtc,web}`); desktop split across `client/protocol/desktop-{config,ui,media}/` + thin `client/protocol/desktop/`.
   - Respect `tools/arch_check.py`: every new file <= 800 LOC with a leading `//!` (Rust) or `/**` (Java/Kotlin) module doc header.
   - `tools/size_ratchet.json` now contains ONLY **18** grandfathered files (the 14 `client/protocol/desktop-config/src/strings/*.rs` locale tables at 1,750 LOC + 4 narrow host/service/driver files). Route all new 7.0 Desktop strings exclusively to `client/protocol/desktop-config/src/strings_70/<code>.rs` and never grow any ratcheted file.
   - Never add raw `Config` reads in `rust/host/stream/src/stream(.rs|/**)` outside `stream/config.rs`.
   - Use loopback (`127.0.0.1`/`[::1]`) and IP-helper tables in unit tests and port probes (never wildcard `0.0.0.0`/`[::]` listens that trigger Windows Defender Firewall prompts).
   - Keep AMD AMF, NVIDIA NVENC (verified against `rust/windows/abi/include/nvEncodeAPI.h` & samples, never claimed as run on hardware), Windows 10, physical displays, WGC, Desktop Duplication, and D3D11 working.
4. **State Machine, Reconnect Matrix & Crash-Restore Journal**:
   - Define every state transition, reconnect/resume/roam/second-device case, and lock order.
   - Any mutation to host PC state (displays, DPI, audio routes, frame limiter, process suspend state, files) MUST be journaled before applying and restored on every exit, crash included (`RecoveryJournalV2`).
5. **Settings & 10/10 Craft Plan**:
   - Declare host settings in `rust/core/config/settings.toml` (`python tools/generated_docs.py`) and client settings in `client/protocol/policy/settings.toml` (`python client/scripts/generate-settings.py`).
   - Fewer-settings rule: prefer safe measured defaults; opt-in/default-off for anything unmeasured; plain-English errors; dynamic input glyphs; zero stream-time blur; zero latency added to the default path.
6. **Disjoint Implementer Slices**:
   - Partition Phase 3 edits into strictly non-overlapping file lists so parallel workers never collide.
7. **Verification & Measurement Plan**:
   - Narrowest core/policy unit tests, loopback/interop tests (`tests/host/`, `tests/interop/`), visual checks, and idle-gated serialized HW criteria.

## Phase 1.5 (Spec Critic) & Phase 2 (Live Path Verification)

1. Run a fresh read-only critic over the spec against `AGENTS.md`, `client/AGENTS.md`, `tools/arch_check.py`, and `tools/size_ratchet.json`.
2. Before Phase 3 coding, verify every target path against live `docs/code-map.md`, `client/docs/code-map.md`, and `tools/size_ratchet.json`, and check whether any sub-task already landed on `main` (skip already-shipped fixes cleanly without regressing them).
3. Update `docs/code-map.md` and `client/docs/code-map.md` in the same change whenever modules are added or moved.
