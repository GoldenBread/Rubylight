# Item 23: Vibepollo Dolby Audio RTSP Negotiation & Unchunked Datagram Receive Parity (Wave 2 — PR 2: Track 5)

Status: engineering design (Phase 1). Written against `origin/main` `995f2540c8` (`6.3.0`) and [`00-protocol-reservations.md`](00-protocol-reservations.md).

## 1. Summary & User Experience

Item 23 delivers **PR 2 (Track 5)**: seamless Dolby AC-3 / E-AC-3 passthrough interoperability between **Nightjar** (Android & Desktop) and **Vibepollo** hosts that negotiate compressed Dolby audio via `a=x-ss-audio-codec-offer` / `a=x-ss-audio-codec-accept` / `X-SS-Audio-Codec` (without Rubylight's `a=x-rl-audio.codecs`) — while preserving 100% of Rubylight's existing `a=x-rl-audio.codecs` chunked passthrough, 12-channel 7.1.4 Opus/PCM, and golden audio/FEC byte parity!

### 1.1 Exact RTSP & Receive Contract

1. **RTSP `DESCRIBE` Offer Parsing (`client/protocol/client/src/rtsp/describe.rs`):**
   - When `a=x-rl-audio.codecs` is **absent**, parse `a=x-ss-audio-codec-offer:AC3|EAC3` (case-insensitive token list, ignoring unknown tokens cleanly).
   - Expose capability honestly via `HostCapabilities` (never gated on host brand or `Family::Rubylight`!).
2. **RTSP `ANNOUNCE` Negotiation (`client/protocol/client/src/rtsp/mod.rs`):**
   - When `audio_passthrough` is enabled, the client decoder/output supports AC-3 or E-AC-3, and negotiated channels are `2` or `6`, emit `a=x-ss-audio-codec-accept:AC3|EAC3` (only advertising formats supported by both the host offer and client sink).
   - In `Stage::Announce` response parsing, read `X-SS-Audio-Codec` **case-insensitively** (`"ac3"`, `"eac3"`, `"opus"`): commit Dolby passthrough + **`32 ms` packet duration** ONLY when the host explicitly confirms `"ac3"` or `"eac3"`; otherwise fall back cleanly to Opus with zero error!
3. **Mandatory Unchunked Dolby Receive Fixes (Audio Receiver, Queue & Session Modules):**
   - **No `ChunkHeader` assembly for Vibepollo unchunked Dolby:** Do **NOT** call `receiver.assemble_frames()` (in `nightjar-core`'s session receive setup) when streaming Vibepollo unchunked Dolby — each UDP packet carries a complete AC-3 / E-AC-3 syncframe with **no `ChunkHeader`** and **no `frames_in_chunks()`** framing (keeping chunked assembly untouched for Rubylight's `a=x-rl-audio.codecs` path!).
   - **Fix `MAX_PACKET_SIZE = 1400` Truncation (`audio/mod.rs` + `audio/queue.rs`):** Unchunked 640 kbps AC-3 / E-AC-3 datagrams reach **~2,588 bytes** and were silently truncated at `MAX_PACKET_SIZE = 1400`! Raise the active receive/decrypt/RS buffer cap dynamically to **`4096 + 64`** bytes **only when unchunked Dolby is active**, and verify `audio/queue.rs` Reed-Solomon block sizing handles `<= 4096`-byte shards cleanly while keeping Opus / Rubylight chunked audio / `golden_audio` / `golden_fec` / `difftest` **byte-for-byte identical**!
4. **Android Passthrough Unlock (`SettingRules.java`, `PreferenceConfiguration.java`):**
   - Unlock Android's `Audio passthrough` preference when connected to Vibepollo hosts advertising `a=x-ss-audio-codec-offer` even when Rubylight's `AudioCodecs` `/serverinfo` element is absent.
5. **Optional Host Interop (`rust/host/src/rtsp_server.rs`):**
   - Optional additive host support for third-party clients sending `a=x-ss-audio-codec-accept:AC3|EAC3` (responding with `X-SS-Audio-Codec` while keeping Rubylight's own `a=x-rl-audio.codecs` negotiation 100% unchanged and preferred when present).

---

## 2. Protocol, Native API & Capability Contract

Per [`00-protocol-reservations.md`](00-protocol-reservations.md):
- `PROTOCOL_VERSION` and `rubylight_native_api::API_VERSION = 1` remain untouched.
- Additive RTSP lines: `a=x-ss-audio-codec-offer:AC3|EAC3` (Describe), `a=x-ss-audio-codec-accept:AC3|EAC3` (Announce request), `X-SS-Audio-Codec: ac3|eac3|opus` (Announce response header, case-insensitive).
- Zero new control message IDs or input magics.
- Preserves 12-channel 7.1.4 audio, Rubylight chunked passthrough (`a=x-rl-audio.codecs`), and stock Moonlight Opus streams byte-for-byte.

---

## 3. Architecture & <=800-LOC Module Plan

All target modules below are conceptual anchors — **resolve exact post-sprint file paths, split submodules, and `tools/size_ratchet.json` ceilings live via `docs/code-map.md` and `client/docs/code-map.md` before assigning implementer slices** (`[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]`). Grandfathered files MUST NOT grow past their live post-sprint ceilings.

| Path | Status | Est. LOC | Responsibility |
| --- | --- | ---: | --- |
| `client/protocol/client/src/rtsp/dolby_ss.rs` | New | ~210 | `//!` `a=x-ss-audio-codec-offer` parser, `a=x-ss-audio-codec-accept` builder (2ch/6ch guard), and case-insensitive `X-SS-Audio-Codec` confirmation + unit/fuzz tests. |
| `client/protocol/client/src/rtsp/describe.rs`, `rtsp/mod.rs`, `rtsp/tests.rs` | Modify | `<= ceiling` | Delegate Vibepollo Dolby negotiation to `dolby_ss.rs` whenever `a=x-rl-audio.codecs` is absent; commit 32 ms duration only on confirmed AC3/EAC3. |
| `client/protocol/client/src/audio/mod.rs`, `audio/queue.rs`, `session/mod.rs` | Modify | `<= ceiling` | Dynamic `4096 + 64` receive/RS shard cap when unchunked Dolby is active; skip `receiver.assemble_frames()` on Vibepollo unchunked Dolby while preserving chunked & Opus paths bit-for-bit. |
| `client/app/src/main/java/com/limelight/preferences/{SettingRules,PreferenceConfiguration}.java`, `HostCapabilities.java` | Modify | `<= ceiling` | Unlock passthrough on hosts offering `x-ss-audio-codec-offer` without `AudioCodecs` (capability-gated, never brand-gated). |
| `rust/host/src/rtsp_server.rs` & `rust/core/src/rtsp.rs` *(optional host interop)* | Modify | `<= ceiling` | Accept third-party `a=x-ss-audio-codec-accept` while preserving `a=x-rl-audio.codecs` priority unchanged. |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

```text
DESCRIBE received:
  ├── a=x-rl-audio.codecs present ──▶ Existing Rubylight chunked passthrough (assemble_frames=true)
  └── a=x-ss-audio-codec-offer present & passthrough on & channels in {2,6}:
        ──▶ Send a=x-ss-audio-codec-accept:<matched> in ANNOUNCE
              ├── Response X-SS-Audio-Codec == ac3|eac3 (case-insensitive):
              │     ──▶ Commit Dolby unchunked (32 ms, buf_cap=4096+64, assemble_frames=false)
              └── Missing / opus / unknown:
                    ──▶ Fallback cleanly to Opus (standard 1400 B cap, zero stream error)
```

- **Reconnect:** Re-negotiates fresh on every RTSP `DESCRIBE`/`ANNOUNCE` cycle; never caches stale unchunked/chunked framing across host switches.
- **Crash-Restore:** Zero host OS state mutated.

---

## 5. Settings & 10/10 Craft Plan

- **Zero new settings** (reuses existing `Audio passthrough` toggle cleanly).
- **Zero `"Experimental"`, zero mod names, zero host-brand gating.**
- **Provenance:** Credit Vibepollo / Nonary protocol conventions cleanly in code comments, `NOTICE`, and `rust/THIRD_PARTY.md` (never in UI text or release notes).

---

## 6. Disjoint Implementer Slices (Phase 3 `parallel()`)

| Slice | Stage | Exclusive Owned Files (Zero Overlap) |
| --- | --- | --- |
| **23-A: Nightjar Core RTSP Dolby Negotiation & Unchunked Receive/RS** | **Stage 1** | `client/protocol/client/src/rtsp/dolby_ss.rs` (new), `client/protocol/client/src/rtsp/describe.rs`, `client/protocol/client/src/rtsp/mod.rs`, `client/protocol/client/src/rtsp/tests.rs`, `client/protocol/client/src/audio/mod.rs`, `client/protocol/client/src/audio/queue.rs`, `client/protocol/client/src/session/mod.rs`, `client/protocol/client/src/http/capabilities.rs`, `client/protocol/client/tests/capabilities_contract.rs` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |
| **23-B: Nightjar Android Passthrough Unlock** | **Stage 2** (after 23-A; parallel with 23-C) | `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java`, `client/app/src/main/java/com/limelight/preferences/SettingRules.java`, `client/app/src/main/java/com/limelight/preferences/PreferenceConfiguration.java`, `client/app/src/test/java/com/limelight/preferences/SettingRulesTest.java` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |
| **23-C: Optional Host Third-Party `x-ss-audio-codec-accept` Interop** | **Stage 2** (parallel with 23-B) | `rust/core/src/rtsp.rs`, `rust/host/src/rtsp_server.rs` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |

---

## 7. Verification & Measurement Plan

1. **Deterministic Unit, Golden & Loopback Tests (No GPU Required):**
   - `cargo test -p nightjar-core --locked dolby_ss rtsp audio session_loopback golden_audio golden_fec` — verifies: (a) 2,588-byte unchunked 640 kbps AC-3 / E-AC-3 datagrams round-trip intact with zero truncation and zero `ChunkHeader` stripping; (b) `audio/queue.rs` Reed-Solomon FEC repairs lost shards at `>1400` B; (c) missing/`opus` `X-SS-Audio-Codec` falls back cleanly to Opus; (d) `golden_audio`, `golden_fec`, and Rubylight `a=x-rl-audio.codecs` chunked passthrough remain byte-for-byte unchanged!
   - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.preferences.SettingRulesTest --tests com.limelight.preferences.SettingsKeysTest`
   - `cargo test -p butterpollo-core --locked rtsp` & `cargo test -p butterpollo --locked rtsp` (if Slice 23-C included).
   - `python tools/arch_check.py` & `cargo fmt -- --check` (without `--all`).
