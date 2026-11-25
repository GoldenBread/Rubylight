# Item 10: Couch Party Audio Routing & Spatial Mix

Status: design, not implemented (Wave 2 — Rubylight & Nightjar 7.0.0). Rebased 2026-11-25 onto post-refactoring `origin/main` `955ef39598` (`v6.4.1`), `docs/designs/00-protocol-reservations.md`, and `tools/size_ratchet.json`.

## 1. Summary & User Experience

Rubylight already lets multiple paired devices share a game (`rust/host/stream/src/multiclient.rs`), runs per-stream audio DSP (`rust/host/stream/src/stream/couch_audio.rs`, `rust/core/media-policy/src/audio_dsp/mod.rs`), isolates multi-instance game sound (`rust/host/stream/src/stream/instance_audio.rs`), and receives client microphones (`rust/host/stream/src/mic.rs`). Today, however, every guest receives the same unmixed game audio, there is no way for a player wearing headphones on a handheld to hear a couch partner's mic without self-echo or drowning out the game, and switching between "muted PC speakers" and "living-room TV speakers + headphone guest simultaneously" requires stopping the stream and editing host settings.

**Couch Party Audio** brings live, zero-dropout per-stream mixing and couch speaker control to multi-device sessions:

- **Per-stream live mix:** Each active device (owner or guest) adjusts its own **Game volume** (0–100%), **Voice volume** (0–200%), **Voice ducking** (Off / Subtle -6 dB / Clear -12 dB when another player speaks), and **Per-guest mute & stereo balance** (so Player 2's mic can sit gently left and Player 3's gently right in headphones without muting their game!).
- **Self-echo prevention:** A device's own microphone slot is automatically excluded from its own party voice bus so players never hear their own delayed voice.
- **Live PC speaker output mode (Owner / Host control):** Toggle between **Headphones / Remote only** (mute PC speakers via virtual sink) and **Couch speakers + Remote** (simultaneous local PC/TV playback AND streamed headphone mix) without restarting the game or dropping audio.
- **Zero audio-thread allocations or locks:** All mix sliders, mute bits, and voice-activity envelopes travel through lock-free atomics and pre-allocated ring buffers into `audio_dsp::Chain`, keeping single-player latency at **0.00 ms added** (single atomic fast-path bypass when no party mix is active).
- **Crash-safe Windows audio restore:** Every host-speaker mute or mirror-endpoint mutation is journaled to `audio-recovery.json` *before* Windows COM calls execute, and restored cleanly on stream end, last guest departure, or crash recovery.

## 2. Protocol, Native API & Capability Contract

### Compatibility Invariants

1. **`PROTOCOL_VERSION` stays unchanged.** Stock Moonlight clients and clients not sending `rlPartyAudio=1` receive the exact existing Opus/PCM stream and see zero new packets (`tests/host/tests/reconnect_tests/extension_tests.rs`).
2. **`rubylight_native_api::API_VERSION` remains `1`.** Strictly additive `audio.party` feature and routes.
3. **Capability-gated, never brand-gated.** Nightjar enables Party Audio controls strictly when `HostCapabilities::has(Feature::PartyAudio)` is true (proven by `PartyAudioVersion >= 1` AND native `audio.party`). Never gate on `Family::Rubylight`.
4. **No third-party mod names on wire or UI:** Per GPL-3.0 §7(e) and `AGENTS.md`, UI strings, Native API labels, and Nightjar never name any external co-op or frame-generation mod.

### Reservations (from `00-protocol-reservations.md`)

| Surface | Constant / Key | Value |
| --- | --- | --- |
| Control Message ID | `party_audio::PARTY_AUDIO_CONTROL_MESSAGE_TYPE` (`client/protocol/core/src/ids.rs` in `RUBYLIGHT_7_CONTROL_IDS = 0x5548..=0x554F`) | `0x554C` (bidirectional, reliable ENet) |
| `/serverinfo` Element | `party_audio::SERVERINFO_FIELD` (`rust/core/media-policy/src/extensions.rs` `Offered::party_audio`) | `PartyAudioVersion = 1` |
| RTSP `DESCRIBE` Line | `a=x-rl-control:0x554C` | Emitted only when `Offered::party_audio` is true |
| Launch / `ANNOUNCE` Opt-In | `rlPartyAudio=1` | Client opts into `0x554C` state pushes & live mix |
| Native API v1 Feature | `feature::AUDIO_PARTY` | `"audio.party"` |
| Native API v1 Routes | `GET /api/native/v1/audio/party`, `POST /api/native/v1/audio/party` | `view` or active guest (read); `launch` (owner/party-wide) or active guest (own mix slot only) |
| Nightjar `HostCapabilities` | Rust `Feature::PartyAudio` / Java `HostCapabilities.Feature.PARTY_AUDIO` | `PartyAudioVersion >= 1` and native `audio.party` |

