# Item 25: PlayStation Controller Speaker — Bluetooth DualSense HID (Track 4A) & Vibepollo `0x5602` / `0x5500_0009` Interop (Track 4B) (Wave 2 — PR 4)

Status: engineering design (Phase 1). Rebased 2026-11-25 onto post-refactoring `origin/main` `955ef39598` (`v6.4.1`) and [`00-protocol-reservations.md`](00-protocol-reservations.md).

## 1. Summary & User Experience

Item 25 delivers **PR 4 (Track 4A + Track 4B)** — completing PlayStation controller speaker audio across **both** Rubylight's native protocol over wireless Bluetooth DualSense controllers **and** Vibepollo's `0x5602` speaker PCM protocol with airtight `0x5500_0009` packet-disambiguation safeguards:

### 1.1 Track 4A — Rubylight <-> Nightjar Bluetooth DualSense Speaker (Desktop Windows & Linux)
- Renders DualSense controller speaker PCM received over Rubylight's existing `0x5533` (controller speaker) / `0x5535` (controller audio control) messages directly to Bluetooth-connected DualSense controllers on **Nightjar Desktop (Windows & Linux)** using Sony's **398-byte Bluetooth HID output reports** (carrying 480-frame 160 kbps Opus audio + 32-frame haptics + IEEE CRC-32 HID footer).
- **Licensing & Provenance Gate (`AGENTS.md`):** Before writing or adapting any Bluetooth DualSense HID framing from **SAxense** or **`Nonary/moonlight-qt` commit `b72d0c2`**, verify exact upstream license compatibility (GPL-3.0-compatible), keep full copyright/license notices in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README credits (NEVER in UI text or release notes), or implement independently from public HID documentation.
- **Unrooted Android Limit:** Clearly document in `docs/features.md` / `client/docs/CLIENT-PARITY.md` that unrooted Android blocks raw Bluetooth HID output report injection for DualSense speakers (USB-C remains supported on Android).

### 1.2 Track 4B — Vibepollo `0x5602` Controller Speaker PCM Interop (Strict `0x5500_0009` Disambiguation!)
- **Capability Advertisement & Arrival:**
  - Feature bit: `x-ml-general.featureFlags` bit **`0x08`**.
  - PlayStation-scoped arrival capability bit: **`0x4000` ONLY when `controller_type == 0x02` (PlayStation)**! On `controller_type == 0x04` (Steam Controller), `0x4000` **strictly retains** its existing meaning `LI_CCAP_STEAM_ATTRIBUTES`!
- **20-Byte `0x5500_0009` Controller Speaker Binding Packet (Client -> Host, Reliable BEFORE Controller Arrival):**
  ```text
  Offset  Len  Field
  0       4    BE32 declared_len = 16   (total packet = 20 bytes)
  4       4    LE32 magic        = 0x5500_0009
  8       2    LE16 slot         (< 16)
  10      2    LE16 version      = 1
  12      8    LE64 owner        (!= 0)
  ```
- **`0x5602` Speaker PCM Control Packet (Host -> Client, Unreliable ENet Channel `0x08`, Encrypted Protocol 13 ONLY):**
  - Fixed **992 bytes** total: **32-byte LE header** + **960 bytes PCM payload** (48 kHz mono/stereo frame chunk matching Vibepollo wire layout).
- **AIRTIGHT `0x5500_0009` Collision Safeguards (Non-Negotiable!):**
  1. **Host disambiguation by exact length (`rust/host/stream/src/stream/input.rs` / `input_packet`):**
     - `len == 20 && declared_len == 16` -> `ControllerSpeakerBinding` **ONLY** when `ml_features & 0x08 != 0` AND control channel is encrypted (proto 13)!
     - `len == 40 && declared_len == 36` -> Foundation `Touchpad` **ONLY** when `native_touchpad` is active!
     - Any other length or unnegotiated capability -> reject/ignore cleanly without corrupting input state.
  2. **Host `ensure_dualsense` (Windows gamepad router):** Attaches `usbip_ds5` for a bound PlayStation slot advertising `0x4000` even when `0x8000` (DualSense haptics) is absent.
  3. **Host audio/channel safety:** Enforce `speaker_allowed`, `PcmTraffic::observe`, `channels > haptics::CHANNEL` (ensuring ENet channel `0x08` is allocated), and **strict mutual exclusion** against Rubylight's native `0x5533`/`0x5535` path (never send both simultaneously on one session!).
  4. **Client mutual exclusion & Foundation touchpad guard:** Nightjar **NEVER** sends `0x5500_0009` binding, NEVER sets `x-ml-general.featureFlags & 0x08`, and NEVER advertises PlayStation `0x4000` if the host advertises `0x5533` in `a=x-rl-control` **OR** advertises Foundation touchpad `x-ss-general.featureFlags & 0x10`!

