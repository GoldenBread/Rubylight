# Item 07: Serialized Hardware Verification & Measurement Plan (`hw-verify`)

Status: design, not implemented (Wave 1, Rubylight & Nightjar 7.0.0). Written against `main` `b3d0c26b9b` (`6.2.0`), [protocol reservations](00-protocol-reservations.md), `tools/hw/catalog.yaml`, `rust/release/README.md`, and `docs/encoder-focus-design.md` Section 9.

## 1. Summary & User Experience

Wave 1 of Rubylight & Nightjar 7.0.0 ships five performance- and quality-critical items across host GPU, capture, encode, stream delivery, and desktop client decode/presentation:

- **Item 03 (`launch-curtain`):** Smooth launch progress curtain and clean handoff into the live game frame.
- **Item 04 (`frame-warp-2`):** Frame Warp 2.0 per-frame camera velocity, gyro-assisted warp, confidence gating, and HUD depth hints (`0x5549`).
- **Item 05 (`stream-dvr`):** Zero-reencode host-side Stream DVR & Instant Replay (`0x554A`).
- **Item 06 (`hdr-sdr-tonemap`):** Real-time single-pass GPU BT.2390 HDR-to-SDR tone mapping in `color.hlsl`.
- **Item 21 (`desktop-perf-and-nvenc`):** Desktop Client Performance Overhaul (Windows DirectFlip/Waitable/D3D12 decode, Linux Wayland DMA-BUF explicit sync & KMS atomic, macOS direct VideoToolbox/Metal/AVSampleBufferDisplayLayer) & Host NVIDIA NVENC Low-Latency Overhaul.

No performance claim, default-on promotion, or release-note "Tested." quote may ship without serialized, idle-gated **release-build measurements** on real local hardware (**RX 7900 XT desktop**, **Radeon 780M laptop**, and **Intel Linux NUC** via `ssh nuc`), while keeping **NVIDIA NVENC** strictly checked against vendored headers, samples, and fake-driver unit tests (`docs/nvenc.md#checking-nvidia-code`) and **macOS Apple Silicon** strictly checked via `nightjar-macos.yml` CI — **never claiming NVIDIA or Apple Silicon paths were run on hardware**.

Item 07 specifies the deterministic, automated verification harness, `tools/hw/catalog.yaml` entries, ABBA statistical methodology, strict latency budgets, `rust/PERFORMANCE.md` reporting format, and immediate AMD encoder hang (`exit 3`) halt protocol that gate all four Wave 1 features before Wave 2 begins.

---

## 2. Protocol, Native API & Capability Contract

Per [00-protocol-reservations.md](00-protocol-reservations.md) Section 7, Item 07 requires **zero wire, protocol, or Native API changes**:

- `PROTOCOL_VERSION` stays unchanged.
- `rubylight_native_api::API_VERSION` stays `1`.
- No new control IDs, `/serverinfo` elements, or `HostCapabilities::Feature` variants are introduced.
- **Read-only host telemetry contract used by the harness:**
  - Installed-host idle probe: `GET http://127.0.0.1:47989/serverinfo` -> XML elements `<RustHostSessionCount>` and `<RustHostPendingSessionCount>` MUST both equal `0` before every single arm (not just once per batch!).
  - Test-host isolated profile: `butterpollo.exe --config-dir C:\tests\wave1-<item> --port 48123 --bind 127.0.0.1` (and ports `48518..48544` verified free by precondition `isolated-ports`).
  - Host per-frame timing record: `BUTTERPOLLO_TIMING_RECORD=1` parsed by `rust/release/timing_report.py`.
  - Host 5-second `stream timings` log lines (`present_to_sent`, `capture_age_at_submit`, `slice_delivery_overflows`) + feature-specific separate log lines (`stream dvr`, `hdr tonemap`, `frame warp`) so existing `timing_report.py` parsers never break.
  - Nightjar headless test client: `nightjar-cli` (`NIGHTJAR_CLI_FRAME_WARP`, `NIGHTJAR_CLI_LATENCY_CSV`) and independent `moonlight-common-c` receiver (`C:\src\butterpollo-release\fixtures\moonlight-client.exe`).

---

## 3. Architecture & <=800-LOC Module Plan

### 3.1 Harness Architecture & Serialization Guarantees