### Control Message `0x554C` Binary Wire Format (`client/protocol/core/src/party_audio.rs`, `no_std`)

Little-endian, versioned header (`version = 1`), zero heap allocations to decode fixed command frames:

| Offset | Size | Field | Semantics |
| --- | --- | --- | --- |
| 0 | `u8` | `version` | `1` (unknown version ignored/rejected cleanly) |
| 1 | `u8` | `opcode` | `1` = `SetOwnMix` (C→H), `2` = `SetGuestSlot` (C→H), `3` = `SetHostOutput` (C→H, owner only), `4` = `StateSync` (H→C snapshot) |
| 2 | `u16` | `flags` | Bit 0: `muted`, Bit 1: `ducking_enabled`, Bit 2: `host_couch_mirror`, Bit 3: `party_active`, Bits 4–15: reserved (`0`) |
| 4 | `u32` | `revision` | Monotonic party-mix revision |
| 8 | `u16` | `game_gain_q15` | `0..=32768` (`32768` = 1.00×) |
| 10 | `u16` | `voice_gain_q15` | `0..=65535` (`32768` = 1.00×, `65535` ≈ 2.00×) |
| 12 | `u16` | `duck_depth_q15` | `8192..=32768` (`32768` = off, `16422` = -6 dB, `8231` = -12 dB) |
| 14 | `i16` | `target_pan_q15` | `-32767..=32767` (for `SetGuestSlot`: stereo pan; `0` = center) |
| 16 | `u32` | `target_slot_id` | `0` for own stream; guest slot ID (`multiclient::GuestId`) for `SetGuestSlot` |
| 20 | `u8` | `slot_count` | For `StateSync` (`opcode = 4`), `0..=8` trailing 12-byte `SlotSummary` records: `[slot_id: u32, flags: u16 (muted/speaking/is_self/is_owner), gain_q15: u16, pan_q15: i16, reserved: u16]` |
| 21 | `[u8; 3]` | `reserved` | `0` |

- Fixed command size (`opcode` 1–3): **24 bytes** (`PARTY_AUDIO_CMD_BYTES = 24`).
- Max `StateSync` size (`opcode = 4`, 8 slots): **24 + 8 × 12 = 120 bytes** (well inside ENet single MTU).
- Client slider drags throttle `0x554C` sends to **≤ 20 Hz** (50 ms coalescing) so ENet control bandwidth remains negligible.

### Native API v1 Contract (`GET /audio/party`, `POST /audio/party`)

```json
{
  "revision": 12,
  "active": true,
  "caller_slot_id": 1,
  "caller_is_owner": true,
  "host_output": {
    "mode": "remote_only",
    "configurable": true,
    "endpoint_name": "Living Room TV (HDMI)",
    "reason": null,
    "message": null
  },
  "own_mix": {
    "game_volume_pct": 85,
    "voice_volume_pct": 120,
    "ducking": "subtle",
    "muted": false
  },
  "slots": [
    {
      "slot_id": 1,
      "device_name": "Living Room Handheld",
      "is_owner": true,
      "is_self": true,
      "mic_active": false,
      "speaking": false,
      "muted_for_me": false,
      "volume_pct": 100,
      "pan": 0
    },
    {
      "slot_id": 2,
      "device_name": "Alex's Phone",
      "is_owner": false,
      "is_self": false,
      "mic_active": true,
      "speaking": true,
      "muted_for_me": false,
      "volume_pct": 100,
      "pan": -35
    }
  ]
}
```

- `POST /api/native/v1/audio/party` (`#[serde(deny_unknown_fields)]`): accepts optional `own_mix`, `slot_updates[]` (`slot_id`, `muted_for_me`, `volume_pct`, `pan`), and `host_output_mode` (`"remote_only" | "couch_mirror"` — owner or `host_control` only; guest attempting to flip `host_output_mode` receives `409 not_session_owner`).
- If no audio stream is active for the caller: `GET` returns `active: false` with `reason: "no_session"`; `POST` returns `409 no_session`.
- If simultaneous couch mirror cannot open the PC's physical playback endpoint (e.g., HDMI TV disconnected or exclusive-mode game): host fails visibly (`host_output.configurable = false`, `reason = "endpoint_unavailable"`, session warning `party_audio_mirror` set, never silently muting remote streams).