---

## 2. Protocol, Native API & Capability Contract

Registered explicitly in [`00-protocol-reservations.md`](00-protocol-reservations.md) and `client/protocol/core/src/ids.rs`:
- `CONTROL_IDS`: add `(0x5602, "PlayStation controller speaker PCM (Vibepollo)")` (right next to `0x5601`, outside `OTHER_CLIENTS_CONTROL_IDS = 0x5506..=0x5510` and `RUBYLIGHT_7_CONTROL_IDS = 0x5548..=0x554F`, bit-15 clear).
- `INPUT_MAGICS` / `RESERVED_INPUT_MAGICS`: register `0x5500_0009` cleanly (narrowing `RESERVED_INPUT_MAGICS` / `pen::RESERVED_MAGICS` to `[0x5500_000A]` or adding `ADOPTED_INPUT_MAGICS` parallel to `ADOPTED_CONTROL_IDS` so `input_magics_are_unique()` passes cleanly!), leaving `0x5500_000A` reserved and `0x5500_0081` untouched.
- `PcmTraffic::observe` accounting: pass `controller_audio::SPEAKER` (or map `0x5602 => 1` in `client/protocol/wire/src/controller_audio.rs`) so `0x5602` speaker traffic is counted identically to `0x5533` (mutually exclusive per session), and run `0x0206` pre-decode session-gated disambiguation in `rust/host/stream/src/stream/control.rs` ahead of `rust/core/input-policy/src/input.rs`.
- `PROTOCOL_VERSION` and `rubylight_native_api::API_VERSION = 1` remain untouched.

---

## 3. Architecture & <=800-LOC Module Plan

All target modules below are conceptual anchors — **resolve exact post-sprint file paths, split submodules, and `tools/size_ratchet.json` ceilings live via `docs/code-map.md` and `client/docs/code-map.md` before assigning implementer slices** (`docs/code-map.md & client/docs/code-map.md`). Every new `.rs` module stays strictly `<= 800` LOC with `//!` headers; grandfathered files MUST NOT grow past their live post-sprint ceilings.

| Path | Status | Est. LOC | Responsibility |
| --- | --- | ---: | --- |
| `client/protocol/core/src/ps_speaker_pcm.rs` | New | ~240 | `//!` `no_std + alloc` wire codec for 20-byte `0x5500_0009` `ControllerSpeakerBinding` (rejecting non-20-byte, `slot >= 16`, `version != 1`, `owner == 0`) and 992-byte `0x5602` PCM header+payload + property/fuzz tests. |
| `client/protocol/core/src/ids.rs`, `lib.rs`, `fuzz.rs` | Modify | `<= ceiling` | Register `0x5602` & `0x5500_0009` length-disambiguated contract; extend `fuzz.rs`. |
| `client/protocol/desktop/src/dualsense_bt_audio.rs` | New | ~420 | `//!` Track 4A Desktop Windows/Linux 398-byte Bluetooth DualSense HID speaker encoder (480-frame 160 kbps Opus + 32-frame haptics + IEEE CRC-32) with verified upstream license header. |
| `client/protocol/desktop/src/controller_audio.rs` | Modify | `<= 780` | Dispatch `0x5533`/`0x5535` and `0x5602` PCM to USB audio or `dualsense_bt_audio.rs` cleanly. |
| `client/protocol/client/src/control/ps_speaker.rs` | New | ~220 | `//!` Track 4B client state machine enforcing strict mutual exclusion (never sends `0x5500_0009` or PS `0x4000`/`0x08` if `0x5533` in `a=x-rl-control` OR Foundation `0x10` flag is set), sending 20-byte binding before arrival and receiving `0x5602` on ENet channel `0x08`. |
| `client/protocol/client/src/control/{messages,events,driver}.rs`, `rtsp/mod.rs` | Modify | `<= ceiling` | Wire `ps_speaker.rs` and ENet channel `0x08` unreliable receive. |
| `rust/host/stream/src/stream/ps_speaker_interop.rs` | New | ~280 | `//!` Host Track 4B disambiguator (`len==20 && declared==16` vs `len==40 && declared==36`), `speaker_allowed`, `PcmTraffic::observe`, `channels > haptics::CHANNEL`, and mutual exclusion against `0x5533`/`0x5535`. |
| `rust/host/stream/src/stream/{control.rs,input.rs}`, `rust/windows/io/src/input/gamepads.rs`, `tests/host/tests/reconnect_tests/ps_speaker_interop_tests.rs` (new) | Modify / New | `<= ceiling` / ~280 | Wire `ps_speaker_interop.rs` + `ensure_dualsense` for bound PS `0x4000` (without `0x8000`) + full interop tests; update `NOTICE` & `rust/THIRD_PARTY.md`. |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

