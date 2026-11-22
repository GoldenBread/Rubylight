# Protocol ID, Capability & Native API Reservations (Rubylight & Nightjar 7.0.0)

Status: design reservation table (Phase 0 contract). Written against `main` `b3d0c26b9b` (`6.2.0`).

This single document reserves every new control-stream message ID, input packet magic, `/serverinfo` element, RTSP `DESCRIBE` / `ANNOUNCE` attribute, launch parameter, `rubylight-native-api` v1 feature string, HTTP route, and `HostCapabilities::Feature` variant across Waves 1–3 of the 7.0.0 roadmap. Parallel implementer branches MUST NOT allocate IDs outside this table.

## 1. Non-Negotiable Compatibility Invariants

1. **`PROTOCOL_VERSION` stays unchanged.** Stock Moonlight clients and older Nightjar builds negotiate and stream byte-for-byte as before; any client that opts into nothing new sees nothing new (`rust/host/src/stream/reconnect_tests/extension_tests.rs`).
2. **`rubylight_native_api::API_VERSION` stays `1`.** All native API changes are strictly additive: new endpoints under `/api/native/v1`, new optional response fields (`#[serde(default)]`), `#[serde(deny_unknown_fields)]` on request bodies, and `Unknown` catch-all enum variants. The existing `ErrorBody { error: ApiError { code, message, setting, retry_after_ms } }` envelope in `rust/core/native-api/src/error.rs` is preserved intact (any new fields are optional additions inside `ApiError`).
3. **Capability-gated, never brand-gated.** Nightjar gates every feature strictly via `HostCapabilities::has(Feature::...)` (both Rust `client/protocol/client/src/http/capabilities.rs` and Java `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java`), never on `Family::Rubylight` or version strings (`tools/arch_check.py` enforces this).
4. **Protocol changes land with their client side** in the same change (`rubylight-protocol`, `nightjar-core`, `nightjar-desktop`, `nightjar-android` / Java, plus contract tests).
5. **No collisions with reserved/foreign ranges** in `client/protocol/core/src/ids.rs`:
   - Existing `RUBYLIGHT_3_CONTROL_IDS = 0x5539..=0x5546` (`0x5542` remains reserved for cursor position), followed by `0x5547` (`transport_feedback::TRANSPORT_FEEDBACK_MESSAGE_TYPE`).
   - Foreign client range: `0x5506..=0x5510` (untouched; only adopted `0x5508`, `0x5510`).
   - DualSense haptics: `0x5601`; capability bit mask `HAPTICS_CAPABILITY = 0x8000` (all reserved IDs keep bit 15 clear).
   - Existing input magics up to `0x5500_0080` (`CONTROLLER_IDENTITY_MAGIC`), reserved touchpad magics `[0x5500_0009, 0x5500_000A]`.
6. **Per-route body limits**: `rust/host/src/native_api.rs` enforces a router-wide `DefaultBodyLimit::max(16 * 1024)` (16 KiB). Routes accepting larger JSON payloads (`PUT /hub/profiles/{id}` and `PUT /input/decks/{id}`, up to 64 KiB) MUST attach an explicit per-route `.layer(DefaultBodyLimit::disable())` and read the body via `axum::body::to_bytes(req.into_body(), 64 * 1024)`, rejecting oversize payloads with structured JSON `ErrorBody` (`ErrorCode::TooLarge`, `"too_large"`, HTTP `413`) instead of Axum's plain-text `LengthLimitError`.

## 2. Control-Stream Message IDs (`client/protocol/core/src/ids.rs`)

Add `pub const RUBYLIGHT_7_CONTROL_IDS: RangeInclusive<u16> = 0x5548..=0x554F;` (with `0x5548..=0x554E` active and `0x554F` in `RESERVED_CONTROL_IDS`) and extend `ids.rs` uniqueness/range tests.

