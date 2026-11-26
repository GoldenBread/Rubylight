# Item 11: Stream Doctor 2.0 & Upscaler Guidance

Status: implemented on `RamazanKara/r70-11-stream-doctor`; see [§9 As built](#9-as-built-2026-11-26) for where the build narrows this design. Hardware runs pending (Wave 2 — Rubylight & Nightjar 7.0.0). Rebased 2026-11-25 onto post-refactoring `origin/main` `955ef39598` (`v6.4.1`), `docs/designs/00-protocol-reservations.md`, and `tools/size_ratchet.json`.

## 1. Summary & User Experience

Nightjar already has a solid first-generation Stream Doctor (`client/protocol/policy/src/health.rs`, `client/protocol/desktop-media/src/stream/doctor.rs`) that inspects client-side 30-second windows, periodic Wi-Fi stall rhythms, receive-buffer truncation, and offers a single reversible bitrate trial. Meanwhile, the Rubylight host independently tracks per-stage frame loss in `rust/core/media-policy/src/stutter_report.rs` (`capture`, `convert`, `encode`, `packetize`, `send`) and streams low-overhead telemetry over `0x5536` (host timing), `0x5538` (link notices), `0x5539` (FEC reports), and `0x553D` (pipeline frame rates). Today those two halves do not fuse into a single actionable diagnosis: players still see multi-paragraph technical text ("Host to network p95: 22.4 ms") and must manually guess whether to lower bitrate, tweak FEC, cap game FPS, or reduce stream resolution with a client upscaler.

**Stream Doctor 2.0** fuses host-side stage attribution with client-side `StreamHealth` into **plain-English, single-tap actionable remedies** via additive Native API v1 (`stream.doctor_v2`):

- **One clear diagnosis card (never jargon dumps):** Distinguishes **Wi-Fi jitter / bufferbloat**, **Host GPU / encoder overload**, **Client decode pressure**, **OS receive-buffer truncation**, and **Display pacing mismatch** in two plain sentences. Raw p95 numbers and stage breakdowns live behind a collapsed *Technical details* drawer for power users.
- **Single-tap reversible fixes (session-scoped with Undo):** Every remedy applies only to the current stream and records its exact prior state so a single **Undo** tap restores it cleanly (and never overwrites a manual change the player made afterwards):
  - *Lower bitrate (-20%)* (upgrades existing `BitrateTrial`)
  - *Enable automatic FEC recovery* (when `Feature::ClientFec` is supported)
  - *Step down render resolution + turn on spatial upscaling* (reuses `0x5532` reconfigure + client spatial upscaler!)
  - *Step down stream frame rate* (reuses `0x553C` / `0x5532` when supported)
  - *Switch frame-pacing mode* (client-local, instant)
- **Strict spatial-only upscaler guidance (HARD RULE):** Upscaler recommendations cover **ONLY SGSR 1**, **FSR 1**, and **MetalFX (macOS only)**. The retired temporal upscaler (**SGSR 2 / `sgsr2`**) is **strictly prohibited** — it MUST NEVER be recommended, offered, serialized as a remedy, or revived in any code path, test fixture, or UI string!
- **Graceful non-Rubylight support:** Against Sunshine, Apollo, or older Rubylight hosts without `stream.doctor_v2`, Stream Doctor 2.0 still presents the clean single-card UX using client-side `StreamHealth` + local/client remedies, without ever gating on host brand.

## 2. Protocol, Native API & Capability Contract

### Compatibility Invariants

1. **`PROTOCOL_VERSION` is unchanged** and **zero new control-stream message IDs** are allocated. Stream Doctor 2.0 reuses existing `0x5536` (`HOST_TIMING_MESSAGE_TYPE`), `0x5538` (`LINK_NOTICE_MESSAGE_TYPE`), `0x5539` (`FEC_REPORT_MESSAGE_TYPE`), `0x553D` (`PIPELINE_RATES_MESSAGE_TYPE`), `0x5532` (`RECONFIGURE_MESSAGE_TYPE`), and `0x553C` (`RATE_CHANGE_MESSAGE_TYPE`).
2. **`rubylight_native_api::API_VERSION` remains `1`.** Strictly additive feature `stream.doctor_v2` and two endpoints under `/api/native/v1`.
3. **Capability-gated, never brand-gated.** Nightjar checks `HostCapabilities::has(Feature::StreamDoctorV2)` (proven solely by native `stream.doctor_v2`), plus existing `Feature::RuntimeBitrate`, `Feature::ClientFec`, `Feature::VirtualDisplay`, and `Feature::HostTiming` for each remedy's eligibility. Never check `Family::Rubylight`.
4. **Fail visibly, never switch encoders silently:** If the host encoder (AMD AMF, NVIDIA NVENC, or PyroWave) is overloaded, Stream Doctor recommends reducing resolution, frame rate, or bitrate — it NEVER silently switches encoder backends or codecs behind the player's back.

### Feature & Route Mapping (from `00-protocol-reservations.md`)

| Surface | Symbol / Route | Permission / Proof |
| --- | --- | --- |
| Native API v1 Feature | `feature::STREAM_DOCTOR_V2 = "stream.doctor_v2"` | Always advertised by 7.0+ host |
| `GET /api/native/v1/stream/doctor` | Read live host stage diagnostics & eligible server remedies | `view` (`VIEW_STREAMS`) or `launch` (`LAUNCH_APPS`) |
| `POST /api/native/v1/stream/doctor/apply` | Apply or undo one reversible session-scoped host remedy | `launch` + active stream owner (`NotSessionOwner` 409 otherwise) |
| Nightjar `HostCapabilities` | Rust `Feature::StreamDoctorV2` / Java `HostCapabilities.Feature.STREAM_DOCTOR_V2` | Proven by native `stream.doctor_v2` |

### Wire Schemas

#### `GET /api/native/v1/stream/doctor` (`200 OK`)

```json
{
  "schema_version": 1,
  "revision": 9,
  "session_active": true,
  "caller_is_owner": true,
  "window_seconds": 30,
  "encoder": {
    "backend": "amf",
    "codec": "hevc",
    "ten_bit": false,
    "pyrowave": false,
    "packed_444": false,
    "width": 2560,
    "height": 1440,
    "fps": 120,
    "live_bitrate_kbps": 45000,
    "fec_mode": "fixed_20"
  },
  "host_stages": {
    "pictures": 1780,
    "late_pictures": 94,
    "dominant_stage": "encode",
    "lost_ms": {
      "capture": 18.2,
      "convert": 11.4,
      "encode": 642.5,
      "packetize": 4.1,
      "send": 22.0
    },
    "pipeline_rates_hz": {
      "source": 118.4,
      "capture": 118.4,
      "submit": 102.1,
      "output": 101.8,
      "repeat": 1.2
    },
    "link_rate_adaptations": 0,
    "unrecoverable_fec_blocks": 0
  },
  "active_trials": [
    {
      "trial_id": "t-1795221800-1",
      "remedy": "bitrate_stepdown",
      "applied_value": "36000",
      "previous_value": "45000",
      "can_undo": true
    }
  ],
  "eligible_remedies": [
    {
      "id": "bitrate_stepdown",
      "configurable": true,
      "target_kbps": 36000,
      "reason": null
    },
    {
      "id": "fec_auto",
      "configurable": true,
      "reason": null
    },
    {
      "id": "render_scale_stepdown",
      "configurable": true,
      "target_width": 1920,
      "target_height": 1080,
      "reason": null
    }
  ]
}
```

#### `POST /api/native/v1/stream/doctor/apply` (`#[serde(deny_unknown_fields)]`)

```json
{
  "action": "apply",
  "remedy": "fec_auto",
  "trial_id": null
}
```

- Undo payload: `{ "action": "undo", "remedy": "fec_auto", "trial_id": "t-1795221800-1" }`.
- Allowed `remedy` enum values: `"bitrate_stepdown" | "fec_auto" | "render_scale_stepdown"`.
- Errors:
  - `403 permission_required` (missing `launch`)
  - `409 no_session` / `409 not_session_owner`
  - `422 invalid_value` (unknown remedy or out-of-range params)
  - `422 not_writable` (e.g., `fec_auto` on PyroWave stream, or `bitrate_stepdown` already at `TRIAL_FLOOR_KBPS = 500`, or manual change replaced the active trial — carries plain-English `message` and `reason`!).

### Diagnosis Fusion & Spatial Upscaler Guidance Contract (HARD RULE)

Pure deterministic classifier in `client/protocol/policy/src/doctor_v2.rs`:

| Primary Root Cause | Fusion Signal (Host + Client) | One-Tap Reversible Remedies (Priority Order) |
| --- | --- | --- |
| **`ReceiveBufferShort`** | `ReceiveBuffer::short()` AND (`loss_percent > 0.5%` OR `unrecoverable_fec_blocks > 0`) without matching host `encode`/`convert` lag | 1. Lower bitrate (-20%) to shrink UDP bursts; 2. Platform hint (Linux `#[cfg(target_os = "linux")]`: exact `sudo sysctl -w net.core.rmem_max=...` copy button; Windows/macOS/Android: plain network advice) |
| **`WifiJitterBufferbloat`** | (`Finding::Network` OR `Finding::Bursts` OR `Finding::Periodic`) AND host `dominant_stage != Some(Encode | Convert | Capture)` (or host `send` clean) | 1. Enable automatic FEC (`fec_auto`, when `ClientFec` supported & not PyroWave); 2. Lower bitrate (-20%); 3. Plain advice for periodic Wi-Fi scans (`period_ms`) |
| **`HostGpuEncoderOverload`** | Host `dominant_stage` in `{Encode, Convert, Capture}` OR (`submit_hz < capture_hz * 0.92` on `0x553D`) OR (`Finding::Host` when host timing active) | 1. **Step down render resolution + enable spatial upscaler** (see upscaler table below); 2. Lower bitrate (-20%) / frame rate stepdown |
| **`ClientDecodePressure`** | `Finding::Decoder` OR `Finding::Heat` (`decode_ms >= frame_budget`, or `#80` packed 4:4:4 memory-copy hit) | 1. Turn off 4:4:4 packed chroma (if active); 2. **Step down stream resolution + enable spatial upscaler**; 3. Lower frame rate |
| **`DisplayPacingMismatch`** | `Finding::Presentation` (`display_p95_ms` high while decode, host, and network are clean) | 1. Switch client frame-pacing mode (session-scoped with Undo); 2. Match screen refresh rate hint |
| **`Healthy`** | `samples >= 3` and zero findings on both host and client | Calm green check ("Your stream is running smoothly") |

#### Allowed Upscaler Recommendation Matrix (**STRICTLY NO SGSR 2 / TEMPORAL**)

```rust
/// ONLY spatial upscalers that ship in Nightjar 7.0.
/// `sgsr2` / temporal upscaling is permanently retired and MUST NEVER be added here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecommendedUpscaler {
    None,
    Sgsr1,
    Fsr1,
    /// macOS Apple Silicon / Metal native spatial scaler only.
    MetalFx,
}
```

| Client Platform & Stream Conditions | Recommended Upscaler | Explicit Blocker Behavior (Loud, No Silent Fallback) |
| --- | --- | --- |
| `hdr == true` OR `ten_bit_sdr == true` | `None` | States clearly: *"Client upscaling is unavailable for HDR / 10-bit streams; lowering resolution uses display scaling instead."* (Matches `UpscalingPolicy.Reason.HDR` / `TEN_BIT` and `metalfx::blocker`!) |
| `pyrowave == true` | `None` | States clearly: *"Client upscaling is disabled for PyroWave streams."* (`UpscalingPolicy.Reason.PYROWAVE`) |
| `output_w <= target_w \|\| output_h <= target_h` | `None` | No upscale needed (`UpscalingPolicy.Reason.NO_UPSCALE`) |
| **macOS** (`target_os = "macos"`) with `metalfx_supported && metalfx_blocker.is_none()` | **`MetalFx`** | Best quality/watt on Metal + VideoToolbox; if `metalfx_blocker` is `Some(...)` (e.g., Frame Warp active or SDL GPU renderer), falls explicitly to **`Fsr1`** (desktop plugged in) or **`Sgsr1`** (battery) and names why |
| **Android** (phone, handheld, TV) with `gles_or_vulkan_upscale_supported` | **`Sgsr1`** (default; **`Fsr1`** offered as secondary crisp choice on high-headroom non-thermal-throttled tablets/TVs) | Single-pass spatial shader keeps mobile GPU cool (`UpscalingPolicy.Mode.SGSR1` / `FSR1`); **never** references `SGSR2` |
| **Windows / Linux desktop** (GPU shaders supported) | **`Fsr1`** on discrete GPU plugged in; **`Sgsr1`** on integrated GPU / handheld / battery | Sharp 2-pass EASU+RCAS (`Upscaling::Fsr1`) when GPU headroom exists, lightweight 1-pass (`Upscaling::Sgsr1`) when constrained |

## 3. Architecture & <=800-LOC Module Plan

### Size-Ratchet Guardrail (`tools/size_ratchet.json`)

| Grandfathered / Near-Ceiling File | Ratchet / Current LOC | Strategy to Avoid Growth |
| --- | --- | --- |
| `client/protocol/policy/src/health.rs` | 832 LOC (ratchet) | **Zero additions to `health.rs`!** Build Stream Doctor 2.0 fusion & upscaler advisor on top of `health::Report` in new `client/protocol/policy/src/doctor_v2.rs` (<=520 LOC). |
| `client/protocol/desktop-media/src/stream/doctor.rs` | 578 LOC (<800 ceiling) | Keep v1 collector intact; put v2 host-snapshot polling, remedy execution & undo manager in new `client/protocol/desktop-media/src/stream/doctor_v2.rs` (<=380 LOC). |
| `rust/host/web/src/native_api.rs` | 853 LOC (ratchet) | Add `mod doctor;` + 2 route lines (`/stream/doctor`, `/stream/doctor/apply`) + `feature::STREAM_DOCTOR_V2`. |
| `rust/host/stream/src/stream/video/run.rs` | 1956 LOC (ratchet) | Expose read-only `stutter_report::Report` snapshot via existing `Session` telemetry handle (<=5 lines); no diagnosis logic on the video loop! |
| `client/app/src/main/java/com/limelight/ui/StreamMenu.java` | 1321 LOC (ratchet) | Route Stream Doctor row to new `client/app/src/main/java/com/limelight/ui/StreamDoctorSheet.java` (<=440 LOC). |
| `client/app/src/main/java/com/limelight/GameMenuActions.java` | 1075 LOC (ratchet) | Delegate remedy apply/undo to new `client/app/src/main/java/com/limelight/binding/video/StreamDoctorController.java` (<=380 LOC). |
| `client/app/src/main/java/com/limelight/binding/video/UpscalingPolicy.java` | 256 LOC (<800) | Unchanged (retains legacy `fromPreference("sgsr2") -> SGSR1` migration only); Doctor 2.0 never emits `SGSR2`. |
| `client/protocol/desktop-config/src/strings/*.rs` | 1750 LOC (ratchet) | **Zero growth!** Route all new 7.0.0 Stream Doctor Desktop translations to `client/protocol/desktop-config/src/strings_70/<code>.rs` (14 non-English locale tables; English literals live at call sites). |

### New & Touched Module Breakdown

| Path | Layer | Est. LOC | Responsibility |
| --- | --- | --- | --- |
| `client/protocol/native-api/src/doctor.rs` (new) | `rubylight-native-api` | ~220 | `DoctorSnapshot`, `HostStageBreakdown`, `EncoderFacts`, `EligibleRemedy`, `ActiveTrial`, `DoctorApplyRequest`, `RemedyId` |
| `rust/core/media-policy/src/doctor_v2.rs` (new) | `butterpollo-core` | ~340 | Pure projection from `stutter_report::Report` + `pipeline_rates` + `fec_report` + `link_rate` into `DoctorSnapshot`, remedy eligibility rules, trial undo validation (`TrialRefused::Replaced` semantics) |
| `rust/host/web/src/native_api/doctor.rs` (new) | `butterpollo` host | ~360 | `GET /stream/doctor` & `POST /stream/doctor/apply` (reuses live runtime bitrate, live FEC auto switch, and session-scoped state without touching saved `config.rs`) |
| `client/protocol/policy/src/doctor_v2.rs` (new) | `nightjar-policy` | ~520 | `DoctorInput`, `DiagnosisCard`, `RootCause`, `RemedyPlan`, `RecommendedUpscaler` (`None | Sgsr1 | Fsr1 | MetalFx` — compile-time exclusion of `sgsr2`), `diagnose()`, `recommend_upscaler()` |
| `client/protocol/client/src/http/doctor.rs` (new) | `nightjar-core` | ~180 | `HostClient::stream_doctor()`, `apply_stream_doctor(req)` |
| `client/protocol/desktop-media/src/stream/doctor_v2.rs` (new) | `nightjar-desktop` | ~380 | Polls `GET /stream/doctor` only while Stream Doctor panel is open (1 Hz — zero background traffic when closed!), executes client+host composite remedies (e.g., `0x5532` resize + `Upscaling::Sgsr1`/`Fsr1`/`MetalFx`), manages Undo stack |
| `client/protocol/desktop-ui/src/ui/doctor_card.rs` (new) | `nightjar-desktop` | ~320 | Clean single-diagnosis card, one-tap action buttons, Undo bar, collapsible *Technical details* drawer |
| `client/protocol/desktop-config/src/strings_70/<code>.rs` (14 locale files) | `nightjar-desktop` | ~45/locale | 7.0.0 Desktop translations for Stream Doctor 2.0 diagnosis cards, Undo notices, and SGSR 1 / FSR 1 / MetalFX-only labels (zero SGSR 2) across all 14 non-English locales, keeping grandfathered `strings/*.rs` <= 1,750 LOC |
| `client/protocol/android/src/doctor_v2.rs` (new) | `nightjar-android` | ~220 | JNI bridge exposing `nightjar_policy::doctor_v2::diagnose` + `nightjar_core::http::doctor` to Java so Android and Desktop produce byte-identical verdicts |
| `client/app/src/main/java/com/limelight/binding/video/StreamDoctorController.java` (new) | `nightjar-android` | ~380 | Runs composite remedies (host apply + `0x5532` resize + `UpscalingPolicy.Mode.SGSR1`/`FSR1` switch), tracks session-only Undo |
| `client/app/src/main/java/com/limelight/ui/StreamDoctorSheet.java` (new) | `nightjar-android` | ~420 | Plain-English diagnosis card, single-tap Apply/Undo, collapsed technical metrics |

### Vendor, OS & Backend Preservation

- **AMD AMF & NVIDIA NVENC:** `EncoderFacts.backend` reads the existing `Encoder` enum (`amf` / `nvenc` / `pyrowave`) purely from host telemetry — zero new vendor SDK calls. Any NVENC-specific wording (e.g., NVENC bitrate/resolution headroom) is verified against existing `rust/windows/graphics/src/nvenc.rs` constants and never claimed as run on NVIDIA hardware.
- **macOS vs Linux vs Windows desktop builds:** Linux `sysctl net.core.rmem_max` copy hint is gated strictly with `#[cfg(target_os = "linux")]` (never `cfg(unix)`!); MetalFX guidance is gated strictly with `#[cfg(target_os = "macos")]` (`tools/arch_check.py` `unannotated-cfg-unix` clean).
- **Zero stream-path overhead when closed:** Host `stutter_report.rs` ALREADY runs on the existing 5 s cadence; `GET /stream/doctor` is polled ONLY while the user opens the Stream Doctor card. Default streaming path adds **0.00 ms** latency.

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### Reversible Remedy Trial State Machine

```
[Idle / Observing]
        │  (user opens Stream Doctor → fetch GET /stream/doctor if Feature::StreamDoctorV2)
        ▼
[Diagnosed(Card)] ──(user taps 1-tap Remedy)──► [ApplyingRemedy]
                                                       │
                       ┌────────(success)──────────────┴────────(refused/network err)────┐
                       ▼                                                                 ▼
                [TrialActive]                                                    [ApplyFailed]
                (stores before_state +                                     (shows plain-English error;
                 applied_state + trial_id;                                  nothing partially applied!)
                 evaluates next 30 s window)
                       │
         ┌─────────────┼───────────────────────────────┐
         ▼             ▼                               ▼
  (user taps Undo)  (manual edit / auto-bitrate     (stream ends or
                     moves same knob externally)     reconnects fresh)
         │             │                               │
         ▼             ▼                               ▼
  [UndoingTrial]    [TrialReplaced]                 [Cleared]
         │          (Undo disabled + plain notice    (host discards all
         ▼           preserving player's manual       session-only trials
  [Diagnosed]        change; never clobbers!)         automatically!)
```

### Composite Remedy Atomicity (Resolution + Spatial Upscaler)

When applying `render_scale_stepdown` + client upscaling:
1. Client validates local upscaler eligibility FIRST (`UpscalingPolicy.unavailableReason` / `metalfx::blocker`).
2. Client sends `0x5532` reconfigure (`target_width`, `target_height`) and waits up to 2 s for decoder format confirmation.
3. On format confirmation, client activates the recommended spatial upscaler (`SGSR 1`, `FSR 1`, or `MetalFX` on macOS) for this session only (does NOT overwrite saved `SharedPreferences` / `config.toml`).
4. If step 2 fails or times out, step 3 does NOT run, the error is surfaced loudly, and Undo is not left half-applied.

### Reconnect & Edge-Case Matrix

| Scenario | Expected Behavior |
| --- | --- |
| **Stream warm-up (< 2 s or < 3 samples)** | Shows calm *"Gathering a few seconds of video..."* card; Apply buttons disabled so startup IDR spikes never trigger false remedies (preserves `nightjar#80` invariant). |
| **Manual bitrate/upscaler change during active trial** | Host compares `live_kbps` against `trial.applied_value`; mismatch returns `422 not_writable` (`trial_replaced`) and client cancels Undo rather than clobbering the user's manual setting. |
| **Client disconnects & reconnects** | Host session-scoped trials (`bitrate`, `fec_auto`) clear cleanly on session end; client clears local `TrialActive` state so stale Undo buttons never appear on a new stream. |
| **Non-owner guest opens Stream Doctor** | Reads diagnostics cleanly (`caller_is_owner = false`); host-mutating remedies (`bitrate_stepdown`, `fec_auto`) are hidden/marked *"Host device only"*, while client-local remedies (frame pacing, local decode guidance) remain available. |
| **HDR / 10-bit SDR or PyroWave active** | Upscaler recommendation strictly returns `RecommendedUpscaler::None` with explicit reason; never attempts unsupported GLES/MetalFX upscaling. |
| **Host lacks `stream.doctor_v2` (older Rubylight / Sunshine / Apollo)** | Client skips `/stream/doctor`, builds card purely from `StreamHealth` + `ReceiveBuffer`, and uses classic `RuntimeBitrate` trial when `Feature::RuntimeBitrate` is supported. |

### Crash-Restore Journal

- **Zero persistent host system mutations:** All Stream Doctor 2.0 remedies (`bitrate_stepdown`, `fec_auto`, `render_scale_stepdown`, client upscaler, client frame pacing) are **strictly session-scoped in memory**. They never modify `config.rs`, Windows registry, display drivers, or client saved preferences unless the user explicitly clicks a separate *"Save as default for this PC"* confirmation after a successful trial. Consequently, a host or client crash automatically restores the user's pre-trial configuration on the very next launch without requiring a disk recovery journal.

## 5. Settings & 10/10 Craft Plan

- **Fewer-settings rule:** Zero new preference keys on Android (`SettingsKeysTest` untouched), Desktop (`config.rs` untouched), or Host (`config.rs` untouched). Stream Doctor 2.0 replaces the verbose wall-of-text view inside the existing **Stream Doctor** Stream Menu entry.
- **Plain-English copy (no jargon dumps):**
  - Wi-Fi jitter: *"Your Wi-Fi is delivering frames in uneven bursts, causing brief stutters. Lowering bitrate slightly or turning on automatic error recovery usually smooths this out."*
  - Host overload: *"Your PC's GPU or encoder fell behind the game's frame rate. Rendering the stream at 1080p with SGSR 1 sharpening takes load off your PC while keeping the picture crisp."*
  - Client decode: *"This device's video decoder took longer than one frame to finish decoding. Dropping the stream resolution and using spatial upscaling keeps motion smooth."*
  - Receive buffer: *"Your system capped the network receive buffer below what high-bitrate video needs, dropping bursts of packets."*
  - Technical details toggle: *"Show technical measurements"* (collapsed by default; reveals p95 ms, frame loss %, host stage breakdown, and encoder backend).
- **Upscaler labels (strictly no `SGSR 2` / no `"Experimental"`):**
  - `"SGSR 1"` (mobile/handheld spatial sharpening)
  - `"FSR 1"` (desktop spatial sharpening)
  - `"MetalFX"` (macOS native spatial scaling — shown ONLY on macOS)
  - Never mention `"SGSR 2"`, `"Temporal"`, `"Experimental"`, or any third-party mod name anywhere in Stream Doctor strings!
- **Dynamic input glyphs & zero stream-time blur:**
  - In-stream card renders over an opaque/semi-opaque dark scrim without blur passes (zero compositor cost while diagnosing stutter!).
  - Primary remedy button and Undo button display active controller prompts (A/Cross to Apply, Y/Triangle for Technical details, B/Circle to Close).
- **15-locale coverage:** All diagnosis titles, two-sentence explanations, remedy button labels, and Undo notices localized across all 15 locales — Desktop routes all 14 translated tables to `client/protocol/desktop-config/src/strings_70/<code>.rs` (English literals at call sites; never growing grandfathered `strings/*.rs` past 1,750 LOC) and Android updates all 15 `values*/strings.xml` files.

## 6. Disjoint Implementer Slices

| Slice | Owner Scope | Exclusive File List |
| --- | --- | --- |
| **Slice 11-A: Native API Types, Lockstep Capabilities, Host Core & Policy Classifier** *(Stage 1)* | Wire types, host `doctor_v2` projection, shared `nightjar-policy::doctor_v2` fusion + spatial-only upscaler recommender + trace tests + all 4 lockstep `HostCapabilities` contract files | `client/protocol/native-api/src/doctor.rs` (new), `client/protocol/native-api/src/lib.rs`, `client/protocol/native-api/tests/fixtures/stream_doctor.json` (new), `client/protocol/native-api/tests/fixtures.rs`, `rust/core/media-policy/src/doctor_v2.rs` (new), `rust/core/src/lib.rs`, `client/protocol/policy/src/doctor_v2.rs` (new), `client/protocol/policy/src/lib.rs`, `client/protocol/policy/tests/traces/doctor_v2.json` (new), `client/protocol/policy/src/http/capabilities.rs`, `client/protocol/client/tests/capabilities_contract.rs`, `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java`, `client/app/src/test/resources/protocol/capabilities/*` |
| **Slice 11-B: Host Native API Routes & Telemetry Hook** *(Stage 2, parallel with 11-C/11-D)* | `GET /stream/doctor`, `POST /stream/doctor/apply`, live trial undo state, route tests, docs | `rust/host/web/src/native_api/doctor.rs` (new), `rust/host/web/src/native_api.rs`, `rust/host/stream/src/stream/video/run.rs`, `tests/host/tests/nvhttp_parity/native_tests/doctor.rs` (new), `tests/host/tests/nvhttp_parity/native_tests.rs`, `docs/native-api.md` |
| **Slice 11-C: Nightjar Core & Desktop Doctor 2.0** *(Stage 2, parallel with 11-B/11-D)* | HTTP client, desktop `doctor_v2.rs`, UI card, MetalFX/FSR1/SGSR1 execution, 15 locales | `client/protocol/client/src/http/doctor.rs` (new), `client/protocol/client/src/http/native.rs`, `client/protocol/desktop-media/src/stream/doctor_v2.rs` (new), `client/protocol/desktop-ui/src/ui/doctor_card.rs` (new), `client/protocol/desktop-config/src/strings_70/<code>.rs` (14 non-English locale tables; never growing grandfathered `strings/*.rs` past 1,750 LOC) |
| **Slice 11-D: Nightjar Android Doctor 2.0 & JNI** *(Stage 2, parallel with 11-B/11-C)* | JNI bridge, `StreamDoctorController.java`, `StreamDoctorSheet.java`, 15 `strings.xml` | `client/protocol/android/src/doctor_v2.rs` (new), `client/protocol/android/src/lib.rs`, `client/app/src/main/java/com/limelight/binding/video/StreamDoctorController.java` (new), `client/app/src/main/java/com/limelight/ui/StreamDoctorSheet.java` (new), `client/app/src/main/java/com/limelight/ui/StreamMenu.java`, `client/app/src/main/java/com/limelight/GameMenuActions.java`, `client/app/src/main/res/values*/strings.xml`, `client/app/src/test/java/com/limelight/binding/video/StreamDoctorControllerTest.java` (new) |

## 7. Verification & Measurement Plan

### Narrowest Unit, Trace & Interop Tests

1. **Spatial-Only Upscaler Hard-Rule Tests (no GPU):**
   - `cargo test -p nightjar-policy --locked doctor_v2`:
     - Exhaustive matrix across `(platform ∈ {Android, Windows, Linux, macOS}) × (hdr, ten_bit, pyrowave, metalfx_ok, thermal, output_vs_input)` asserting:
       - Returned `RecommendedUpscaler` is strictly within `{None, Sgsr1, Fsr1, MetalFx}`.
       - `MetalFx` is ONLY ever returned on `Platform::MacOs` when `metalfx_supported && metalfx_blocker.is_none()`.
       - String serialization of every possible `DiagnosisCard` and `RemedyPlan` NEVER contains `"sgsr2"`, `"SGSR 2"`, `"temporal"`, or `"Experimental"`!
     - Distinguishes all 5 root causes cleanly on synthetic traces (Wi-Fi jitter vs. host encode stall vs. client decode pressure vs. short receive buffer vs. presentation wait).
     - Startup 2 s spike produces `Healthy` / no false remedies (`nightjar#80` regression guard).
2. **Host API & Trial Undo Tests (GPU-free loopback):**
   - `cargo test -p butterpollo --locked nvhttp::native_tests::doctor`:
     - Owner applies `bitrate_stepdown` & `fec_auto`, then undoes both cleanly.
     - External manual bitrate change causes `undo` to return `422 not_writable` (`trial_replaced`) without overwriting the manual rate.
     - Guest receives `409 not_session_owner` on `POST /stream/doctor/apply` while `GET /stream/doctor` succeeds.
     - PyroWave stream marks `fec_auto` as `configurable: false` with explicit reason and refuses `POST`.
3. **Android & Desktop Contract Tests:**
   - `cargo test -p nightjar-core --locked capabilities_contract` + `cargo test -p nightjar-desktop --locked doctor`.
   - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.binding.video.StreamDoctorControllerTest --tests com.limelight.preferences.SettingsKeysTest`.
   - `python tools/arch_check.py` (verifies `health.rs <= 832` unchanged, grandfathered `client/protocol/desktop-config/src/strings/*.rs` stay <= 1,750 LOC, zero `Family::Rubylight`, zero unannotated `cfg(unix)`).

### Visual Screenshot Checks

- **Desktop:** `client/protocol/desktop-ui/src/ui/tests.rs` + `shot.rs` rasterizes all 5 diagnosis cards (collapsed and expanded *Technical details*), active Undo banner, and macOS MetalFX vs. Windows FSR 1 / SGSR 1 copy in English and German.
- **Android:** Robolectric/emulator screenshots of `StreamDoctorSheet` on phone landscape, portrait, and Android TV verifying single-screen legibility without scrolling for the primary remedy button and zero blur layers.

### Serialized Idle-Gated Hardware Criteria

- Pre-check: `(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` shows `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0`, user idle.
- Isolated test host (`butterpollo.exe --config-dir C:\tests\doctor --port 48123 --bind 127.0.0.1`) with synthetic fault injection (`BUTTERPOLLO_TEST_SEND_JITTER=1`, `BUTTERPOLLO_TEST_SEND_LOSS=5`):
  - Verify `GET /stream/doctor` classifies network jitter (NOT host encode overload) while host stage p95 stays clean, and one-tap `fec_auto` + `bitrate_stepdown` recovers unrecoverable FEC blocks to 0 within 10 s, then one-tap Undo restores original bitrate/FEC.
  - Verify 0.00 ms encode/convert p99 regression when Stream Doctor panel is closed (ABBA 90 s `e2e.py` run).
- **Exit code 3 abort rule:** If `e2e.py` or `check.ps1` exits with code `3` (AMD encoder hang), stop immediately and investigate — never retry.

---

## 8. Post-Discovery & 6.3.0 Rebase Addendum — `#142` Transport, Multi-GPU LUIDs, OEM 60 Hz & Aspect-Safe Stepdown

- **Fuse 6.3.0 `#142` Transport Telemetry into `DoctorSnapshot` (`API_VERSION = 1` additive `#[serde(default)]`):** Add a compact `transport` sub-object populated from live host telemetry (`pacing_reason`, `pacing_bps`, `link_ceiling_kbps`, `ceiling_source`, `enobufs_waits`, `gso_fallbacks`, `send_losses`, `queue_p95_ms`, `congestion_state`, `link_rate_state`, `traverse_active`) and split coarse `network_loss` into four precision rules: `host_socket_pressure` (`WSAENOBUFS`/GSO fallback -> host NIC/offload, not Wi-Fi), `wireless_burst_clamp`, `wifi_queue_jitter`, and `link_bandwidth_ceiling`.
- **Multi-GPU & Android 60 Hz Diagnostics:** Surface Item `28` active `capture_luid`, `vdisplay_render_luid`, `encoder_luid`, and `cross_adapter_copy` state in `DoctorSnapshot`, and wire Item `29`'s `RefreshSwitchDiagnosis` (`UNEXPLAINED` / `USER_LIMIT`) + `ArrHalfRateCheck` directly into a `DisplayPacingMismatch` card with plain-English OEM 60 Hz unlock steps (OriginOS / Samsung GOS / MIUI / Android 15 Game Default Frame Rate).
- **Aspect-Safe `render_scale_stepdown` (`#137`):** Replace hardcoded `1920×1080` stepdown target with `02`'s shared `nightjar_policy::resolution_scale` helper so 16:10, 3:2, 20:9, and Retina displays step down cleanly along their native aspect ratio without letterboxing.

## 9. As built (2026-11-26)

The build narrows this design as follows; each is deliberate and reversible later as an additive change.

- **Host projection** lives in `rust/core/session/src/native/doctor.rs` (the session crate owns the Native API projections), not in media-policy. `host_stages` is the console's stutter report over the last 30 s, derived from the published 5-second reports (kept by age, at most 32), so no per-picture work is added.
- **One host remedy, `bitrate_stepdown`.** It keeps the launch's FEC share inside the bitrate and caps at the codec's runtime limit and `max_bitrate`, so its undo restores the encoder exactly; an undo the limit now holds lower is refused (`capped`) and kept. `fec_auto` is not built: the FEC split is computed at stream start and needs its own design and a hardware run. The `revision` field was dropped; remedy errors use the existing error codes.
- **Resolution step-down** is client-side over the existing in-place mode change (0x5532), offered only where the stream already supports it, with an undo for this connection. On desktop it changes the stream's in-memory settings like the Stream size editor, so after an auto-reconnect the smaller size stays while its Undo is gone (the size editor remains the way back).
- **Frame-rate step-down and frame-pacing switch are advice, not one-tap remedies.** `LowerFrameRate` and `FramePacing` name the setting to change for the next stream. A one-tap frame-rate step through the same 0x5532 path is a follow-up.
- **The upscaler is advice for the next stream**: no renderer switches mid-stream, and no saved setting changes.
- **Not built from §8:** the `transport` sub-object, multi-GPU LUIDs and Android 60 Hz diagnostics, and controller glyph prompts (§5). Desktop treats only gamescope as constrained (no iGPU or battery detection yet), so integrated-GPU laptops get the FSR 1 recommendation.