## 3. Architecture & <=800-LOC Module Plan

### Real-Time Lock-Free Audio Pipeline

```
  [Game WASAPI Loopback]              [mic.rs UDP Opus Decode] (per speaking slot)
            │                                       │
            ▼ (5 ms / 240-frame f32 packet)         ▼ (lock-free SPSC ring per slot + AtomicU32 RMS)
  ┌─────────────────────────────────────────────────────────────────────┐
  │ audio thread (ZERO heap allocs, ZERO Mutex::lock, ZERO COM calls)  │
  │                                                                     │
  │  1. Surround (8ch→2ch HRIR, existing)                               │
  │  2. Fast-path check: party_atomics.active.load(Relaxed) == false?   │
  │     └─► YES: 0.00 ms overhead, skip straight to Night + Opus        │
  │  3. PartyMixer::process (in-place, pre-allocated 240-frame scratch):│
  │     • Read own_mix atomics (game_gain, voice_gain, duck_depth)      │
  │     • Pop non-self speaking slots from PartyVoiceBus rings          │
  │     • Smooth ducking envelope (15 ms attack, 250 ms release)        │
  │     • Apply ramped game_gain * duck_gain (10 ms clickless ramp)     │
  │     • Equal-power stereo pan + mix voice slots into L/R             │
  │  4. Existing Night compressor + soft_clip (prevents clipping)       │
  │  5. Optional non-blocking push to CouchMirrorRing (if mirror on)    │
  └──────────────────────────┬───────────────────────────┬──────────────┘
                             ▼                           ▼
                    [Opus Encode → UDP]        [party-audio-mirror thread]
                                               (WASAPI render to PC speakers,
                                                drops oldest frames on overrun;
                                                NEVER stalls `audio` thread!)
```

### Size-Ratchet Guardrail (`tools/size_ratchet.json`)

| Grandfathered / Near-Ceiling File | Ratchet / Current LOC | Strategy to Avoid Growth |
| --- | --- | --- |
| `rust/core/media-policy/src/audio_dsp/mod.rs` | 682 LOC (<800 ceiling!) | Keep all party DSP, equal-power pan math, ducking envelope, and `PartyControls` atomics in new `rust/core/media-policy/src/audio_dsp/party.rs` (<=450 LOC). `audio_dsp/mod.rs` adds only `pub mod party;` + 6-line hook in `Chain::process`. |
| `rust/host/stream/src/multiclient.rs` | 783 LOC (<800 ceiling!) | **Only 17 LOC headroom!** Do NOT add party logic here; emit slot join/leave via a 3-line call to `crate::stream::party_audio::sync_slots(&h)`. |
| `rust/host/stream/src/mic.rs` | 790 LOC (<800 ceiling!) | **Only 10 LOC headroom!** Extract helper `rust/host/stream/src/mic/party_tap.rs` (<=180 LOC); add only a 3-line call pushing decoded `self.pcm` into `h.party_audio.voice_bus()` after Opus decode. |
| `rust/windows/io/src/audio_route.rs` | 1310 LOC (ratchet) | Add `pub(crate)` accessor for journal path/endpoints (~5 lines); put couch-mirror WASAPI render + speaker mute-journal in new `rust/windows/io/src/audio_party_route.rs` (<=450 LOC). |
| `rust/host/stream/src/stream/control.rs` | 2675 LOC (ratchet) | 4-line match arm for `0x554C` delegating to `crate::stream::party_audio::handle_control(...)`. |
| `rust/host/web/src/native_api.rs` | 853 LOC (ratchet) | Add `mod party_audio;` + 2 routes (`/audio/party` GET/POST) + `feature::AUDIO_PARTY`. |
| `rust/host/stream/src/state.rs` | 2247 LOC (ratchet) | Add 1 field `pub party_audio: Arc<crate::stream::party_audio::PartyHub>`. |
| `client/app/src/main/java/com/limelight/ui/StreamMenu.java` | 1321 LOC (ratchet) | Add 1 entry row opening new `client/app/src/main/java/com/limelight/ui/PartyAudioSheet.java` (<=400 LOC). |
| `client/protocol/desktop-ui/src/ui.rs` | 1586 LOC (ratchet) | Render party audio controls via new `client/protocol/desktop-ui/src/ui/party_audio.rs` (<=320 LOC). |
| `client/protocol/desktop-config/src/strings/*.rs` | 1750 LOC (ratchet) | **Zero growth!** Route all new 7.0.0 Desktop translations to `client/protocol/desktop-config/src/strings_70/<code>.rs` (14 non-English locale tables; English literals live at call sites). |

