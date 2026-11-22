# Item 29: High-Refresh Transport Pacing, Android MediaCodec/ARR Pacing & WebRTC Transport Resilience (`#144`, `#7`, `#119`, `#116`) (Wave 1)

Status: engineering design (Phase 1). Written against `origin/main` `995f2540c8` (`6.3.0`, including `#142` pacing/ENOBUFS/GSO commits), [`00-protocol-reservations.md`](00-protocol-reservations.md), and open issues **Rubylight `#144`**, **Nightjar `#7`**, **Nightjar `#119`**, and **Nightjar `#116`**.

## 1. Summary & User Experience

Item `21` overhauls desktop client decode/presentation and host NVENC; Item `29` closes the symmetric **Host UDP/WebRTC Transport & Android MediaCodec/SurfaceFlinger Pacing** gap behind the highest-signal community smoothness reports (Shield TV Pro 240/480/1000 Hz stutters vs Vibepollo `#144`, Shield reconnect decoder debt `#7`, Pixel/phone 120 Hz judder `#119`, Vivo OriginOS / Android 15 60 Hz caps `#116`, and browser `/play` & `/guest` congestion storms) — with **zero wire-protocol changes** and **fewer settings** (retiring default-off `experimentalChipsetTuning`).

### 1.1 Three Tightly Scoped Performance & Smoothness Pillars

1. **Host High-Refresh Burst Pacing & 6.3.0 `#142` Transport Overhaul (`rust/core/src/network_pacing.rs`, `congestion.rs`, `adaptive_pacing.rs`):**
   - **Frame-Period-Aware Burst Shaping & GCC Grouping:** Scales `network_pacing::batch_payload_limit` from a fixed 1 ms wire burst down to `min(1 ms, frame_period / 4)` with tighter GSO chunk caps on wireless/unknown-wireless paths, and scales `congestion.rs` `BURST_US` from fixed `5_000 µs` to `min(5_000, frame_period_us / 2)` so 240/480/1000 Hz streams (`#144`) never collapse 2–5 frames into a single inter-arrival group!
   - **Soft-Start Burst Guard for Unknown-Route Clients:** Under `PacketPacing::Auto`, when `clientWireless` is absent (stock Moonlight, Shield TV, Steam Deck), applies a brief 1–2 s conservative start-burst ceiling (`~4×` stream bitrate) that opens smoothly on clean FEC/RTT arrival instead of blasting 800 Mbps GSO start bursts and waiting for keyframe loss to latch — adding **zero latency** to healthy wired LAN streams once settled.
   - **Delay-Based Congestion Control (`0x5547`) A/B Graduation Gate:** Keeps `congestion_control` opt-in until ABBA release measurements under `07-hw-verify.md` prove lower Wi-Fi 6E/7 p99 frame jitter with `<= +0.05 ms` wired LAN delta.
   - **Capture-to-Receive Gap Attribution:** Emits structured host-side vs receive-side irregular-interval counters so `#144` diagnostics cleanly separate host send irregularities from network/Android receive bursts.