```text
tools/hw/run.ps1 -Batch desktop (or -Only wave1-*)
   │
   ├── Precondition Gate (every case, in exact catalog.yaml order):
   │     1. idle           -> /serverinfo SessionCount==0 & PendingCount==0,
   │                          user_idle.py >= 10 min, no fullscreen app
   │     2. isolated-ports -> 48123 and 48518..48544 free
   │     3. gpu-lock       -> atomic acquisition of C:\src\butterpollo-release\GPU-LOCK
   │     4. quiet-cpu      -> machine_load.py (<30% CPU over 5 s, no cargo/rustc/java/gradle/ninja)
   │
   ├── Serialized Wave 1 Suites (ONE arm at a time; never parallel GPU runs!):
   │     ├── wave1-curtain-ab      (Item 03: handoff latency + stock client parity)
   │     ├── wave1-frame-warp2     (Item 04: look_probe --fov + frame_warp_accuracy.py)
   │     ├── wave1-dvr-zero-stall  (Item 05: ABBA off vs armed+clip-flush + ffprobe sync)
   │     └── wave1-tonemap-golden  (Item 06: GPU vs CPU golden LSB + ABBA D3D12/D3D11 timing)
   │
   ├── Hang Sentinel (after EVERY leg & on failure):
   │     gpu_hang.py checks encoder call >= 2 s, stall watch >= 5 s,
   │     or new C:\Windows\LiveKernelReports watchdog dump.
   │     ANY HIT -> write gpu_hang to result.json -> EXIT 3 -> ABORT ENTIRE BATCH!
   │
   └── Aggregator (`rust/release/wave1_verify.py summarize`):
         Combines RX 7900 XT + Radeon 780M JSON outputs -> pass/opt-in/drop verdict
         + emits dated markdown block ready for `rust/PERFORMANCE.md`.
```

### 3.2 Second-GPU Execution (`laptop-host` / Radeon 780M)

- Uses the existing `tools/hw/laptop-host.ps1` (`ramaz@192.168.4.12`) locking discipline documented in `rust/release/README.md`:
  - Acquires `C:\src\butterpollo-release\LAPTOP-LOCK` (mutually exclusive with `smoke.ps1`) and the laptop's `BUILD-LOCK`.
  - Verifies laptop console user idle >= 120 s via `user_idle.py`, laptop installed host has 0 sessions, and no process runs from the swapped package directory.
  - Searches the laptop's host log since start for AMD encoder hang after every leg; exit code `3` aborts immediately; exit code `4` reports laptop busy/not idle (not a code failure).
- **Direction-agreement rule (from `encoder-focus-design.md` M4):** Every Wave 1 quality/accuracy improvement and every timing budget MUST hold on **both** the RX 7900 XT desktop and the Radeon 780M laptop (or be explicitly documented as limited to one GPU before shipping).

### 3.3 `<=800-LOC` Module Plan & `size_ratchet.json` Compliance

Note that Python/YAML files in `tools/` and `rust/release/` are outside `arch_check.py`'s `.rs/.java/.kt/.c/.cpp/.h/.hpp/.ts/.svelte` list, yet we still keep every new harness module `<= 500` LOC for clarity, testability, and parallel edit safety. Any new Rust GPU probe binary under `rust/windows/examples/` MUST stay `<= 800` LOC and open with a `//!` header!

#### Grandfathered files touched (from `tools/size_ratchet.json`)

| Grandfathered file | Ratchet ceiling | How we avoid growing it |
|---|---|---|
| `rust/windows/examples/look_probe.rs` `[verify post-sprint code-map.md]` | `1540` | **Do NOT grow `look_probe.rs`!** Put Frame Warp 2.0 synthetic gyro/velocity curve generation in a new `<=400-LOC` helper `rust/windows/examples/look_probe_v2.rs` (or a sibling module `examples/look_probe/v2.rs` if converted) leaving `look_probe.rs` at `<=1540` LOC. |
| `client/protocol/cli/src/main.rs` `[verify post-sprint code-map.md]` | `1744` | Put Wave 1 CLI verification flags (`--dvr-clip-at`, `--sdr-tonemap`, `--fw2-csv`) in new `client/protocol/cli/src/wave1.rs` (<=8 lines in `main.rs`, balanced by extracting an existing helper if needed). |

#### New `<=800-LOC` files & catalog additions