### New & Touched Module Plan

| Path | Layer | Est. LOC | Responsibility |
| --- | --- | --- | --- |
| `client/protocol/core/src/party_audio.rs` (new) | `rubylight-protocol` (`no_std`) | ~220 | `PARTY_AUDIO_CONTROL_MESSAGE_TYPE` (`0x554C`), `PartyAudioCommand`, `PartyAudioState`, `SlotSummary`, encode/decode + fuzz targets |
| `client/protocol/core/src/ids.rs` | `rubylight-protocol` | +12 | Add `RUBYLIGHT_7_CONTROL_IDS = 0x5548..=0x554F`, register `0x554C`, keep `0x554F` free |
| `client/protocol/native-api/src/party_audio.rs` (new) | `rubylight-native-api` | ~180 | `PartyAudioState`, `OwnMix`, `SlotMix`, `HostOutputState`, `PartyAudioWrite`, `DuckingLevel` |
| `rust/core/media-policy/src/audio_dsp/party.rs` (new) | `butterpollo-core` | ~420 | Lock-free `PartyControls`, `PartyVoiceSlot` ring (fixed 8 slots × 8 × 240-frame ring = ~60 KiB static), `DuckEnvelope`, equal-power panner, `PartyMixer::process` (100% pure Rust, unit-tested without Windows) |
| `rust/windows/io/src/audio_party_route.rs` (new) | `butterpollo-windows` | ~420 | Dedicated `party-audio-mirror` WASAPI shared-mode render thread on PC's pre-stream physical speakers, `ISimpleAudioVolume` speaker mute/unmute, crash-restore journal extension (`party-audio-recovery.json` / `audio-recovery.json` hook) |
| `rust/host/stream/src/stream/party_audio.rs` (new) | `butterpollo` host | ~440 | `PartyHub` registry (slot assignment for owner + `multiclient` guests + isolated multi-instance sessions), `0x554C` control dispatcher, state broadcast on change, mirror thread lifecycle |
| `rust/host/stream/src/mic/party_tap.rs` (new) | `butterpollo` host | ~160 | Publishes decoded mic `f32` PCM + RMS VAD into `PartyHub` slot ring without locking `audio` threads |
| `rust/host/web/src/native_api/party_audio.rs` (new) | `butterpollo` host | ~320 | `GET`/`POST /audio/party`, permission & owner checks, couch-menu `pub(crate)` hooks |
| `client/protocol/client/src/control/party_audio.rs` (new) | `nightjar-core` | ~220 | Send/receive `0x554C` messages, 20 Hz slider coalescing, `Feature::PartyAudio` gating |
| `client/protocol/policy/src/party_audio.rs` (new) | `nightjar-policy` | ~220 | Gain curve (perceptual dB slider ↔ Q15), pan label formatting, owner vs. guest control enablement |
| `client/protocol/desktop-ui/src/ui/party_audio.rs` (new) | `nightjar-desktop` | ~300 | Stream Menu Party Audio panel (sliders, speaking indicator LED, guest mute/pan, PC speaker toggle) |
| `client/protocol/desktop-config/src/strings_70/<code>.rs` (14 locale files) | `nightjar-desktop` | ~35/locale | 7.0.0 Desktop translations for Party Audio UI across all 14 non-English locales (`de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`), keeping grandfathered `strings/*.rs` <= 1,750 LOC |
| `client/app/src/main/java/com/limelight/ui/PartyAudioSheet.java` (new) | `nightjar-android` | ~380 | Android phone/handheld/TV Stream Menu Party Audio page + JNI bindings in `client/protocol/android/src/party_audio.rs` (<=180 LOC) |

### Audio / Capture / GPU Compatibility