2. **Nightjar Android MediaCodec Zero-Debt, Surface Pacing, Android 15/16 ARR & Auto-Quarantined SoC Tuning (`#7`, `#119`, `#116`):**
   - **Shield / Reconnect Zero-Debt & Mismatched-Refresh Cadence (`#7`, `#144`, `#119`):** Flushes/resets MediaCodec input-to-output residency cleanly on reconnect and cadence shifts to eliminate persistent 30–40 ms decoder debt on NVIDIA Shield in `Smoothest` mode; bounds output-queue residency in `Smoothest` and non-integer host/TV refresh ratios (`240/480 Hz -> 60 Hz` TV) inside new `<= 800`-LOC `binding/video/PacingCadenceGuard.java` without growing grandfathered `MediaCodecDecoderRenderer.java` (`6811` LOC ceiling!). Evaluates expanding display-queue watching beyond `omx.nvidia.`/`c2.nvidia.` behind device-validated cadence guards.
   - **Android 15 (API 35) LTPO + Android 16 ARR & OEM 60 Hz Lock Guidance (`#116`, `#119`):** Extends `DisplayFrameRatePolicy.java` to drive high-rate `Surface.setFrameRate` / LTPO cadence on Android 15 (API 35) alongside API 36 ARR (mitigating Android 15's 60 Hz `appCategory="game"` cap), and feeds `RefreshSwitchDiagnosis` (`UNEXPLAINED` / `USER_LIMIT`) + `ArrHalfRateCheck` directly into `11-stream-doctor.md`'s `DisplayPacingMismatch` card with plain-English OEM unlock instructions (OriginOS / Samsung GOS / MIUI / Android 15 Game Default Frame Rate).
   - **Retire `experimentalChipsetTuning` (Fewer Settings!):** Graduates MediaTek, Exynos, Qualcomm declared-only, and Tensor Codec2 low-latency keys onto automatic probing protected by existing `VendorKeyHangs.java` per-`(decoder, mime, versionCode, Build.FINGERPRINT)` auto-quarantine (retiring the default-off `experimentalChipsetTuning` preference key cleanly via `SettingsKeysTest` migration while keeping honest hardware disclosure for SoCs not tested locally!).

3. **WebRTC `/play` & `/guest` Transport Resilience (`rust/host/src/webrtc/`):**
   - **TWCC Bitrate Adaptation (`webrtc/bwe.rs`):** Enables `str0m` `transport-cc` (`set_fb_transport_cc(true)` in `sdp.rs`) and feeds BWE + NACK/loss into an owner-stream adaptive bitrate controller bounded cleanly like `link_rate.rs`, replacing fixed-bitrate 50 ms `MAX_PACED_BACKLOG` drop -> IDR freeze loops with smooth rate adaptation.
   - **ICE Restart, Trickle Candidates & Dual-Stack / STUN Candidates (`webrtc/ice_resilience.rs`):** Raises fatal `DISCONNECT_TIMEOUT` from 3 s to 12 s with fast ICE restart on browser `iceconnectionstatechange` / tab visibility resume, stops discarding client trickle ICE `Candidate` messages (`server.rs:902-906`), and offers paired IPv4 + IPv6 host candidates + optional STUN `srflx` gathering.
   - **Guest IDR Shield:** Replaces fixed 2 s `KEYFRAME_INTERVAL` (`guests.rs:65`) with per-seat exponential backoff + bounded guest send queue so one lagging `/guest` viewer can **never** disrupt the owner's stream with 2 s IDR spikes.

---

## 2. Protocol, Native API & Capability Contract

Per [`00-protocol-reservations.md`](00-protocol-reservations.md) §7, Item 29 requires **ZERO wire-protocol or Native API breaking changes**:
- `PROTOCOL_VERSION` and `rubylight_native_api::API_VERSION = 1` remain untouched.
- All Android pacing, ARR, and `VendorKeyHangs` probing work identically against **every** host (Sunshine, Apollo, Vibepollo, Rubylight, GameStream) — zero `Family::Rubylight` gating.
- WebRTC signaling adds strictly additive optional `ice_restart` / trickle `candidate` handling preserving backwards compatibility with existing `/play` and `/guest` bundles.

---

## 3. Architecture & <=800-LOC Module Plan

All repo paths carry `[verify post-sprint code-map.md]`. Every new module stays `<= 800` LOC with `//!` / `/**` headers; grandfathered files (`congestion.rs` 1499, `MediaCodecDecoderRenderer.java` 6811, `webrtc/media.rs` 1964, `webrtc/session.rs` 1542, `webrtc/guests.rs` 1201, `webrtc/server.rs` 1322) MUST NOT grow!

| Path | Status | Est. LOC | Responsibility |
| --- | --- | ---: | --- |
| `rust/core/src/pacing_high_hz.rs` | New | ~310 | `//!` Frame-period-aware `batch_payload_limit`, scaled `BURST_US`, soft-start unknown-route burst envelope, and irregular-interval gap telemetry + unit tests. |
| `rust/core/src/network_pacing.rs`, `congestion.rs`, `adaptive_pacing.rs` | Modify | `<= ceiling` | Delegate high-Hz scaling & soft-start guard to `pacing_high_hz.rs` (zero net growth to `congestion.rs`). |
| `rust/host/src/webrtc/bwe.rs` | New | ~320 | `//!` `str0m` TWCC BWE adaptive bitrate controller + slow-guest exponential IDR backoff shield + unit tests. |
| `rust/host/src/webrtc/ice_resilience.rs` | New | ~290 | `//!` 12 s disconnect grace + ICE restart state machine, client trickle ICE intake, and dual-stack IPv4+IPv6 / STUN `srflx` candidate collector. |
| `rust/host/src/webrtc/{sdp,media,session,guests}.rs` | Modify | `<= ceiling` | Thin hooks into `bwe.rs` and `ice_resilience.rs` (net LOC `<= ceiling`). |
| `client/app/src/main/java/com/limelight/binding/video/PacingCadenceGuard.java` | New | ~340 | `/**` Reconnect zero-debt flush coordinator, `Smoothest`/mismatched-refresh residency bound (`#7`, `#144`, `#119`), and device-validated display-queue guard. |
| `client/app/src/main/java/com/limelight/binding/video/Android15ArrCompat.java` | New | ~240 | `/**` API 35 `Surface.setFrameRate` / LTPO fallback alongside API 36 ARR + `RefreshSwitchDiagnosis` / `ArrHalfRateCheck` structured signal export for Stream Doctor 2.0. |
| `client/app/src/main/java/com/limelight/binding/video/{DecoderQueuePolicy,RefreshSlots,DisplayFrameRatePolicy,VendorLowLatencyKeys,MediaCodecDecoderRenderer}.java` | Modify | `<= ceiling` | Thin delegation to `PacingCadenceGuard` / `Android15ArrCompat` + auto-quarantined `VendorKeyHangs` SoC key activation (`MediaCodecDecoderRenderer.java` net `<= 6811`). |
| `client/app/src/main/java/com/limelight/preferences/{PreferenceConfiguration,StreamSettings}.java` & `SettingsKeysTest.java` | Modify | `<= ceiling` | Cleanly retire `experimentalChipsetTuning` (keep key in retired-keys list in `SettingsKeysTest`, zero reused keys). |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

- **Unknown-Route Soft-Start Pacing:** `Start(t=0..1.5s, cap=4x_bitrate)` -> `(0 FEC loss & stable RTT)` -> `UnthrottledLan` (0 µs added latency) OR `(loss/queue spike)` -> `AdaptivePacingLatched`.
- **Android Reconnect Zero-Debt (`#7`):** `StreamReconnect` -> `MediaCodec.flush()` + reset `PacingCadenceGuard` residency counter + discard pre-reconnect pending presentation timestamps -> clean first IDR latch with zero residual 30–40 ms buffer debt.
- **SoC Auto-Quarantine:** Unknown/new `(decoder, mime, versionCode, Build.FINGERPRINT)` probes low-latency vendor keys under `VendorKeyHangs` sentinel; any stall/hang latches safe standard keys permanently for that fingerprint and logs visibly.
- **Crash-Restore:** Zero host OS state mutated.

---

## 5. Settings & 10/10 Craft Plan

- **Net -1 setting!** Retires `experimentalChipsetTuning` automatically behind `VendorKeyHangs` crash/hang quarantine (`SettingsKeysTest` verified). Adds **zero** new settings on host or client.
- **Plain-English diagnostics:** Android OEM 60 Hz locks (OriginOS, Samsung GOS, MIUI, Android 15 Game Default Frame Rate) surface actionable plain-English steps inside Stream Doctor 2.0 (`11-stream-doctor.md`).
- **Zero `"Experimental"` & zero mod names** anywhere in UI strings across all 15 Nightjar and 22 host locales.

---

## 6. Disjoint Implementer Slices (Phase 3 `parallel()`)

Wave 1 scheduling: depends on `04`, `06`, and `27` (zero conflict with Wave 1 video/encode or `webrtc/server.rs` security hardening); `07-hw-verify` depends on `["03", "04", "05", "06", "21", "22", "27", "28", "29"]`.

| Slice | Stage | Exclusive Owned Files (Zero Overlap) |
| --- | --- | --- |
| **29-A: Host High-Hz Burst Pacing & Soft-Start Envelope** | **Stage 1** (parallel with 29-B, 29-C) | `rust/core/src/pacing_high_hz.rs` (new), `rust/core/src/lib.rs`, `rust/core/src/network_pacing.rs`, `rust/core/src/congestion.rs`, `rust/core/src/adaptive_pacing.rs` `[verify post-sprint code-map.md]` |
| **29-B: Host WebRTC TWCC BWE, ICE Restart & Guest IDR Shield** | **Stage 1** (parallel with 29-A, 29-C) | `rust/host/src/webrtc/bwe.rs` (new), `rust/host/src/webrtc/ice_resilience.rs` (new), `rust/host/src/webrtc/sdp.rs`, `rust/host/src/webrtc/media.rs`, `rust/host/src/webrtc/session.rs`, `rust/host/src/webrtc/guests.rs`, `rust/host/src/webrtc/server.rs` *(conflict-free: Item `29` depends on `27`)* `[verify post-sprint code-map.md]` |
| **29-C: Nightjar Android MediaCodec Zero-Debt, ARR & SoC Auto-Quarantine** | **Stage 1** (parallel with 29-A, 29-B) | `client/app/src/main/java/com/limelight/binding/video/PacingCadenceGuard.java` (new), `client/app/src/main/java/com/limelight/binding/video/Android15ArrCompat.java` (new), `client/app/src/main/java/com/limelight/binding/video/{DecoderQueuePolicy,RefreshSlots,DisplayFrameRatePolicy,VendorLowLatencyKeys,MediaCodecDecoderRenderer}.java`, `client/app/src/main/java/com/limelight/preferences/{PreferenceConfiguration,StreamSettings}.java`, `client/app/src/test/java/com/limelight/binding/video/PacingCadenceGuardTest.java` (new), `client/app/src/test/java/com/limelight/preferences/SettingsKeysTest.java` `[verify post-sprint code-map.md]` |

---

## 7. Verification & Measurement Plan

1. **Deterministic Unit & Simulation Tests (No GPU Required):**
   - `cargo test -p butterpollo-core --locked pacing_high_hz network_pacing congestion adaptive_pacing` — verifies 240/480/1000 Hz `batch_payload_limit` & `BURST_US` non-aliasing, soft-start open/latch transitions, and zero overhead after wired LAN settle.
   - `cargo test -p butterpollo --locked webrtc::bwe webrtc::ice_resilience` — verifies TWCC rate stepdown under simulated packet loss without 50 ms backlog IDR storms, ICE restart within 12 s grace window, and guest exponential IDR backoff shielding owner stream.
   - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.binding.video.PacingCadenceGuardTest --tests com.limelight.binding.video.DecoderQueuePolicyTest --tests com.limelight.preferences.SettingsKeysTest` — verifies Shield `Smoothest` reconnect flush, non-integer cadence residency cap, API 35/36 ARR fallback, `VendorKeyHangs` auto-quarantine, and clean `experimentalChipsetTuning` retirement.
2. **Real Hardware Verification (Executed Under `07-hw-verify`, Serialized Idle-Gated):**
   - **Host + Android Devices (Retroid Pocket 6, Galaxy Z Fold7, TCL C6K TV, and Shield TV Pro if attached):** 90 s ABBA release-build measurements comparing baseline vs Item `29` across 60/120/240 Hz streams (`present_to_sent p99 <= +0.05 ms` on wired LAN, reduced irregular receive intervals & `>25 ms` gaps, zero reconnect decoder latency debt).
   - **Hardware Honesty:** Untested third-party Android SoCs (e.g., MediaTek/Exynos/Tensor models not physically tested locally) and NVIDIA NVENC host paths are documented honestly as unit-tested/auto-quarantined rather than locally benchmarked.
   - **Exit Code 3 Rule:** Any AMD encoder hang (`exit 3`) immediately aborts the batch for investigation — never retry.
