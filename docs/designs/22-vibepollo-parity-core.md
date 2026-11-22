# Item 22: Vibepollo Protocol & Telemetry Parity Core — Launch Warnings, `gpu_encoder_percent` DXGI/PDH Telemetry, Desktop `FrameLimiterStatus` & `psmap` (Wave 1 — PR 1: Tracks 1, 2, 3 & 7a)

Status: engineering design (Phase 1). Written against `origin/main` `995f2540c8` (`6.3.0`), [`00-protocol-reservations.md`](00-protocol-reservations.md), and `docs/nvenc.md`.

## 1. Summary & User Experience

Item 22 delivers **PR 1 (Tracks 1, 2, 3 & 7a)** of Rubylight 7.0 + Nightjar 7.0's Vibepollo protocol/capability parity — improving everyday diagnostics, NVIDIA/Intel/AMD GPU telemetry, Desktop capability parity, and PlayStation controller slot awareness with **zero breaking protocol changes**, **zero default-path streaming latency**, and **zero new user-facing settings bloat**:

1. **Track 1 — Launch Warnings (`<clientWarning>` + `<clientWarningMessage>` on `/launch` & `/resume`):**
   - When a host starts a stream cleanly (`HTTP 200`, `<gamesession>1` or `<resume>1`) after a non-fatal setup fallback (such as falling back from a virtual display disabled by the client, unavailable `rubylight-vdisplay`, or a lowered `rlDisplayScale`), the host emits optional depth-2 `<root>` XML elements `<clientWarning><code></clientWarning>` and `<clientWarningMessage><text></clientWarningMessage>`.
   - **Never** emitted on HTTP error responses, `410`, or control-tile launches; **first warning wins**.
   - Known codes: `virtual_display_disabled_by_client` and `virtual_display_unavailable` (unknown codes display their sanitized `<clientWarningMessage>` text cleanly so future hosts interoperate seamlessly).
   - **Strict sanitizer enforced on BOTH host emit and client parse:**
     - Reject responses `> 16 KiB`; reject DTD declarations (`<!DOCTYPE` / `<!ENTITY`).
     - Ignore `<clientWarningMessage>` if `<clientWarning>` is absent; reject duplicate `<clientWarning>` or `<clientWarningMessage>` tags.
     - Plain text only (no HTML/XML markup).
     - Strip all Unicode `Cc` (Control) and `Cf` (Format) code points **except** ZWJ (`U+200D`) and ZWNJ (`U+200C`).
     - Normalize `\r`, `\t`, `U+2028`, and `U+2029` to spaces/newlines with **at most 3 `\n` line breaks**; trim leading/trailing whitespace; drop messages exceeding **512 UTF-8 bytes**.
   - **Host:** `rust/host/src/nvhttp/launch_steps.rs::launch_response` + `rust/host/src/display_session.rs` fallback & lowered `rlDisplayScale` capture.
   - **Clients:** Shared parser in `client/protocol/client/src/http/launch.rs` (`LaunchReply` + unit tests mirroring `tst_launchwarning.cpp`); Android `NvHTTP.readLaunchResponse` -> `ConnectionContext.launchWarning` (preserving `launchApp`'s `boolean` return signature!) -> post-connect stream notice; Desktop `app/launch.rs` -> `stream/notices.rs`.

2. **Track 2 — `gpu_encoder_percent` + Windows DXGI/PDH Telemetry (Bringing v1 Hardware Telemetry to NVIDIA & Intel GPUs Too!):**
   - Adds `"gpu_encoder_percent"` to `HARDWARE_METRICS` (`rust/core/src/config.rs`, console schema `rust/web/src/lib/schema/basics.ts`, `SettingsSchema.test.mjs`, `rust/host/src/console/settings.rs`, all 22 host locales, `docs/hardware-metrics.md`), `GPU_METRICS` (`client/protocol/client/src/http/hardware_metrics.rs`, `HardwareMetrics.java`), and renders it cleanly in Desktop `overlay.rs` and Android `PerformanceOverlay.java` (**detailed** in Debug mode, **compact** in Statistics mode, **omitted** cleanly when absent).
   - **Windows PDH + DXGI Correlated Collector (`rust/windows/src/telemetry/pdh_dxgi.rs` + `rust/windows/src/telemetry.rs`):**
     - Uses `PdhAddEnglishCounterW` (locale-independent on all Windows languages!) querying `\GPU Engine(*engtype_VideoEncode)\Utilization Percentage`, `\GPU Engine(*engtype_3D)\Utilization Percentage`, and `\GPU Adapter Memory(*)\Dedicated Usage`.
     - **NEVER sums globally across adapters!** Parses each PDH instance's `luid_0xHIGH_0xLOW` token and correlates per adapter via DXGI `IDXGIAdapter1::GetDesc1()`.
     - Attaches per-adapter `gpu_encoder_percent` directly to the matching ADLX AMD `GpuSample` (preserving ADLX's existing AMD metrics).
     - Emits full DXGI+PDH `GpuSample` entries for **non-AMD GPUs (NVIDIA / Intel)** — deterministic LUID-derived `id`, DXGI `Description`, `gpu_usage_percent`, `gpu_encoder_percent`, `gpu_vram_used_mb`, and `gpu_vram_usage_percent` — bringing native v1 hardware telemetry to NVIDIA hosts for the first time (feeding directly into `01-ui-rubylight-studio`, `07-hw-verify`, and `11-stream-doctor`!).
     - Single telemetry worker thread owns `PDH_HQUERY` with deterministic `Drop` -> `PdhCloseQuery`. First sample omits rate readings rather than calling `sleep` (zero blocking on startup!).

3. **Track 3 — Nightjar Desktop `FrameLimiterStatus` Parity:**
   - Strictly gated on `HostCapabilities::has(Feature::FrameLimiterStatus)` (zero `Family::Rubylight` brand checks!), surfacing read-only `FrameLimiterSupported`, `FrameLimiterEnabled`, `VirtualDisplayFrameLimiterEnabled`, and `FrameLimiterFpsLimitMilliHz` inside Desktop `Screen::HostDetails` and/or `stream_menu::Page::More` (plus Wave 0's 7.0 Control Center) with unit tests.

4. **Track 7a — `&psmap=<mask>` Launch Parameter (Deliberately Skipping Harmful Legacy Params):**
   - Both Nightjar Desktop (`client/protocol/client/src/http/launch.rs`) and Android (`NvHTTP.java`) append `&psmap=<mask>` on `/launch` and `/resume`, where `<mask>` is the bitmask of connected PlayStation (`LI_CTYPE_PS`) controller slots masked by `gcmap & 0xFFFF` (zero UI setting!).
   - **Explicitly skipped & documented:** `surroundParams` (breaks 12-channel 7.1.4 negotiation), `clientName` (redundant / leaks device label unnecessarily), and `continuousAudio` (conflicts with Rubylight/Nightjar silence & pacing handling).

---

## 2. Protocol, Native API & Capability Contract

Per [`00-protocol-reservations.md`](00-protocol-reservations.md):
- `PROTOCOL_VERSION` and `rubylight_native_api::API_VERSION = 1` remain unchanged.
- `/launch` & `/resume` XML `<clientWarning>` / `<clientWarningMessage>` are strictly additive depth-2 children of `<root>` on HTTP 200 success only; stock Moonlight clients ignore unknown XML tags safely.
- `&psmap=<mask>` is strictly additive on `/launch` & `/resume` query strings; older hosts ignore unknown query parameters safely.
- `"gpu_encoder_percent"` is strictly additive inside existing `/api/native/v1/hardware-metrics` `GpuSample.metrics` maps (`BTreeMap<String, f64>`).
- Desktop Track 3 gates strictly on `HostCapabilities::Feature::FrameLimiterStatus` (already defined in `capabilities.rs` — never `Family::Rubylight`).

---

## 3. Architecture & <=800-LOC Module Plan

All target modules below are conceptual anchors — **resolve exact post-sprint file paths, split submodules, `tools/size_ratchet.json` ceilings, and `tools/dev.ps1` flags live via `docs/code-map.md` and `client/docs/code-map.md` before assigning implementer slices** (`[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]`). Every new `.rs`/`.java` file stays strictly `<= 800` LOC with `//!` / `/**` headers; grandfathered files MUST NOT grow (Desktop 7.0 translations route strictly into `client/protocol/desktop/src/strings_70/<code>.rs`).

| Path | Status | Est. LOC | Responsibility |
| --- | --- | ---: | --- |
| `rust/core/src/launch_warning.rs` | New | ~220 | `//!` Canonical launch-warning codes & strict sanitizer (Cc/Cf except ZWJ/ZWNJ, `<=3` newlines, `<=512` UTF-8 bytes, plain text) + tests. |
| `rust/host/src/nvhttp/launch_steps.rs`, `rust/host/src/display_session.rs` | Modify | `<= ceiling` | Emit depth-2 `<clientWarning>` + `<clientWarningMessage>` on HTTP 200 `<gamesession>1` / `<resume>1` only (first warning wins). |
| `rust/windows/src/telemetry/pdh_dxgi.rs` | New | ~380 | `//!` `PdhAddEnglishCounterW` + DXGI `IDXGIAdapter1::GetDesc1()` `luid_0xHIGH_0xLOW` per-adapter correlation, non-AMD `GpuSample` synthesis, single-thread `PDH_HQUERY` `Drop` -> `PdhCloseQuery`, no-sleep first sample + parser unit tests. |
| `rust/windows/src/telemetry.rs` | Modify | `<= 720` | Attach `gpu_encoder_percent` to matching ADLX GPUs and append DXGI+PDH samples for non-AMD GPUs. |
| `rust/core/src/config.rs`, `rust/web/src/lib/schema/basics.ts`, `SettingsSchema.test.mjs`, `rust/host/src/console/settings.rs`, `rust/assets/locale/*.json`, `docs/hardware-metrics.md` | Modify | `<= ceiling` | Register `"gpu_encoder_percent"` across `HARDWARE_METRICS`, Console schema, scriptless fallback, 22 locales & docs. |
| `client/protocol/client/src/http/launch_warning.rs` | New | ~240 | `//!` Client strict XML/text sanitizer (16 KiB max, DTD rejection, duplicate tag rejection, orphan message drop, Cc/Cf & `<=3` `\n` & `<=512` B rules) + `tst_launchwarning.cpp`-parity unit tests. |
| `client/protocol/client/src/http/launch.rs`, `client/protocol/client/src/http/hardware_metrics.rs` | Modify | `<= 650` | Parse `LaunchReply` with `warning`, append `&psmap=<mask>` (`LI_CTYPE_PS & gcmap & 0xFFFF`), add `"gpu_encoder_percent"` to `GPU_METRICS`. |
| `client/app/src/main/java/com/limelight/nvstream/http/{LaunchWarningParser.java,NvHTTP.java}`, `ConnectionContext.java`, `HardwareMetrics.java`, `PerformanceOverlay.java`, `Game.java` | New / Modify | ~220 / `<= ceiling` | Java launch-warning sanitizer (`LaunchWarningParser`), `readLaunchResponse` -> `ConnectionContext.launchWarning` (keeping `launchApp` boolean), `&psmap=`, and Debug/Statistics `gpu_encoder_percent` overlay display. |
| `client/protocol/desktop/src/app/launch.rs`, `stream/notices.rs`, `app/overlay.rs`, `ui/host_details_limiter.rs` (new), `ui/control_center_cards.rs`, `strings_70/*.rs` | New / Modify | ~190 / `<= ceiling` | Desktop launch-warning toast, `gpu_encoder_percent` overlay formatting, and `Feature::FrameLimiterStatus` read-only panel + UI tests. |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

- **Launch Warning State:** `LaunchAttempt` -> `(HTTP != 200 || gamesession!=1 && resume!=1)` -> drop warning; `(HTTP 200 && valid sanitized code+msg)` -> latch `first_warning` -> display once post-connect -> clear on stream stop/reconnect so stale warnings never replay.
- **PDH Telemetry Lifecycle:** `WorkerStart` -> open `PDH_HQUERY` + enumerate DXGI LUIDs -> `Sample #1` (collect baseline, omit rate counters — zero `sleep`) -> `Sample #N` (correlate `luid_0xHIGH_0xLOW` per adapter; if PDH fails transiently, degrade gracefully without killing ADLX or blocking stream threads) -> `WorkerDrop` -> `PdhCloseQuery`.
- **Crash-Restore:** Zero host OS state mutated.

---

## 5. Settings & 10/10 Craft Plan

- **Zero new settings bloat:** `"gpu_encoder_percent"` slots cleanly into the existing `hardware_metrics` multi-select list; `psmap`, launch warnings, and `FrameLimiterStatus` readout require zero new settings.
- **Licensing & Provenance (`AGENTS.md`):** Where launch-warning semantics or `tst_launchwarning.cpp` test cases mirror **Vibepollo / Nonary moonlight-qt** (GPL-3.0), preserve file-level credits in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README credits (never in UI text or release notes).
- **Zero `"Experimental"` & zero mod names** across all 22 host and 15 Nightjar locales.

---

## 6. Disjoint Implementer Slices (Phase 3 `parallel()`)

| Slice | Stage | Exclusive Owned Files (Zero Overlap) |
| --- | --- | --- |
| **22-A: Core Warning Sanitizer, Host Launch Warnings & `HARDWARE_METRICS`** | **Stage 1** (parallel with 22-B, 22-C) | `rust/core/src/launch_warning.rs` (new), `rust/core/src/lib.rs`, `rust/core/src/config.rs`, `rust/host/src/nvhttp/launch_steps.rs`, `rust/host/src/display_session.rs`, `rust/host/src/console/settings.rs`, `rust/web/src/lib/schema/basics.ts`, `rust/web/SettingsSchema.test.mjs`, `rust/assets/locale/*.json`, `docs/hardware-metrics.md` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |
| **22-B: Windows DXGI + PDH Per-Adapter Telemetry** | **Stage 1** (parallel with 22-A, 22-C) | `rust/windows/src/telemetry/pdh_dxgi.rs` (new), `rust/windows/src/telemetry.rs` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |
| **22-C: Shared Nightjar Core (`launch_warning`, `psmap`, `GPU_METRICS`)** | **Stage 1** (parallel with 22-A, 22-B) | `client/protocol/client/src/http/launch_warning.rs` (new), `client/protocol/client/src/http/launch.rs`, `client/protocol/client/src/http/hardware_metrics.rs`, `client/protocol/client/src/http/mod.rs` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |
| **22-D: Nightjar Desktop (Notices, Overlay & `FrameLimiterStatus` UI)** | **Stage 2** (after 22-C; parallel with 22-E) | `client/protocol/desktop/src/app/launch.rs`, `client/protocol/desktop/src/stream/notices.rs`, `client/protocol/desktop/src/app/overlay.rs`, `client/protocol/desktop/src/ui/host_details_limiter.rs` (new), `client/protocol/desktop/src/ui/control_center_cards.rs`, `client/protocol/desktop/src/strings_70/*.rs`, `client/protocol/desktop/src/strings/tests.rs` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |
| **22-E: Nightjar Android (`LaunchWarningParser`, `NvHTTP`, `psmap`, Overlay)** | **Stage 2** (after 22-C; parallel with 22-D) | `client/app/src/main/java/com/limelight/nvstream/http/LaunchWarningParser.java` (new), `client/app/src/main/java/com/limelight/nvstream/http/NvHTTP.java`, `client/app/src/main/java/com/limelight/nvstream/ConnectionContext.java`, `client/app/src/main/java/com/limelight/nvstream/http/HardwareMetrics.java`, `client/app/src/main/java/com/limelight/ui/PerformanceOverlay.java`, `client/app/src/main/java/com/limelight/Game.java`, `client/app/src/test/java/com/limelight/nvstream/http/LaunchWarningParserTest.java` (new), `client/app/src/main/res/values*/strings.xml` `[resolve post-sprint via docs/code-map.md & client/docs/code-map.md]` |

---

## 7. Verification & Measurement Plan

1. **Unit & Sanitizer Matrix (No GPU Required):**
   - `cargo test -p butterpollo-core --locked launch_warning` & `cargo test -p nightjar-core --locked launch_warning launch hardware_metrics` — full `tst_launchwarning.cpp` matrix (DTD rejection, `>16 KiB` rejection, duplicate tags, orphan message, Cc/Cf stripping keeping ZWJ/ZWNJ, `\r`/`\t`/`U+2028`/`U+2029` normalization with max 3 `\n`, `>512` UTF-8 byte drop, `&psmap=` masking by `gcmap & 0xFFFF`).
   - `cargo test -p butterpollo-windows --locked telemetry::pdh_dxgi` — unit tests parsing multi-adapter `luid_0xHIGH_0xLOW` PDH paths (never summing across adapters!), ADLX merge, non-AMD DXGI+PDH `GpuSample` synthesis, and first-sample rate omission.
   - `cargo test -p nightjar-desktop --locked host_details_limiter overlay` & `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.nvstream.http.LaunchWarningParserTest` & `cd rust/web && node --test SettingsSchema.test.mjs`.
2. **Hardware Verification (Under `07-hw-verify`):** Verify real Windows PDH+DXGI `gpu_encoder_percent` correlates cleanly with ADLX on RX 7900 XT + Radeon 780M with zero stream latency overhead; non-AMD NVIDIA DXGI+PDH synthesis verified via mock/unit tests and Windows PDH/DXGI API headers only (never claimed executed on NVIDIA GPU hardware).