- **Zero video/encoder impact:** Works identically with AMD AMF, NVIDIA NVENC, PyroWave, D3D12, D3D11, WGC, Desktop Duplication, physical displays, and Windows 10 (including systems without Windows 11 Build 20348 per-process audio capture, where System audio loopback still feeds `CouchAudio` and `PartyMixer`!).
- **Multi-instance co-op (`instance_audio.rs`):** Each instance stream already gets its own `CouchAudio` instance and virtual-sink capture; registering each active stream's `PartyControls` in `PartyHub` automatically lets separate instance players share the voice ducking bus while keeping their own game audio isolated!

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### Party Audio Session State Machine

```
[SoloBypass] (1 active stream, host_output = default, no party overrides)
    │  (2nd device joins via multiclient / co-op OR owner changes Party mix / mirror)
    ▼
[PartyActive] ──(guest joins/leaves)──► [PartyActive] (atomic slot bitmask update + 0x554C StateSync)
    │                                         │
    │ (owner toggles host_output=couch_mirror)│ (mirror endpoint opens)
    ▼                                         ▼
[MirrorStarting] ──(journal written first!)──► [PartyActive + MirrorRunning]
    │ (WASAPI endpoint open fails)            │ (endpoint unplugged / invalidated)
    ▼                                         ▼
[MirrorFaulted] (loud warning `party_audio_mirror`, stays remote_only, journal cleaned)
    │
    │ (all streams end OR host shutdown)
    ▼
[Restoring] (stop mirror thread → restore PC speaker mute/default → clear journal) ──► [Idle]
```

### Reconnect & Edge-Case Matrix

| Scenario | Expected Behavior |
| --- | --- |
| **Single-player stream (default)** | `PartyControls::active` stays `false`; `audio` thread executes zero mixing work and adds 0.00 ms latency. |
| **Guest joins mid-stream** | `multiclient` allocates guest slot ID → `PartyHub` assigns lock-free `PartyVoiceSlot`, sets `active = true`, pushes `0x554C StateSync` to opted-in peers. Existing streams experience zero audio glitch (no re-init of WASAPI or Opus). |
| **Guest disconnects / kicked / timed out** | `PartyHub` clears slot's active bit atomically; `DuckEnvelope` releases smoothly over 250 ms back to 100% game volume; remaining streams receive updated `StateSync`. |
| **Owner reconnects / roams (`Continue here`)** | Owner's `PartyControls` live preferences are recalled in-memory (like `audio_dsp::Memory`) for the running game generation; host speaker mirror state persists cleanly without double-journaling. |
| **Guest tries to mute PC speakers or mute Owner for everyone** | `0x554C SetHostOutput` from non-owner is refused (logged + `StateSync` resyncs true state); `POST /audio/party` returns `409 not_session_owner`. Per-guest mute (`muted_for_me`) only affects the caller's own mix! |
| **Multiple guests speak simultaneously** | `mic.rs` continues its existing Windows virtual-mic handover for in-game VoIP, while `mic/party_tap.rs` decodes every authenticated party participant's voice packets into its own `PartyVoiceSlot` ring so couch party chat supports full simultaneous multi-talker mixing (up to 4 concurrent talkers summed with soft-clip protection)! |
| **NaN / Inf in capture or voice packet** | Flushed/zeroed inside `PartyMixer::process` before summation, reusing `audio_dsp::BAD_SAMPLES_WARNING`. |
| **PC HDMI TV unplugged while `couch_mirror` active** | `party-audio-mirror` thread catches `AUDCLNT_E_DEVICE_INVALIDATED`, closes render client, clears mirror flag, sets visible session warning `party_audio_mirror` ("PC speaker output stopped because the audio device disconnected; remote streams continue normally"), and keeps remote streams uninterrupted. |
| **Host process crashes / power loss** | On next boot, `audio_route::restore` reads `audio-recovery.json` (plus `party-audio-recovery.json`), restores Windows default playback/recording endpoints AND restores saved `ISimpleAudioVolume` mute/scalar state on the physical speaker endpoint before deleting the journal. |

### Crash-Restore Journal Schema (`party-audio-recovery.json`)

Written atomically via `butterpollo_core::state::write_json` **before** calling Windows `ISimpleAudioVolume::SetMute` or opening a mirror route that alters endpoint state:

```json
{
  "version": 1,
  "physical_endpoint_id": "{0.0.0.00000000}.{...}",
  "was_muted_before": false,
  "previous_scalar_volume": 0.72,
  "mirror_active": true,
  "written_unix_ms": 1795221800000
}
```

