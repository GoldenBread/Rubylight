---
name: rubylight-feature-spec
description: >-
  Use when writing, critiquing (Phase 1.5), or verifying post-sprint paths (Phase 2)
  for any Rubylight or Nightjar feature design spec before writing production code.
---

# Rubylight Feature Spec Procedure

Follow `docs/encoder-focus-design.md` as the engineering depth bar and never start Phase 3 coding until Phases 1.5 and 2 pass.

## Required Spec Sections

Every spec under `docs/designs/<NN>-<slug>.md` must contain:

1. **Summary & User Experience**: Plain-language user value, zero internal jargon, zero "Experimental" labels, zero mod names (GPL-3.0 §7(e)).
2. **Protocol / Native API / Capability Contract**:
   - Preserve `PROTOCOL_VERSION` and stock Moonlight compatibility (clients asking for nothing new see nothing new).
   - Gate all client extras via `HostCapabilities` (serverinfo field, DESCRIBE line, or additive native API v1 feature string) — never host brand or version string.
   - Reserve every control ID, input magic, serverinfo field, launch param, and native feature string in `docs/designs/00-protocol-reservations.md` first.
3. **Architecture & <=800-LOC Module Plan**:
   - Keep decisions in `butterpollo-core` / `nightjar-policy` (OS-independent unit-testable functions), Windows/GPU calls in `butterpollo-windows`, wiring in `butterpollo` / app shells.
   - Respect `tools/arch_check.py`: every new file <= 800 LOC with a leading `//!` (Rust) or `/**` (Java/Kotlin) module doc header.
   - Never grow a grandfathered file in `tools/size_ratchet.json` past its recorded ceiling — extract helper modules instead.
   - Never add raw `Config` reads in `rust/host/src/stream(.rs|/**)` outside `stream/config.rs`.
   - Keep AMD AMF, NVIDIA NVENC (verified against headers/samples, never claimed as run on hardware), Windows 10, physical displays, WGC, Desktop Duplication, and D3D11 working.
4. **State Machine, Reconnect Matrix & Crash-Restore Journal**:
   - Define every state transition, reconnect/resume/roam/second-device case, and lock order.
   - Any mutation to host PC state (displays, DPI, audio routes, frame limiter, process suspend state, files) MUST be journaled before applying and restored on every exit, crash included.
5. **Settings & 10/10 Craft Plan**:
   - Fewer-settings rule: prefer safe measured defaults; opt-in/default-off for anything unmeasured; plain-English errors; dynamic input glyphs; zero stream-time blur; zero latency added to the default path.
6. **Disjoint Implementer Slices**:
   - Partition Phase 3 edits into strictly non-overlapping file lists so `parallel()` workers never collide.
7. **Verification & Measurement Plan**:
   - Narrowest core/policy unit tests, loopback/interop tests, visual checks, and idle-gated serialized HW criteria.

## Phase 1.5 (Spec Critic) & Phase 2 (Post-Sprint Path Verification)

1. Run a fresh read-only critic over the spec against `AGENTS.md`, `client/AGENTS.md`, `tools/arch_check.py`, and `tools/size_ratchet.json`.
2. Before Phase 3 coding, resolve every `[verify post-sprint code-map.md]` marker in the spec against current `docs/code-map.md`, `client/docs/code-map.md`, and live file line counts in `tools/size_ratchet.json`.
3. Update `docs/code-map.md` and `client/docs/code-map.md` in the same change whenever modules are added or moved.