| Planned Path | Est. LOC | Ownership |
|---|---|---|
| `rust/release/wave1_verify.py` `[verify post-sprint code-map.md]` | ~420 | Orchestrator for Wave 1 ABBA runs (`curtain`, `frame-warp2`, `dvr`, `tonemap`, `summarize`), enforcing per-arm idle check, `machine_load.py`, `gpu_hang.py`, and exit-code-3 abort |
| `rust/release/wave1_gates.py` `[verify post-sprint code-map.md]` | ~280 | Pure statistical ABBA noise calculator (`max(|A1-A2|, |B1-B2|)`), budget evaluator (`+0.05/+0.10/+0.20 ms`), DVR `ffprobe` sync verifier, HDR golden diff checker, `PERFORMANCE.md` markdown emitter |
| `rust/release/test_wave1_verify.py` `[verify post-sprint code-map.md]` | ~320 | Deterministic pytest/unittest suite over synthetic timing JSONL, DVR ffprobe JSON, HDR golden fixtures, and exit-code-3 abort behavior (zero GPU required) |
| `rust/windows/examples/hdr_tonemap_probe.rs` `[verify post-sprint code-map.md]` | ~360 | Standalone `#[allow]`-free probe generating synthetic scRGB FP16 & HDR10 PQ ramp/color-checker/specular textures, running D3D12 `compute::Converter` + D3D11 `gpu_color::Converter`, comparing against CPU reference (`butterpollo_core::sdr_tonemap`), outputting JSON metrics + side-by-side PNGs |
| `rust/windows/examples/look_probe_v2.rs` `[verify post-sprint code-map.md]` | ~340 | Frame Warp 2.0 synthetic camera + gyro/velocity + depth-layered HUD probe emitting `OUT.json` + `OUT.frames.csv` compatible with `frame_warp_accuracy.py` |
| `tools/hw/catalog.yaml` `[verify post-sprint code-map.md]` | +110 | 8 new `wave1-*` catalog entries (`desktop` and `laptop-host` pairs for Items 03, 04, 05, 06), initially `status: pending` until Wave 1 binaries land on `main`, then flipped to `ready` |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### 4.1 Hardware Runner State Machine

```text
  [Init] ──► [Check Idle & Ports] ──(busy)──► EXIT 4 (deferred, no failure)
                     │ (clean)
                     ▼
             [Acquire GPU-LOCK + quiet CPU wait (<=15 min)]
                     │
                     ▼
             [Start Isolated Host (--port 48123 --bind 127.0.0.1)]
                     │
                     ▼
             [Run ABBA Arm: A1 -> B1 -> A2 -> B2]
                     │
                     ├──(each arm: check gpu_hang.py)
                     │       │
                     │       ├──(HANG DETECTED)──► [Kill test host, save gpu_hang]
                     │       │                     ──► [Release GPU-LOCK]
                     │       │                     ──► EXIT 3 (ABORT BATCH — NO RETRY!)
                     │       │
                     │       └──(clean)──► [Record arm result.json & timing.jsonl]
                     │
                     ▼
             [Stop Isolated Host & Verify Zero Stray Processes]
                     │
                     ▼
             [Release GPU-LOCK & Write Aggregated Verdict] ──► EXIT 0 (PASS) / EXIT 1 (FAIL)
```

### 4.2 Fault & Environment Matrix

| Condition | Runner Action | Exit Code |
|---|---|---|
| Installed host has `RustHostSessionCount > 0` or `RustHostPendingSessionCount > 0` | Refuse immediately before touching GPU or starting test host | `4` (busy) |
| Desktop user active within 10 min, fullscreen app running, or laptop user active within 120 s | Refuse immediately | `4` (busy) |
| `GPU-LOCK` or `LAPTOP-LOCK` held by another runner | Wait up to 30 min (warn every 60 s); NEVER break an active lock | `4` (timeout) |
| `LOCK LOST` detected (another process altered/deleted lock) | Fail case loudly with `lock_warnings`, stop remaining arms | `1` (fail) |
| CPU load `>= 30%` over 5 s or `cargo`/`rustc`/`java`/`gradle`/`ninja` active | Wait up to 15 min (`quiet_wait`), log `machine_load` in `result.json` | Continues with warning after 15 m |
| Encoder call `>= 2 s`, stall watch unresolved `>= 5 s`, or new `C:\Windows\LiveKernelReports` watchdog dump | Kill isolated host, record `gpu_hang`, release locks, **abort entire batch with zero retries** | **`3` (AMD hang)** |
| Audio sink muted or `< 10%` volume | Log loud warning in `result.json` `audio_sink` (never mutate user volume) | Recorded warning |
| Test host fails to exit cleanly after arm | Force-terminate test host PID only (NEVER touch installed `ApolloService` / `47989`), clean `C:\tests\wave1-<item>` temp files | `1` (fail) |