Restore invariants:
1. Never restore to a host-owned virtual endpoint (`Steam Streaming Speakers` / `Steam Streaming Microphone`, guarded by `butterpollo_core::audio_defaults::host_owned`).
2. Restore runs in `session_control::session_teardown` on last stream exit AND unconditionally during host startup before serving any launch.
3. If the saved endpoint is offline at boot (e.g., Bluetooth speaker off), keep the journal entry only if a host endpoint is still active, otherwise log a clear warning and clean up so subsequent sessions are not wedged.

## 5. Settings & 10/10 Craft Plan

- **Fewer-settings rule:** Zero new permanent client preference keys (`SettingsKeysTest` untouched) and **at most one** optional host console toggle (`party_audio_enabled`, default `true` in `config.rs`, grouped under Audio in `rust/web/src/lib/schema/` — or folded under existing Guest/Audio settings if no separate switch is needed; live mix values live per-session on the host, matching `audio_dsp::Memory`!).
  - Host console two-sentence help (if exposed): *"Lets each connected device adjust its own game volume, voice balance, and ducking while sharing a session. Adds no delay when only one device is streaming; turn off only if you want all devices to hear the exact host mix."*
- **Default-off DSP gate until measured:** Voice ducking defaults to **Off** (`ducking = "off"`) and `host_output` defaults to **Remote only** (existing host behavior) until the player explicitly enables them in the Party Audio menu. Zero behavior change for existing users.
- **Plain-English error states:**
  - Mirror unavailable: *"Could not play on PC speakers while streaming (device busy or disconnected). Headphones and remote devices continue normally."*
  - Guest attempting host-speaker switch: *"Only the device hosting this session can change PC speaker output."*
  - No mic on guest: *"No microphone active"* (subtle muted icon, never an alarming red error).
- **Dynamic input glyphs & zero stream-time blur:**
  - Android `PartyAudioSheet` and Desktop `ui/party_audio.rs` render as solid/tinted crisp cards inside the Stream Menu with **zero backdrop blur** (zero extra GPU passes during active video decode/present!).
  - Gamepad D-pad left/right adjusts sliders in 5% increments; face button glyphs (Xbox/PS/Nintendo/Keyboard) update dynamically from the active input device.
- **Zero `"Experimental"`, zero mod names, 15 locales:** All strings across Desktop (English literals at call sites + all 14 translated tables routed to `client/protocol/desktop-config/src/strings_70/<code>.rs`, never growing grandfathered `strings/*.rs` past 1,750 LOC) and Android (`values*/strings.xml`) translated in all 15 locales; host console labels in `rust/assets/locale/`.

## 6. Disjoint Implementer Slices