| ID | Constant & Module (`client/protocol/core/src/`) | Item | Direction | Delivery | Summary |
| --- | --- | --- | --- | --- | --- |
| `0x5548` | `launch_curtain::LAUNCH_CURTAIN_STATE_MESSAGE_TYPE` | 03 `launch-curtain` | Host → Client | Reliable | Launch progress phase, launcher prompt metadata, curtain lift / timeout / error reason |
| `0x5549` | `frame_warp::FRAME_WARP_V2_META_MESSAGE_TYPE` | 04 `frame-warp-2` | Host → Client | Unreliable | Frame Warp 2.0 per-frame camera velocity, confidence, gyro-assisted warp & HUD depth hints |
| `0x554A` | `stream_dvr::DVR_STATE_MESSAGE_TYPE` | 05 `stream-dvr` | Bidirectional | Reliable | Instant-replay clip save / manual record toggle and host DVR ring status / clip result |
| `0x554B` | `quick_suspend::QUICK_SUSPEND_MESSAGE_TYPE` | 08 `quick-suspend` | Bidirectional | Reliable | Freeze / unfreeze active game process tree, state confirmation, refusal reason |
| `0x554C` | `party_audio::PARTY_AUDIO_CONTROL_MESSAGE_TYPE` | 10 `couch-party-audio` | Bidirectional | Reliable | Live per-guest audio mix, voice ducking, and local/remote speaker balance |
| `0x554D` | `input_deck::INPUT_DECK_ACTION_MESSAGE_TYPE` | 13 `client-input-deck` | Bidirectional | Reliable | Radial/chord action trigger, virtual pad slot profile switch, active layer sync |
| `0x554E` | `coherence::COHERENCE_WINDOW_MESSAGE_TYPE` | 14 `coherence-workstation` | Bidirectional | Reliable | Multi-window list, active window focus/bounds/DPI change, close/minimize command |
| `0x554F` | *(Reserved in `RESERVED_CONTROL_IDS`)* | 7.0.x headroom | — | — | Kept free and asserted unused in `ids.rs::tests::reserved_ids_stay_free` |

## 3. Input Packet Magics (`client/protocol/core/src/ids.rs`)

| Magic (`u32` LE) | Constant & Module | Item | Direction | Summary |
| --- | --- | --- | --- | --- |
| `0x5500_0081` | `input_deck::INPUT_DECK_VIRTUAL_AXIS_MAGIC` | 13 `client-input-deck` | Client → Host | High-rate virtual analog stick / radial dial / gyro-mapped axis routed to `rubylight-vgamepad` VHF slots (no unsigned KMDF) |

## 4. `/serverinfo` Elements, RTSP `DESCRIBE` / `ANNOUNCE`, and Launch Parameters

All fields appear in `rust/core/src/extensions.rs` (`Offered`) only when the host has the capability enabled and ready.

| Item | `/serverinfo` Element | Value | `DESCRIBE` Line | Launch / `ANNOUNCE` Opt-In |
| --- | --- | --- | --- | --- |
| 03 `launch-curtain` | `LaunchCurtainVersion` | `1` | `a=x-rl-control:0x5548` | Launch `rlLaunchCurtain=1` |
| 04 `frame-warp-2` | `FrameWarpVersion` | `2` *(v1 compat kept)* | `a=x-rl-control:0x5549` *(with `0x5543..0x5546`)* | `FrameWarpRequest` `version=2` (`0x5543`) |
| 05 `stream-dvr` | `StreamDvrVersion` | `1` | `a=x-rl-control:0x554A` | Control `0x554A` or Native API |
| 06 `hdr-sdr-tonemap` | `HdrToneMapVersion` | `1` | `a=x-rl-video.sdrToneMap:1` | Launch `rlSdrToneMap=auto\|bt2390\|off` |
| 08 `quick-suspend` | `QuickSuspendVersion` | `1` | `a=x-rl-control:0x554B` | Control `0x554B` or Native API |
| 09 `host-hub-sync` | *(via `NativeApiVersion`)* | `1` | *(none)* | Native API v1 `hub.sync` |
| 10 `couch-party-audio` | `PartyAudioVersion` | `1` | `a=x-rl-control:0x554C` | Launch `rlPartyAudio=1` + control `0x554C` |
| 11 `stream-doctor` | *(via `NativeApiVersion`)* | `1` | *(reuses `0x5536`, `0x5538`, `0x5539`, `0x553D`)* | Native API v1 `stream.doctor_v2` |
| 12 `phone-safe-area` | *(client-only)* | — | *(reuses existing `0x5532` reconfigure)* | None |
| 13 `client-input-deck` | `InputDeckVersion` | `1` | `a=x-rl-control:0x554D` | Launch `rlInputDeck=1` + control `0x554D` |
| 14 `coherence-workstation` | `CoherenceVersion` | `1` | `a=x-rl-control:0x554E` | Launch `rlCoherence=1` |
| 15 `away-wake-relay` | `AwayWakeRelayVersion` | `1` | *(none; `rubylight-traverse` peer relay)* | Native API v1 `wake.relay` |

## 5. Native API v1 Features & Routes (`rust/core/native-api/src/`)

All routes live under `/api/native/v1`, authenticate paired device TLS certificates, verify device permissions on every call, and use `rust/core/native-api/src/error.rs` status codes (`not_paired` → `403`, `permission_required` → `403`, `not_session_owner` → `409`, `not_writable` → `409`, `too_large` → `413`).