### 4.3 Isolation & Crash-Restore Guarantees

- **Zero mutation of installed host or user displays:** Isolated test hosts ALWAYS use:
  - `--config-dir C:\tests\wave1-<item> --port 48123 --bind 127.0.0.1`
  - `virtual_display_layout = extended`, `dd_configuration_option = disabled`, `virtual_display_mode = disabled` (except the single SYSTEM-elevated HDR virtual display verification case, which runs through `ButterpolloReleaseElevated` and asserts original display topology is restored identically after teardown).
- **Lock cleanup in `finally`:** Every runner script (`wave1_verify.py`, `run.py`, `laptop-host.ps1`) releases only locks whose owner PID/timestamp matches itself inside a `try ... finally` block; forcibly killed runs require manual owner inspection before clearing locks (`rust/release/README.md`).

---

## 5. Settings & 10/10 Craft Plan

Item 07 is internal release/verification tooling and adds **zero user-facing settings**, **zero UI text**, **zero locale strings**, **zero Nightjar preference keys**, and **zero `"Experimental"` labels**.

### Craft & Integrity Standards for Measurement Reports

- **Fewer-settings & default-off enforcement:** `wave1_verify.py summarize` explicitly encodes the default-on promotion gates for Items 03, 04, 05, and 06. Any feature that fails its timing or quality gate is programmatically marked `opt_in` or `drop` — never silently defaulted on.
- **Zero NVIDIA hardware claims:** Every generated `rust/PERFORMANCE.md` section and `VALIDATION.json` summary MUST include an explicit footnote: `"NVIDIA NVENC was checked against vendored SDK headers/samples and fake-driver tests only; not run on NVIDIA hardware."`
- **No mod names:** Verification fixtures and reports use synthetic test generators (`motion_probe`, `look_probe`, `hdr_tonemap_probe`, Xiph Twitch raw Y4M clips) and never name third-party mods.
- **Read summaries, never raw log dumps:** Runners inspect `result.json`, per-arm `timing_report.py --json`, and 5 s summary lines (`stream timings`, `stream dvr`, `hdr tonemap`, `frame warp`), avoiding raw full-log scraping.

---

## 6. Disjoint Implementer Slices

All slices have **strictly non-overlapping file lists** and can be built in parallel with Wave 1 feature slices (just like `M-harness` in `docs/encoder-focus-design.md` Section 11!).