| Slice | Owner Scope | Exclusive File List |
| --- | --- | --- |
| **Slice 10-A: Protocol, Lockstep Capabilities, Native API Types & Lock-Free DSP** *(Stage 1, parallel with 10-B)* | `0x554C` wire format, `ids.rs`, `extensions.rs`, native-api types, lock-free `audio_dsp/party.rs`, shared `nightjar-policy/party_audio.rs`, and all 4 lockstep `HostCapabilities` contract files | `client/protocol/core/src/party_audio.rs` (new), `client/protocol/core/src/ids.rs`, `client/protocol/core/src/lib.rs`, `client/protocol/core/src/fuzz.rs`, `rust/core/media-policy/src/extensions.rs`, `client/protocol/native-api/src/party_audio.rs` (new), `client/protocol/native-api/src/lib.rs`, `rust/core/media-policy/src/audio_dsp/party.rs` (new), `rust/core/media-policy/src/audio_dsp/mod.rs`, `client/protocol/policy/src/party_audio.rs` (new), `client/protocol/policy/src/lib.rs`, `client/protocol/policy/src/http/capabilities.rs`, `client/protocol/client/tests/capabilities_contract.rs`, `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java`, `client/app/src/test/resources/protocol/capabilities/*` |
| **Slice 10-B: Windows Mirror & Crash Journal** *(Stage 1, parallel with 10-A; lands before 10-C)* | WASAPI shared-mode couch speaker mirror thread, volume mute guard, crash-restore journal | `rust/windows/io/src/audio_party_route.rs` (new), `rust/windows/src/lib.rs`, `rust/windows/io/src/audio_route.rs` |
| **Slice 10-C: Host Hub, Control `0x554C`, Mic Tap & Native Routes** *(Stage 2, parallel with 10-D/10-E)* | `PartyHub`, `couch_audio` wiring, `mic/party_tap.rs`, `native_api/party_audio.rs`, reconnect/interop tests | `rust/host/stream/src/stream/party_audio.rs` (new), `rust/host/stream/src/stream/couch_audio.rs`, `rust/host/stream/src/stream/control.rs`, `rust/host/stream/src/mic/party_tap.rs` (new), `rust/host/stream/src/mic.rs`, `rust/host/stream/src/multiclient.rs`, `rust/host/web/src/native_api/party_audio.rs` (new), `rust/host/web/src/native_api.rs`, `rust/host/stream/src/state.rs`, `rust/host/stream/src/extensions.rs`, `tests/host/tests/nvhttp_parity/native_tests/party_audio.rs` (new), `tests/host/tests/reconnect_tests/party_audio_tests.rs` (new), `docs/control-messages.md`, `docs/native-api.md` |
| **Slice 10-D: Nightjar Core & Desktop UI** *(Stage 2, parallel with 10-C/10-E)* | `0x554C` control client, Native API calls, Desktop Stream Menu panel, 15 locales | `client/protocol/client/src/control/party_audio.rs` (new), `client/protocol/client/src/control/messages.rs`, `client/protocol/client/src/control/events.rs`, `client/protocol/client/tests/control_messages_contract.rs`, `client/protocol/desktop-ui/src/ui/party_audio.rs` (new), `client/protocol/desktop-ui/src/ui.rs`, `client/protocol/desktop-media/src/stream/mod.rs`, `client/protocol/desktop-config/src/strings_70/<code>.rs` (14 non-English locale tables; never growing grandfathered `strings/*.rs` past 1,750 LOC) |
| **Slice 10-E: Nightjar Android UI & JNI** *(Stage 2, parallel with 10-C/10-D)* | JNI `party_audio.rs`, `PartyAudioSheet.java`, `StreamMenu.java` hook, 15 `strings.xml` | `client/protocol/android/src/party_audio.rs` (new), `client/protocol/android/src/lib.rs`, `client/app/src/main/java/com/limelight/ui/PartyAudioSheet.java` (new), `client/app/src/main/java/com/limelight/ui/StreamMenu.java`, `client/app/src/main/res/values*/strings.xml`, `client/app/src/test/java/com/limelight/ui/PartyAudioSheetTest.java` (new) |

## 7. Verification & Measurement Plan

### Narrowest Unit, Fuzz & Interop Tests

1. **Wire & Fuzz (no GPU):**
   - `cargo test -p rubylight-protocol --locked party_audio ids` — asserts `0x554C` uniqueness, bit-15 clear, `0x554F` still free, round-trip of all 4 opcodes, malformed-length rejection.
   - `cargo test -p rubylight-protocol --locked fuzz` (`PROPTEST_CASES=2048`) — zero panics on arbitrary bytes for `0x554C`.
2. **Lock-Free DSP Proof (no GPU):**
   - `cargo test -p butterpollo-core --locked audio_dsp::party`:
     - **Bit-identical bypass:** with `active == false`, output samples are bit-for-bit identical to existing `Chain::process`.
     - **Self-echo exclusion:** slot 1 speaking produces 0.0 voice energy in slot 1's own output and full energy in slot 2's output.
     - **Ducking attack/release & click-free ramp:** verify smooth monotonic ramp over 480 frames with no sample-to-sample step > 0.02 on step gain change; verify -6 dB / -12 dB attenuation within ±0.2 dB.
     - **Zero-allocation assertion:** custom counting allocator test around `PartyMixer::process` asserts `alloc_count == 0` across 10,000 consecutive 5 ms packets!
3. **Host Control, Permissions & Crash-Journal Tests (GPU-free loopback):**
   - `cargo test -p butterpollo --locked party_audio` (`reconnect_tests::party_audio_tests` + `native_tests::party_audio`):
     - Guest can adjust own mix and `muted_for_me`, cannot toggle `host_output` (`409 not_session_owner`).
     - Stock Moonlight client (no `rlPartyAudio=1`) sees zero `0x554C` frames.
     - Simulated crash leaves `party-audio-recovery.json`; next `restore()` restores mute state and deletes journal without touching `Steam Streaming Speakers`.