| Item | `feature::*` Constant & Wire String | Routes | Required Permission |
| --- | --- | --- | --- |
| 03 `launch-curtain` | `LAUNCH_CURTAIN = "session.launch_curtain"` | `GET /session/curtain`, `POST /session/curtain/dismiss` | `view` (read), `launch` (dismiss) |
| 05 `stream-dvr` | `STREAM_DVR = "stream.dvr"` | `GET /dvr`, `POST /dvr/clip`, `POST /dvr/record`, `GET /dvr/clips`, `DELETE /dvr/clips/{id}` | `view` (read), `launch` (clip/record), `host_control` (delete) |
| 06 `hdr-sdr-tonemap` | `STREAM_SETTINGS = "stream.settings"`, `HDR_SDR_TONEMAP = "video.sdr_tonemap"` | **New** `GET /stream/settings`, `POST /stream/settings` (`rust/core/native-api/src/stream.rs`, `rust/host/src/native_api/stream_settings.rs`) | `launch` |
| 08 `quick-suspend` | `QUICK_SUSPEND = "session.quick_suspend"` | `GET /session/suspend`, `POST /session/suspend`, `POST /session/resume` | `view` (read), `launch` (owner) or `host_control` |
| 09 `host-hub-sync` | `HUB_SYNC = "hub.sync"` | `GET /hub/snapshot`, `PUT /hub/profiles/{id}` *(64 KiB body limit)*, `DELETE /hub/profiles/{id}`, `GET /hub/covers/{app_id}` | `list` (read), `launch` (own device profiles) |
| 10 `couch-party-audio` | `AUDIO_PARTY = "audio.party"` | `GET /audio/party`, `POST /audio/party` | `view` (read), `launch` or active guest (own slot) |
| 11 `stream-doctor` | `STREAM_DOCTOR_V2 = "stream.doctor_v2"` | `GET /stream/doctor`, `POST /stream/doctor/apply` | `view` (read), `launch` (apply safe recommendation) |
| 13 `client-input-deck` | `INPUT_DECK = "input.deck"` | `GET /input/decks`, `PUT /input/decks/{id}` *(64 KiB body limit)*, `DELETE /input/decks/{id}`, `POST /input/decks/active` | `launch` (own device decks) |
| 14 `coherence-workstation` | `COHERENCE = "session.coherence"` | `GET /session/windows`, `POST /session/windows/select`, `POST /session/windows/action` | `view` (list), `launch` (select/action) |
| 15 `away-wake-relay` | `WAKE_RELAY = "wake.relay"` | `GET /wake/relay`, `POST /wake/relay/enroll`, `POST /wake/relay/wake` | `host_control` (enroll), `launch` (wake) |

## 6. Nightjar `HostCapabilities::Feature` Mapping

Every new capability is added in lockstep to `client/protocol/client/src/http/capabilities.rs` (`Feature`, `Feature::ALL`, `Evidence`, `with_native`) AND `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java`, backed by recorded fixtures in `client/app/src/test/resources/protocol/capabilities/` verified by `capabilities_contract.rs`.

| Item | Rust `Feature` / Java `HostCapabilities.Feature` | Proven By |
| --- | --- | --- |
| 03 | `LaunchCurtain` / `LAUNCH_CURTAIN` | `LaunchCurtainVersion >= 1` or native `session.launch_curtain` |
| 04 | `FrameWarpV2` / `FRAME_WARP_V2` | `FrameWarpVersion >= 2` |
| 05 | `StreamDvr` / `STREAM_DVR` | `StreamDvrVersion >= 1` and native `stream.dvr` |
| 06 | `HdrSdrToneMap` / `HDR_SDR_TONEMAP` | `HdrToneMapVersion >= 1` or native `video.sdr_tonemap` |
| 08 | `QuickSuspend` / `QUICK_SUSPEND` | `QuickSuspendVersion >= 1` and native `session.quick_suspend` |
| 09 | `HubSync` / `HUB_SYNC` | Native `hub.sync` |
| 10 | `PartyAudio` / `PARTY_AUDIO` | `PartyAudioVersion >= 1` and native `audio.party` |
| 11 | `StreamDoctorV2` / `STREAM_DOCTOR_V2` | Native `stream.doctor_v2` |
| 13 | `InputDeck` / `INPUT_DECK` | `InputDeckVersion >= 1` and native `input.deck` |
| 14 | `Coherence` / `COHERENCE` | `CoherenceVersion >= 1` and native `session.coherence` |
| 15 | `AwayWakeRelay` / `AWAY_WAKE_RELAY` | `AwayWakeRelayVersion >= 1` and native `wake.relay` |

## 7. Items Requiring Zero Wire Changes

- **Wave 0 (`01`, `02`)**, **Wave 1 (`07`, `21`)**, **Wave 2 (`12`)**, **Wave 4 (`16`, `17`)**, and **Wave 5 (`18`, `19`, `20`)** require zero new control message IDs or protocol changes (Wave 2 item `12` reuses existing `0x5532` reconfigure).