- **Negotiation Mutual Exclusion:**
  - Host offers `0x5533` -> **Rubylight Native Path (`0x5533`/`0x5535`)** -> supports BOTH USB and Bluetooth DualSense speaker (Track 4A); client NEVER sends `0x5500_0009` or `0x08`.
  - Host omits `0x5533` AND omits Foundation `0x10` AND advertises `x-ml-general.featureFlags & 0x08` + encrypted proto 13 -> **Vibepollo `0x5602` Path (Track 4B)**: send 20-byte `0x5500_0009` reliably -> send PS arrival with `type == 0x02` & cap `0x4000` -> receive 992-byte `0x5602` on unreliable ENet channel `0x08`.
  - Host advertises Foundation `0x10` -> **Foundation Touchpad Only**: `0x5500_0009` stays exclusively 40-byte touchpad, `0x5602` disabled.
- **Crash-Restore:** Zero host OS state mutated; BT HID handle closed cleanly on stream end/disconnect.

---

## 5. Settings & 10/10 Craft Plan

- **Zero new settings** (reuses existing controller speaker toggle).
- **Strict upstream license hygiene (`AGENTS.md`):** Record SAxense / `Nonary/moonlight-qt` (`b72d0c2`) / Vibepollo attributions strictly in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README credits (never in UI text or release notes).

---

## 6. Disjoint Implementer Slices (Phase 3 `parallel()`)

| Slice | Stage | Exclusive Owned Files (Zero Overlap) |
| --- | --- | --- |
| **25-A: `rubylight-protocol` Wire Codec, `ids.rs` & Fuzz** | **Stage 1** | `client/protocol/core/src/ps_speaker_pcm.rs` (new), `client/protocol/core/src/ids.rs`, `client/protocol/core/src/lib.rs`, `client/protocol/core/src/fuzz.rs` `docs/code-map.md & client/docs/code-map.md` |
| **25-B: Desktop Bluetooth DualSense 398-Byte HID Speaker (Track 4A)** | **Stage 2** (parallel with 25-C, 25-D) | `client/protocol/desktop/src/dualsense_bt_audio.rs` (new), `client/protocol/desktop/src/controller_audio.rs`, `NOTICE`, `rust/THIRD_PARTY.md` `docs/code-map.md & client/docs/code-map.md` |
| **25-C: Nightjar Client `0x5602` / `0x5500_0009` Interop (Track 4B Client)** | **Stage 2** (parallel with 25-B, 25-D) | `client/protocol/client/src/control/ps_speaker.rs` (new), `client/protocol/client/src/control/messages.rs`, `client/protocol/client/src/control/events.rs`, `client/protocol/client/src/control/driver.rs`, `client/protocol/client/src/rtsp/mod.rs` `docs/code-map.md & client/docs/code-map.md` |
| **25-D: Host `0x5602` / `0x5500_0009` Disambiguator, `ensure_dualsense` & Interop Tests (Track 4B Host)** | **Stage 2** (parallel with 25-B, 25-C) | `rust/host/stream/src/stream/ps_speaker_interop.rs` (new), `rust/host/stream/src/stream/control.rs`, `rust/host/stream/src/stream/input.rs`, `rust/windows/io/src/input/gamepads.rs`, `tests/host/tests/reconnect_tests/ps_speaker_interop_tests.rs` (new) `docs/code-map.md & client/docs/code-map.md` |

---

## 7. Verification & Measurement Plan

1. **Automated Unit, Fuzz & Interop Tests (No Hardware Required):**
   - `cargo test -p rubylight-protocol --locked ps_speaker_pcm ids fuzz` — verifies `0x5602` uniqueness, `0x5500_0009` 20-byte encoding/validation (`slot < 16`, `version == 1`, `owner != 0`), and 992-byte `0x5602` frame codec.
   - `cargo test -p butterpollo --locked ps_speaker_interop` — verifies exact 20-byte (`declared==16`) vs 40-byte (`declared==36`) disambiguation, encryption & `ml_features & 0x08` gating, `ensure_dualsense` with PS `0x4000` without `0x8000`, `PcmTraffic::observe`, `channels > haptics::CHANNEL`, and mutual exclusion vs `0x5533`/`0x5535`.
   - `cargo test -p nightjar-core --locked ps_speaker` & `cargo test -p nightjar-desktop --locked dualsense_bt_audio` — verifies client NEVER emits `0x5500_0009`/`0x08`/`0x4000` when `0x5533` OR Foundation `0x10` is present, PS `0x4000` never collides with Steam `0x04` `LI_CCAP_STEAM_ATTRIBUTES`, and 398-byte BT HID report + CRC-32 packing passes golden vector checks.
   - `python tools/arch_check.py` & `cargo fmt -- --check` (without `--all`).