4. **Cross-client Contract & Architecture:**
   - `cargo test -p nightjar-core --locked control_messages_contract capabilities_contract`.
   - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.ui.PartyAudioSheetTest --tests com.limelight.preferences.SettingsKeysTest`.
   - `python tools/arch_check.py` (strictly verifies `audio_dsp/mod.rs`, `multiclient.rs`, and `mic.rs` stay <= 800 LOC, and grandfathered `client/protocol/desktop-config/src/strings/*.rs` stay <= 1,750 LOC!).

### Visual Screenshot Checks

- **Desktop:** `client/protocol/desktop-ui/src/ui/tests.rs` + `shot.rs` captures Party Audio panel with 1 owner + 3 guests (one actively speaking, one muted, one panned left) across 1280×720 and 800×480 handheld viewport.
- **Android:** Robolectric/emulator screenshots of `PartyAudioSheet` in phone landscape (both standard & reverse), handheld, and TV D-pad focus states, verifying zero backdrop blur and dynamic button glyphs.

### Serialized Idle-Gated Hardware Criteria

- Pre-check: `(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` MUST show `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0`, user idle.
- Isolated test host (`butterpollo.exe --config-dir C:\tests\party-audio --port 48123 --bind 127.0.0.1`) with `audio_cadence_trace = true`:
  - **Solo bypass timing:** 90 s 1080p60 stream ABBA comparison — audio packet interval jitter p99 delta <= +0.05 ms; zero cadence dropouts.
  - **2-client + active mic + couch mirror soak (120 s):** zero WASAPI underruns on remote streams, `audio` thread CPU time per 5 ms packet < 0.15 ms p99, clean Windows speaker mute restore verified after killing test host via `Stop-Process -Force` and restarting!
- **Exit code 3 abort rule:** If `e2e.py` or `check.ps1` exits with code `3` (AMD encoder hang), abort immediately and investigate — never retry.

---

## 8. Post-Discovery & Post-6.4.1 Addendum — Event-Driven Audio Hot-Plug & Android BT A2DP / Direct Playback

> **Already Shipped in `6.4.1` (Do NOT Redo!):**
> - **Android 5.1/7.1/7.1.4 low-latency AAudio (`#156`, `0975ddb505`):** Already shipped on `origin/main` (`AudioBufferPolicy.useAAudio()` API 32+ surround + positional channel masks). Preserve intact!
> - **Host non-fatal surround fallback (`12 -> 8 -> 6`) & format-switch rollback (`cf9131b14c`):** Already shipped in `rust/windows/io/src/audio_route/`. Preserve intact!

- **Windows Event-Driven `IMMNotificationClient` Hot-Plug & Dead-Sink Recovery (Slice 10-B, `rust/windows/io/src/audio_route/`):**
  1. Implement an `IMMNotificationClient` COM listener (`OnDefaultDeviceChanged`, `OnDeviceStateChanged`, `OnDeviceAdded`, `OnDeviceRemoved`, `OnPropertyValueChanged`) that immediately unparks `RouteUpkeep` (<20 ms recovery instead of waiting up to 1000 ms poll when Exclusive virtual display drops an HDMI monitor or a USB/BT headset plugs in).
  2. Verify `DEVICE_STATE_ACTIVE` on `AUDCLNT_E_DEVICE_INVALIDATED` in `capture_sink()` / `maintain_default()` so invalidated virtual or HDMI sinks re-acquire or fall back visibly instead of streaming silence forever, support headless virtual-sink-only acquisition, and recreate `KeepAlive` alongside `Loopback` on any endpoint or format change (`48 <-> 96 kHz`, `2ch <-> 7.1ch`).
  3. Unify `party-audio-recovery.json` into Item `17`'s canonical crash-recovery journal (`RecoveryJournalV2`) rather than maintaining fragmented audio recovery JSON files.
- **Android Bluetooth A2DP Voice-Quality Guard & API 33 Direct Playback Check (Slice 10-E):**
  1. Extend `MicRoute.java` (which already classifies `TYPE_BLE_HEADSET` for echo cancellation): when classic Bluetooth A2DP headphones are active without LE Audio, route `AudioRecord` via `setPreferredDevice(TYPE_BUILTIN_MIC)` so enabling voice chat in Couch Party Audio **never** collapses 48 kHz stereo A2DP game audio into 8/16 kHz mono HFP/SCO (while letting LE Audio gaming headsets use bidirectional mic cleanly, surfaced with a plain-English notice in `PartyAudioSheet`).
  2. Migrate `AudioPassthrough.java:101` on API 33+ from deprecated `AudioTrack.isDirectPlaybackSupported` to `AudioManager.getDirectPlaybackSupport`.