| Slice | Parallel Group | Exclusive File List (zero overlap!) |
|---|---|---|
| **7A: Statistical Gate Engine & `PERFORMANCE.md` Formatter** | Stage 1 (parallel with Items 03–06) | `rust/release/wave1_gates.py` (new), `rust/release/test_wave1_verify.py` (new) |
| **7B: Wave 1 ABBA Runner & Catalog Cases** | Stage 2 (after 7A) | `rust/release/wave1_verify.py` (new), `tools/hw/catalog.yaml` |
| **7C: GPU Tone-Map Golden Probe (`hdr_tonemap_probe`)** | Stage 2 (after Item 06's Slice 6C) | `rust/windows/examples/hdr_tonemap_probe.rs` (new) |
| **7D: Frame Warp 2.0 Synthetic Probe & CLI Hooks** | Stage 2 (after Item 04 lands) | `rust/windows/examples/look_probe_v2.rs` (new), `client/protocol/cli/src/wave1.rs` (new), `client/protocol/cli/src/main.rs` |

*(Every path above is marked `[verify post-sprint code-map.md]` for Phase 2 reconciliation.)*

---

## 7. Verification & Measurement Plan

### 7.1 Strict Default-Path Latency Budget (All Wave 1 Items)

For every ABBA quartet (`A1 = off/baseline`, `B1 = feature`, `A2 = off/baseline`, `B2 = feature`), define run noise `N = max(|A1 - A2|, |B1 - B2|)` and effect `Delta = mean(B1, B2) - mean(A1, A2)`. **Every limit below MUST pass on BOTH RX 7900 XT desktop and Radeon 780M laptop:**

| Metric | Source | Hard Limit (Default Path / Feature Active Where Specified) |
|---|---|---|
| **Convert p99** | `timing_report.py` (`BUTTERPOLLO_TIMING_RECORD=1`) | `<= +0.05 ms` on default path (and `<= +0.15 ms` when HDR-to-SDR BT.2390 is actively converting 4K60 HDR) |
| **Encode p99** | `timing_report.py` | `<= +0.10 ms` |
| **`present_to_sent` p99** | Host 5 s `stream timings` line | `<= +0.20 ms` (within measurement noise) |
| **`capture_age_at_submit` p99** | Host 5 s `stream timings` line | `<= +0.10 ms` (no capture thread backpressure) |
| **`slice_delivery_overflows`** | Host 5 s `stream timings` line | `0` increase vs. baseline arms |
| **Audio drops / thread stalls** | `result.json` + `stall_watch.rs` | `0` new drops or stalls |

### 7.2 Per-Item Wave 1 Hardware Catalog & Acceptance Gates

| Catalog ID | Device | Workload & Tools | Acceptance Gate |
|---|---|---|---|
| `wave1-curtain-desktop` / `wave1-curtain-laptop` | `desktop` / `laptop-host` | Item 03 (`launch-curtain`): `e2e.py --record` + `nightjar-cli` (5 launch cycles with `rlLaunchCurtain=1` + 1 stock `moonlight-client.exe` control) | Curtain lifts within `<= 1` frame of first game frame; post-lift steady-state timing within default budget (`+0.05/+0.10/+0.20 ms`); stock client receives zero `0x5548` messages. |
| `wave1-fw2-desktop` / `wave1-fw2-laptop` | `desktop` / `laptop-host` | Item 04 (`frame-warp-2`): `look_probe_v2 --fov 90` + `nightjar-cli` (`NIGHTJAR_CLI_FRAME_WARP`) -> `frame_warp_accuracy.py --json` + ABBA `e2e.py --record` | Displacement `removed` >= Frame Warp v1 baseline on both centre and edge (`lens`); HUD mask false-positive rate `<= 2%`; default-off path within `+0.05/+0.10/+0.20 ms`. |
| `wave1-dvr-desktop` / `wave1-dvr-laptop` | `desktop` / `laptop-host` | Item 05 (`stream-dvr`): 90 s ABBA `e2e.py --record` at 1080p60 HEVC 20 Mbps & 4K60 HEVC 60 Mbps (A: `dvr_enabled=false`, B: `dvr_enabled=true`, 60 s ring, 3 `POST /dvr/clip` saves at `t=30,55,80 s` + 1 manual record arm) + PyroWave refusal arm | Default-off delta `0.00 ms`; active DVR + clip flushes within `+0.05/+0.10/+0.20 ms`, `0` `dvr_tap_drops`, `0` audio drops; every saved `.mp4` passes `ffmpeg -v error -i clip.mp4 -f null -` with A/V sync drift `<= 20 ms`; PyroWave arm returns `unsupported` (`pyrowave`). |
| `wave1-tonemap-desktop` / `wave1-tonemap-laptop` | `desktop` / `laptop-host` | Item 06 (`hdr-sdr-tonemap`): `hdr_tonemap_probe` (D3D12 + D3D11 over `RgbaF16` & `Rgba10Pq` ramps/color-checker/SDR-white fixtures) + 90 s ABBA `e2e.py --record` on SDR-on-SDR and HDR-to-SDR (4K60 HEVC 40 Mbps & 1080p60 HEVC 20 Mbps) | GPU vs. CPU reference max pixel error `<= 1` LSB; pure SDR desktop patch error `<= 1/255` (no dimming); 1000–4000 nit ramp strictly monotonic with `0` flat-clip collapse; SDR-on-SDR convert p99 `<= +0.05 ms` (`yuv420_cs` DXBC hash unchanged); active 4K60 HDR-to-SDR convert p99 `<= +0.15 ms`, `present_to_sent` p99 `<= +0.20 ms`. |
| `wave1-desktop-perf-win` | `desktop` + `laptop` | Item 21 (`desktop-perf-and-nvenc` Windows client): 90 s ABBA release-build `nightjar.exe` runs (`NIGHTJAR_WIN_DIRECTFLIP=0` vs DirectFlip Waitable + D3D12/D3D11 fast path) at 1080p120 & 4K60 HEVC | Confirmed `DirectFlip`/Independent Flip + Waitable Object active, `0` `DXGI_ERROR_DEVICE_REMOVED` events, measured `decode_to_present_us` p50/p99 improvement or parity without added jitter, Control Center handoff `<= 1` frame. |
| `wave1-desktop-perf-linux-nuc` | `nuc` (`ssh nuc`) | Item 21 (`desktop-perf-and-nvenc` Linux client): 90 s ABBA release-build `nightjar` on Intel NUC (Wayland `wl_subsurface` DMA-BUF explicit sync & KMS atomic fast path vs `NIGHTJAR_WAYLAND_DMABUF_DIRECT=0`) | `>= 95%` zero-copy direct scanout hit rate fullscreen, `0` CPU map fallbacks, `0` Wayland protocol errors on Control Center open/close, measured `decode_to_present_us` p50/p99 recorded in `rust/PERFORMANCE.md`. *(macOS verified via `nightjar-macos.yml` CI only; NVIDIA NVENC verified via SDK 13.0 headers & fake-driver tests only.)* |

### 7.3 NVIDIA NVENC Verification Protocol (Non-Hardware)

For every Wave 1 item touching `rust/windows/src/nvenc.rs` or shared converters (`compute.rs`, `gpu_color.rs`, `encoder.rs`):

1. Verify every NVENC struct field, API version gate (11/12/13), and call ordering against vendored `nvEncodeAPI.h` (`rust/THIRD_PARTY.md`) per `docs/nvenc.md#checking-nvidia-code` (author check + independent reviewer check naming exact header paths/lines).
2. Run full fake-driver suite: `cargo test -p butterpollo-windows --locked nvenc`.
3. Record under `"Not run on NVIDIA hardware"` in `rust/PERFORMANCE.md` and release notes — **never imply or claim NVIDIA hardware execution**.

### 7.4 `rust/PERFORMANCE.md` Dated Reporting Format

Once both GPUs pass, `wave1_verify.py summarize` outputs a standardized dated section appended to `rust/PERFORMANCE.md`:

```markdown
## <Month Day>: Wave 1 (7.0.0) — Launch Curtain, Frame Warp 2.0, Stream DVR & HDR-to-SDR Tone Mapping

Measured on commit `<short-sha>` (release build):
- **Desktop:** AMD Radeon RX 7900 XT, Adrenalin `<driver>`, Windows `<build>`, isolated host `127.0.0.1:48123`.
- **Laptop:** AMD Radeon 780M, Adrenalin `<driver>`, Windows `<build>`, `laptop-host.ps1`.
- **NVIDIA NVENC:** Verified against vendored SDK 13.0 headers and `nvenc/tests.rs` fake-driver tests only; not run on NVIDIA hardware.

### Default-path timing budget (ABBA, 90 s arms, `e2e.py --record`)
| Feature & Mode | GPU | Convert p99 (Δ / noise) | Encode p99 (Δ / noise) | present_to_sent p99 (Δ / noise) | Overflows | Verdict |
|---|---|---|---|---|---|---|
| ... | ... | ... | ... | ... | ... | PASS |

### Feature-specific quality & accuracy metrics
- **Frame Warp 2.0:** `removed` centre / whole / edge (vs. v1 baseline), HUD mask FP/FN.
- **Stream DVR:** clip save flush time, RAM high-water mark, `dvr_tap_drops`, `ffprobe` A/V sync offset.
- **HDR-to-SDR Tone Mapping:** D3D12/D3D11 max LSB error vs. CPU reference, SDR-white preservation, highlight ramp headroom.
```

### 7.5 Narrowest Non-GPU Checks & Exit Code 3 Rule

- **Harness unit tests (no GPU):** `python -m unittest rust/release/test_wave1_verify.py` and `python tools/hw/run.py --validate-catalog` (validates YAML schema, preconditions, and `{{var}}` substitutions).
- **Mandatory Exit Code 3 Abort Rule:** If ANY leg of `wave1_verify.py`, `e2e.py`, `check.ps1`, or `laptop-host.ps1` exits with code **`3`**, an AMD video engine hang occurred (`gpu_hang.py`). The runner **stops immediately**, writes `gpu_hang` into `result.json`, releases `GPU-LOCK`/`LAPTOP-LOCK`, and exits `3`. Agents and operators MUST investigate the hang (and reboot if Device Manager reports Code 31) and **NEVER retry** the failed run.
