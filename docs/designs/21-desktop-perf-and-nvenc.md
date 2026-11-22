# Item 21: Desktop Client Performance Overhaul (macOS, Linux, Windows) & Host NVIDIA NVENC Low-Latency Overhaul (Wave 1)

Status: engineering design (Phase 1). Written against `main` `b3d0c26b9b` (`6.2.0`), [`00-protocol-reservations.md`](00-protocol-reservations.md), [`docs/nvenc.md`](../nvenc.md), and [`client/docs/code-map.md`](../../client/docs/code-map.md).

## 1. Summary & User Experience

Item 21 delivers a **major, architecture-level latency, decode-throughput, and frame-pacing overhaul** across **all three Nightjar 7.0 desktop platforms (macOS, Linux, Windows)** and **Rubylight 7.0's host NVIDIA NVENC encoding path** — without compromising compatibility, without breaking any grandfathered `tools/size_ratchet.json` ceiling, and with **zero wire-protocol changes**:

1. **macOS Desktop Client (`client/protocol/desktop` — Apple Silicon & Intel macOS 13+):**
   - Takes clean architectural inspiration from native Apple Silicon streaming clients such as **Glimmer (`Se7enbrc/glimmer`, GPL-3.0)** and **Nonary's moonlight-qt (GPL-3.0)** — implemented independently in Rust + Objective-C/Metal (with full GPL-3.0 attribution in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README credits if any code is directly adapted; never in UI text or release notes).
   - **Direct Native VideoToolbox (`VTDecompressionSession`) Zero-FFmpeg-Hop Decoder (`stream/macos_vt.rs` + `stream/macos_vt.m`):** Feeds received H.264, HEVC (8-bit/10-bit/4:4:4), and AV1 access units directly into `CMBlockBuffer` / `CMSampleBuffer` -> `VTDecompressionSessionDecodeFrame` (`kVTDecompressionPropertyKey_RealTime = true`, `kVTDecodeFrame_1xRealTimePlayback`, strict `<= 1`-frame decode queue depth, zero B-frame reorder delay), decoding directly into `IOSurface`-backed `CVPixelBuffer` (`kCVPixelBufferMetalCompatibilityKey = true`, `NV12` / `P010` / `444f`). Falls back cleanly and visibly to existing FFmpeg `media.rs` VideoToolbox/SW on unsupported profiles.
   - **Dual Zero-Copy Presentation Fast Path (`stream/macos_metal_present.rs` + `stream/macos_metal_present.m`):**
     - **Path A — `AVSampleBufferDisplayLayer` Direct Hardware Plane Scanout:** Automatically engaged when shaders, MetalFX, Frame Warp, and overlays/Control Center are inactive — pushes decoded `IOSurface` / `CMSampleBuffer` straight to macOS WindowServer's hardware video plane (native EDR/HDR10 PQ + Dolby Vision + ProMotion zero-copy scanout, eliminating both SDL draw passes and intermediate Metal texture passes). Hands off cleanly back to Path B in `<= 1` frame the moment Control Center, Stats HUD, MetalFX, or Frame Warp opens.
     - **Path B — `CVMetalTextureCache` -> `CAMetalLayer` Single-Pass Zero-Copy Metal Presenter:** Maps `CVPixelBuffer`/`IOSurface` planes directly as zero-copy `MTLTexture`s (`R8Unorm`/`RG8Unorm` or `R16Unorm`/`RG16Unorm`) via `CVMetalTextureCacheCreateTextureFromImage`, executing YCbCr->RGB/EDR + optional `MetalFX` (`stream/metalfx.rs`) + optional Frame Warp 2.0 + blur-free HUD in **one** `MTLCommandBuffer`, using `presentsWithTransaction = NO`, `displaySyncEnabled = NO` for unlocked/VRR mode, or ProMotion `CADisplayLink` / `NSScreen` `DisplayTiming` (`mac_display.rs`) `presentAtTime` pacing.
2. **Linux Desktop Client (`client/protocol/desktop` — Intel / AMD / ARM Linux, hardware-verified on Intel NUC via `ssh nuc`):**
   - **Wayland Zero-Copy DMA-BUF Subsurface + Explicit Sync (`stream/linux_dmabuf_direct.rs` + `stream/linux_syncobj.rs`):** On Wayland compositors advertising `zwp_linux_dmabuf_v1` + `wp_linux_drm_syncobj_manager_v1` (KWin 6.2+, Mutter 47+, wlroots/Hyprland, gamescope), attaches VA-API / Vulkan Video / V4L2 M2M decoded `dmabuf::Descriptor` frames directly as `wl_buffer`s on a dedicated video `wl_subsurface` with explicit acquire/release timeline sync points (`wp_linux_drm_syncobj_surface_v1` + `DMA_BUF_IOCTL_EXPORT_SYNC_FILE`), avoiding **both** the extra GPU composition pass AND implicit fence stalls while **never** colliding with Mesa Vulkan WSI's single-per-surface `wp_linux_drm_syncobj_surface_v1` / `wp_color_management_surface_v1` objects on the main window surface!
   - **DRM/KMS Atomic Zero-Copy Fast Path (`stream/kms_atomic_fast.rs`):** Extends `kms_scanout.rs` (without growing grandfathered `kms_scanout.rs`) with `DRM_MODE_ATOMIC_NONBLOCK`, explicit `IN_FENCE_FD` sync, `DRM_MODE_PAGE_FLIP_ASYNC` tearing flip when `plan::may_tear`, and a per-surface `drmModeAddFB2WithModifiers` framebuffer LRU cache keyed by DMA-BUF inode + modifier to eliminate per-frame `ADDFB2`/`RMFB` ioctl churn.
   - **V4L2 M2M Zero-Copy DMA-BUF Decoder (`stream/v4l2_m2m.rs`):** Optional hardware decode backend (`V4L2_MEMORY_DMABUF`) for Linux ARM/SBC devices feeding the identical `dmabuf::Descriptor` zero-copy Wayland/KMS pipeline (cleanly skipped with zero overhead on x86 Intel/AMD systems without M2M nodes).
3. **Windows Desktop Client (`client/protocol/desktop` — hardware-verified on RX 7900 XT Desktop & Radeon 780M Laptop):**
   - **DirectFlip / Independent Flip / MPO + Waitable Swapchain Pacer (`stream/win_directflip.rs`):** Configures the streaming `IDXGISwapChain3` with `DXGI_SWAP_EFFECT_FLIP_DISCARD`, `DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT | DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING`, `SetMaximumFrameLatency(1)`, and waits directly on `GetFrameLatencyWaitableObject` immediately before frame submission — eliminating DWM 1–2 frame swapchain queue build-up while enabling borderless Independent Flip / Hardware MPO (`DXGI_OVERLAY_SUPPORT_FLAG_DIRECT`) whenever overlays/Control Center are closed.
   - **Safe D3D12 Video Zero-Copy Decode Path (`stream/win_d3d12_decode.rs`):** Solves the historical Nightjar `#112` Intel D3D11VA texture-array concurrent-bind device-removal hazard cleanly by introducing a native **D3D12 Video Decode (`AV_PIX_FMT_D3D12` / `ID3D12VideoDecoder` with explicit subresource state barriers + GPU fence synchronization)** zero-copy path alongside the existing safe D3D11 `CopySubresourceRegion` -> DirectFlip path (sticky automatic fallback on first init refusal or `DXGI_ERROR_DEVICE_REMOVED`, never looping).
   - **Low-Latency Decode-to-Present Handoff:** Replaces sluggish UI-poll wake latency between `pipeline.rs` decode completion and swapchain presentation with an event-driven wake + zero-copy surface lease handoff so `decode_to_present` stays sub-millisecond in Immediate/VRR mode.
4. **Host NVIDIA NVENC Low-Latency Encoding Overhaul (`rust/windows/src/nvenc/*` + `rust/core/src/nvenc.rs`):**
   - Extracts new and refactored logic strictly into modular `<= 800`-LOC submodules under `rust/windows/src/nvenc/` so grandfathered `nvenc.rs` (**3711** LOC ceiling) **shrinks** and `nvenc/tests.rs` (**3461** LOC ceiling) never grows!
   - **D3D12 Zero-Copy Pre-Registered Ring & Async Submission Overlap (`nvenc/d3d12_async.rs`):** Pre-registers the D3D12 conversion input texture pool and `READBACK` output ring once per session (removing per-frame `nvEncRegisterResource`/`nvEncUnregisterResource` churn), chains GPU-GPU fences (`inputFencePoint` -> encode -> `outputFencePoint`) with bounded async overlap (`max_in_flight = 2`, or `1` in Lowest-Latency preset) and event-driven `SetEventOnCompletion` wake (`nvenc_output_event`) eliminating 100 µs poll jitter.
   - **Ultra-Low-Latency Rate Control, Single-Frame VBV & Adaptive AQ Audit (`nvenc/rate_control.rs` + `rust/core/src/nvenc.rs`):** Hardens `NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY`, single-frame `vbvBufferSize`/`vbvInitialDelay`/`lowDelayKeyFrameScale = 1`, `NV_ENC_TWO_PASS_QUARTER_RESOLUTION` vs `NV_ENC_MULTI_PASS_DISABLED` selection when frame budget guard approaches ceiling, in-place dynamic bitrate reconfigure (`NV_ENC_CAPS_SUPPORT_DYN_BITRATE_CHANGE`) without pipeline drain, spatial AQ (`enableAQ = 1`, `aqStrength`), and SDK 13.0-compliant opt-in temporal AQ (`enableTemporalAQ`, strictly gated on zero lookahead & zero B-frames).
   - **Split-Frame, Intra-Refresh & LTR-RFI Recovery Tuning (`nvenc/recovery_tuning.rs`):** Enforces API 12.2+ `splitEncodeMode` mutual-exclusion invariants with `enableSubFrameWrite` and HEVC weighted prediction, tightens LTR Reference Frame Invalidation (`nvEncInvalidateRefFrames` with explicit DPB slot tracking to prevent unnecessary full IDR resets), and coordinates periodic intra-refresh (`enableIntraRefresh`) cleanly when slice delivery is off.
   - **Native 4:4:4 Pooled CUDA Interop & RTX HDR / RTX Video Zero-Copy Pool Interop (`nvenc/format_444_rtx.rs`):** Pools & pre-registers `nvenc/cuda.rs` 8-bit `AYUV` and 10-bit stacked `R16` -> `YUV444_10BIT` surfaces across frames, and connects `rust/windows/src/{truehdr,rtx_video}.rs` enhanced textures directly into the NVENC input pool via fence-synchronized shared surfaces (avoiding full encoder session teardown/recreate stalls when resolution and pixel format stay identical, while always failing visibly and never silently switching encoders).

### Strict Hardware Honesty Contract (`AGENTS.md`)

| Target | Local Hardware Available? | Required Verification & Claim Discipline |
| --- | --- | --- |
| **Windows Desktop Client** | **YES** (RX 7900 XT desktop, Radeon 780M laptop) | **Must be measured on real release builds (ABBA)** via `07-hw-verify` (`wave1-desktop-perf-win`) before claiming any latency or pacing improvement! |
| **Linux Desktop Client** | **YES** (Intel NUC via `ssh nuc`) | **Must be measured on real Intel NUC hardware** via `07-hw-verify` (`wave1-desktop-perf-linux-nuc`) for DMA-BUF zero-copy Wayland/KMS scanout & decode-to-present latency! Non-Intel Linux GPUs (AMD/NVIDIA Linux) are verified by unit/mock tests and explicitly documented as not locally benchmarked. |
| **macOS Desktop Client** | **NO** (no local Apple Silicon Mac) | Verified strictly via `nightjar-macos.yml` CI (`aarch64-apple-darwin` & `x86_64-apple-darwin` builds, Objective-C `-Wall -Werror`, Rust unit tests for Annex-B/AVCC/HVCC/OBU framing, fast-path eligibility & ProMotion pacer math). **NEVER claim benchmarked or run on local Apple Silicon hardware!** |
| **Host NVIDIA NVENC** | **NO** (no local NVIDIA GPU) | Verified strictly line-by-line against vendored `rust/windows/include/nvEncodeAPI.h` (SDK 13.0) & NVIDIA Video Codec SDK samples (`docs/nvenc.md#checking-nvidia-code`), `rtx-video-sdk-1.1.0` headers, and fake-driver tests (`nvenc/overhaul_tests.rs` + `nvenc/tests.rs`) by both author and independent reviewer. **NEVER claim run or benchmarked on NVIDIA GPU hardware!** |

---

## 2. Protocol, Native API & Capability Contract

Per [`00-protocol-reservations.md`](00-protocol-reservations.md) §7, Item 21 requires **ZERO wire-protocol, control-stream, `/serverinfo`, RTSP, or Native API changes**:

| Surface | Contract |
| --- | --- |
| `PROTOCOL_VERSION` | **Unchanged.** Stock Moonlight and older Nightjar builds interoperate byte-for-byte identically. |
| Control IDs & Input magics | **None added.** |
| `rubylight_native_api::API_VERSION` | **Unchanged (`1`).** Zero new native routes or feature strings. |
| `HostCapabilities` | **Unchanged.** All desktop fast paths activate strictly from local OS/GPU/compositor/decoder runtime capabilities — never from host brand or `Family::Rubylight`! |
| Compatibility preservation | Preserves AMD AMF, Windows 10 & 11, physical displays, WGC, Desktop Duplication, D3D11 fallback, Wayland/X11/KMS, macOS Metal, and existing FFmpeg/SDL fallback paths. Every fast path degrades visibly with a clear single log line on refusal — **never silently switches host encoders and never breaks playback**. |

---

## 3. Architecture & <=800-LOC Module Plan

All repo paths carry `[verify post-sprint code-map.md]` for Phase 2 reconciliation. Every new `.rs` module opens with a `//!` header and stays strictly **`<= 800` LOC**. Every Linux-only module uses `#[cfg(target_os = "linux")]` (never bare `cfg(unix)` that would break macOS CI!), and any shared Unix code carries an explicit OS-intent comment per `tools/arch_check.py`.

### 3.1 Grandfathered Files in `tools/size_ratchet.json` (STRICT CEILING PROTECTION)

| Grandfathered File | Ratchet Ceiling | Discipline in Item 21 |
| --- | ---: | --- |
| `rust/windows/src/nvenc.rs` `[verify post-sprint code-map.md]` | **3711** | **Net SHRINK:** Extract D3D12 async ring, rate-control/VBV/AQ, LTR-RFI/split-encode tuning, and pooled 4:4:4/RTX interop into `nvenc/{d3d12_async,rate_control,recovery_tuning,format_444_rtx}.rs`; target `nvenc.rs <= 3350` LOC! |
| `rust/windows/src/nvenc/tests.rs` `[verify post-sprint code-map.md]` | **3461** | **ZERO lines added:** All new NVENC fake-driver & ABI-version tests land in `rust/windows/src/nvenc/overhaul_tests.rs` (`<= 700` LOC). |
| `client/protocol/desktop/src/stream/pipeline.rs` `[verify post-sprint code-map.md]` | **2013** | Delegate platform decoder selection and low-latency wake hook to leaf modules; net LOC `<= 2013`. |
| `client/protocol/desktop/src/stream/render.rs` `[verify post-sprint code-map.md]` | **1740** | Add clean `FastPresent` trait dispatch hook (<=15 lines balanced by helper extraction); net LOC `<= 1740`. |
| `client/protocol/desktop/src/stream/present.rs` & `present/pacer.rs` `[verify post-sprint code-map.md]` | **986** / **1060** | Keep waitable-swapchain & ProMotion timing helpers in `win_directflip.rs` and `macos_metal_present.rs`; net LOC `<= ceiling`. |
| `client/protocol/desktop/src/stream/d3d11.rs` `[verify post-sprint code-map.md]` | **1125** | **ZERO growth** (untouched by Stage 1; Windows DirectFlip/D3D12 live in dedicated sibling files). |
| `client/protocol/desktop/src/dmabuf.rs`, `vulkan_video.rs`, `stream/wl_scanout.rs`, `stream/kms_scanout.rs` `[verify post-sprint code-map.md]` | **886** / **3307** / **1836** / **969** | **ZERO growth:** Linux DMA-BUF subsurface, explicit syncobj, KMS atomic FB cache, and V4L2 M2M live exclusively in new `<= 800`-LOC sibling modules. |
| `client/protocol/desktop/src/media.rs` `[verify post-sprint code-map.md]` | **2103** | **ZERO growth:** macOS native VideoToolbox and Linux V4L2 M2M implement Nightjar's decoder boundary directly without inflating `media.rs`. |

### 3.2 New & Modified Module Plan (Every New File `<= 800` LOC)

| Path | Target | Est. LOC | Ownership & Responsibility |
| --- | --- | ---: | --- |
| `client/protocol/desktop/src/stream/macos_vt.rs` *(new)* | macOS | ~420 | `//!` Pure-Rust Annex-B->AVCC/HVCC/AV1 OBU packetizer, 0-reorder queue (`depth <= 1`), `VTDecompressionSession` Rust wrapper + cross-platform unit tests. |
| `client/protocol/desktop/src/stream/macos_vt.m` *(new)* | macOS | ~340 | ARC Objective-C bridge (`-Wall -Werror`): `VTDecompressionSession` creation (`kVTDecompressionPropertyKey_RealTime`), `IOSurface`/`CVPixelBuffer` output callback, clean session invalidation & error codes. |
| `client/protocol/desktop/src/stream/macos_metal_present.rs` *(new)* | macOS | ~380 | `//!` Dual-path selector (`AVSampleBufferDisplayLayer` direct video plane vs `CVMetalTextureCache` -> `CAMetalLayer`), ProMotion `presentAtTime` pacer on `mac_display::DisplayTiming`, cross-platform state-machine tests. |
| `client/protocol/desktop/src/stream/macos_metal_present.m` *(new)* | macOS | ~390 | ARC Objective-C/Metal bridge: `AVSampleBufferDisplayLayer` zero-copy plane enqueue + atomic 1-frame handoff to `CAMetalLayer`, `CVMetalTextureCacheCreateTextureFromImage`, single-pass YCbCr/EDR/MetalFX command buffer. |
| `client/protocol/desktop/src/stream/macos_vt_tests.rs` *(new)* | Cross-plat | ~260 | `//!` Deterministic unit tests (run on Windows/Linux/macOS CI!) verifying H.264/HEVC/AV1 framing, `<= 1` queue stale-frame drop, `AVSampleBufferDisplayLayer` <-> `CAMetalLayer` switch triggers, and ProMotion `presentAtTime` math. |
| `client/protocol/desktop/src/stream/linux_dmabuf_direct.rs` *(new)* | Linux | ~460 | `//!` `#[cfg(target_os = "linux")]` Dedicated Wayland `wl_subsurface` + `zwp_linux_dmabuf_v1` zero-copy video path avoiding any collision with Mesa WSI's main-surface objects; atomic fallback to renderer when Control Center/shaders/warp activate. |
| `client/protocol/desktop/src/stream/linux_syncobj.rs` *(new)* | Linux | ~290 | `//!` `#[cfg(target_os = "linux")]` `wp_linux_drm_syncobj_manager_v1` / `wp_linux_drm_syncobj_surface_v1` timeline acquire/release syncpoint & `DMA_BUF_IOCTL_EXPORT_SYNC_FILE` helper + protocol-state unit tests. |
| `client/protocol/desktop/src/stream/kms_atomic_fast.rs` *(new)* | Linux | ~340 | `//!` `#[cfg(target_os = "linux")]` Non-blocking atomic commit helper (`DRM_MODE_ATOMIC_NONBLOCK`, `IN_FENCE_FD`, `DRM_MODE_PAGE_FLIP_ASYNC`) + DMA-BUF inode/modifier `drmModeAddFB2WithModifiers` LRU cache. |
| `client/protocol/desktop/src/stream/v4l2_m2m.rs` *(new)* | Linux | ~410 | `//!` `#[cfg(target_os = "linux")]` Optional V4L2 M2M hardware decoder exporting `V4L2_MEMORY_DMABUF` into `dmabuf::Descriptor`; probes `/dev/video*` cleanly and returns `None` with zero cost on x86 Intel/AMD hosts. |
| `client/protocol/desktop/src/stream/linux_perf_tests.rs` *(new)* | Cross-plat | ~240 | `//!` Pure state-machine & LRU cache unit tests for Wayland subsurface eligibility, syncobj timeline monotonicity, KMS FB LRU eviction, and V4L2 capability probing. |
| `client/protocol/desktop/src/stream/win_directflip.rs` *(new)* | Windows | ~430 | `//!` `#[cfg(windows)]` `IDXGISwapChain3` waitable-object (`DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT`, `SetMaximumFrameLatency(1)`), DirectFlip/Independent Flip/MPO eligibility tracker & borderless fullscreen guard. |
| `client/protocol/desktop/src/stream/win_d3d12_decode.rs` *(new)* | Windows | ~490 | `//!` `#[cfg(windows)]` Zero-copy D3D12 Video Decode (`AV_PIX_FMT_D3D12` / explicit subresource barrier + fence handoff) preventing `#112` concurrent-bind device removal, with sticky automatic fallback to safe D3D11 copy path. |
| `client/protocol/desktop/src/stream/win_perf_tests.rs` *(new)* | Cross-plat | ~240 | `//!` Unit tests for DirectFlip/MPO state transitions, waitable-handle timeout recovery, `#112` sticky fallback latch, and decode-to-present wake coalescing. |
| `rust/windows/src/nvenc/d3d12_async.rs` *(new)* | Host NVENC | ~460 | `//!` Pre-registered D3D12 input/READBACK ring, GPU-GPU fence chaining (`inputFencePoint` -> `outputFencePoint`), bounded async overlap (`max_in_flight = 1..=2`), event-driven fence completion & sticky D3D11 demotion. |
| `rust/windows/src/nvenc/rate_control.rs` *(new)* | Host NVENC | ~340 | `//!` `NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY`, single-frame VBV + `lowDelayKeyFrameScale = 1`, quarter-res two-pass vs single-pass budget guard, zero-drain dynamic bitrate reconfigure, spatial AQ & SDK 13.0-gated temporal AQ. |
| `rust/windows/src/nvenc/recovery_tuning.rs` *(new)* | Host NVENC | ~310 | `//!` API 12.2+ `splitEncodeMode` vs `enableSubFrameWrite`/weighted-prediction invariants, LTR DPB slot tracking for `nvEncInvalidateRefFrames`, and intra-refresh coordination. |
| `rust/windows/src/nvenc/format_444_rtx.rs` *(new)* | Host NVENC | ~360 | `//!` Pooled/pre-registered 4:4:4 CUDA surfaces (`AYUV` / stacked `R16` -> `YUV444_10BIT`) and fence-synchronized zero-copy pool interop with `truehdr.rs` / `rtx_video.rs` (avoiding redundant session rebuilds when size/format match). |
| `rust/windows/src/nvenc/overhaul_tests.rs` *(new)* | Host NVENC | ~580 | `//!` Fake-driver & SDK 13.0 ABI tests covering all 4 new NVENC submodules across API 11.0, 12.0, 12.1, 12.2, and 13.0 without touching grandfathered `nvenc/tests.rs`. |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### 4.1 Zero-Copy Fast-Path Lifecycle (Unified Across macOS, Linux & Windows)

```text
  [Stream Start / Reconfigure]
        │
        ▼  Probe OS/GPU fast path (VT+AVSBDL / Wayland DMA-BUF+syncobj / KMS Atomic / Win DirectFlip+D3D12)
        ├──(refused / unsupported)──▶ [Standard Renderer Path] (log single plain-English reason; never crash)
        │
        ▼  (eligible & no active shaders/warp/overlays)
  [Zero-Copy Direct Plane Path]  (AVSampleBufferDisplayLayer / wl_subsurface / KMS plane / Win DirectFlip MPO)
        │   INVARIANT: decode queue depth <= 1, zero CPU copies, zero extra composition pass
        ├──(Control Center, Stats HUD, MetalFX/FSR/SGSR, or Frame Warp 2.0 activates)──▶ [Zero-Copy Single-Pass GPU Path]
        │      (CVMetalTextureCache->CAMetalLayer / EGL-Vulkan DMA-BUF / D3D11-D3D12 single pass)
        │      ──(overlays & shaders inactive again)──▶ atomic 1-frame handoff back to [Zero-Copy Direct Plane Path]
        ├──(device removed / DXGI_ERROR_DEVICE_REMOVED / VT invalidation / DMA-BUF modifier change)──▶
        │      [Sticky Safe Fallback for Session] (log visible WARN; flush & recover cleanly without crash loop)
        └──(stream end / reconnect)──▶ release surfaces, fences, syncobjs, FB cache & restore display state cleanly
```

### 4.2 Edge-Case & Reconnect Matrix

| Scenario | Required Verified Behavior |
| --- | --- |
| **Control Center / Stats HUD opens during Direct Plane Scanout (macOS / Linux / Windows)** | Switches atomically within `<= 1` frame from `AVSampleBufferDisplayLayer` / Wayland `wl_subsurface` / KMS direct plane / DirectFlip MPO to the zero-copy GPU composition pass, and cleanly returns to Direct Plane Scanout on close (`0` leaked surfaces/windows/fences). |
| **Windows `#112` GPU driver removal or D3D12 decode refusal** | `win_d3d12_decode.rs` latches a session-sticky fallback to safe D3D11 `CopySubresourceRegion` -> DirectFlip on the first refusal or `DXGI_ERROR_DEVICE_REMOVED`, logging exact `GetDeviceRemovedReason()` without ever crash-looping. |
| **Linux Wayland compositor rejects DMA-BUF modifier or lacks `wp_linux_drm_syncobj_manager_v1`** | `linux_dmabuf_direct.rs` negotiates via `zwp_linux_dmabuf_feedback_v1` scanout tranches; if no direct modifier matches or syncobj is absent, tears down the child `wl_subsurface` cleanly and runs on existing Mesa Vulkan/EGL rendering with zero protocol errors (`wl_display` never disconnects!). |
| **macOS window dragged across fixed-60 Hz external monitor <-> 120 Hz ProMotion internal panel** | `mac_display.rs` refreshes `DisplayTiming` on display change; `macos_metal_present.rs` updates `presentAtTime` / `displaySyncEnabled` cadence seamlessly without restarting `VTDecompressionSession`. |
| **NVENC D3D12 async ring hits fence timeout or driver returns `NV_ENC_ERR_DEVICE_NOT_EXIST`** | `nvenc/d3d12_async.rs` releases pre-registered slots safely via `drain`, logs loud `encoder_conversion` warning, and stickily demotes to the D3D11 graphics queue (never silently switches to AMF or software!). |
| **NVENC RTX HDR / RTX Video toggled mid-stream** | `nvenc/format_444_rtx.rs` swaps the pre-encode pool texture via fence handoff when dimensions/format match; if pixel format changes (e.g. SDR -> FP16 scRGB for RTX HDR), drains in-flight slots cleanly and rebuilds the session deterministically. |

### 4.3 Crash-Restore Guarantees

Item 21 mutates zero persistent host OS configuration beyond existing `nvenc` GPU profile and DRM/KMS connector restoration (`kms_scanout.rs` already restores connector properties on exit). On Windows, `win_directflip.rs` never alters OS display mode or registry settings; on Linux, `kms_atomic_fast.rs` removes every cached `drmModeRmFB` handle in `Drop` before relinquishing DRM master; on macOS, `macos_vt.m` calls `VTDecompressionSessionInvalidate` + `CFRelease` deterministically in `Drop`.

---

## 5. Settings & 10/10 Craft Plan

- **Fewer-settings rule:** **ZERO** new user-facing settings are added in Rubylight or Nightjar! Every fast path (`VTDecompressionSession`/`AVSampleBufferDisplayLayer`, Wayland DMA-BUF+syncobj, KMS atomic FB cache, Windows DirectFlip/Waitable/D3D12 decode, NVENC D3D12 pre-registered async ring) activates automatically when runtime capabilities support it, with environment escape hatches (`NIGHTJAR_MACOS_VT_DIRECT=0`, `NIGHTJAR_WAYLAND_DMABUF_DIRECT=0`, `NIGHTJAR_WIN_DIRECTFLIP=0`, `NIGHTJAR_WIN_D3D12_DECODE=0`) for field debugging — matching existing `NIGHTJAR_D3D11_PASS` / `NIGHTJAR_WAYLAND_DMABUF_REALLOC` conventions.
- **Fail visibly, never silently degrade:** Every fast-path active/fallback reason is logged in one concise structured line (`video path: ...`) and surfaced in the existing Stream Stats diagnostics line so users and Stream Doctor 2.0 can see exact active paths (`VT Direct`, `Metal Zero-Copy`, `Wayland DMA-BUF Syncobj`, `KMS Atomic`, `Win DirectFlip MPO`, `NVENC D3D12 Async`).
- **Zero `"Experimental"` & zero mod names:** Zero UI strings added; existing diagnostics copy uses plain English.
- **License & upstream credit hygiene (`AGENTS.md`):** Prefer clean independent implementations inspired by studying open-source peers (**Glimmer** GPL-3.0, **moonlight-qt / Nonary** GPL-3.0, **Foundation Sunshine** GPL-3.0, **mpv**). If any code or algorithm is directly adapted from an upstream project, record exact file-level attribution and license notice in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README credits (never in UI text or release notes).

---

## 6. Disjoint Implementer Slices (Phase 3 Staged `parallel()`)

Wave 1 scheduling note: Item 21 depends on `04` and `06` (so `render.rs` and `nvenc.rs` changes from `04`/`06` are already landed before Item 21 starts — **zero cross-item merge conflicts!**), and `07-hw-verify` depends on `["03", "04", "05", "06", "21"]`.

| Slice | Stage & Parallelism | Exclusive Owned Files (Zero Overlap) |
| --- | --- | --- |
| **21-A: macOS Direct VideoToolbox + Metal/AVSampleBufferDisplayLayer** | **Stage 1** (parallel with 21-B, 21-C, 21-D) | `client/protocol/desktop/src/stream/macos_vt.rs` (new), `client/protocol/desktop/src/stream/macos_vt.m` (new), `client/protocol/desktop/src/stream/macos_metal_present.rs` (new), `client/protocol/desktop/src/stream/macos_metal_present.m` (new), `client/protocol/desktop/src/stream/macos_vt_tests.rs` (new), `client/protocol/desktop/src/mac_display.rs`, `client/protocol/desktop/build.rs` `[verify post-sprint code-map.md]` |
| **21-B: Linux Wayland DMA-BUF Explicit Sync, KMS Atomic Fast Path & V4L2 M2M** | **Stage 1** (parallel with 21-A, 21-C, 21-D) | `client/protocol/desktop/src/stream/linux_dmabuf_direct.rs` (new), `client/protocol/desktop/src/stream/linux_syncobj.rs` (new), `client/protocol/desktop/src/stream/kms_atomic_fast.rs` (new), `client/protocol/desktop/src/stream/v4l2_m2m.rs` (new), `client/protocol/desktop/src/stream/linux_perf_tests.rs` (new), `client/protocol/desktop/src/linux_video.rs` `[verify post-sprint code-map.md]` |
| **21-C: Windows DirectFlip / Waitable Swapchain / MPO & D3D12 Video Zero-Copy** | **Stage 1** (parallel with 21-A, 21-B, 21-D) | `client/protocol/desktop/src/stream/win_directflip.rs` (new), `client/protocol/desktop/src/stream/win_d3d12_decode.rs` (new), `client/protocol/desktop/src/stream/win_perf_tests.rs` (new), `client/protocol/desktop/src/stream/direct.rs` `[verify post-sprint code-map.md]` |
| **21-D: Host NVIDIA NVENC Low-Latency Overhaul & SDK 13.0 Fake-Driver Suite** | **Stage 1** (parallel with 21-A, 21-B, 21-C) | `rust/core/src/nvenc.rs`, `rust/windows/src/nvenc/d3d12_async.rs` (new), `rust/windows/src/nvenc/rate_control.rs` (new), `rust/windows/src/nvenc/recovery_tuning.rs` (new), `rust/windows/src/nvenc/format_444_rtx.rs` (new), `rust/windows/src/nvenc/overhaul_tests.rs` (new), `rust/windows/src/nvenc/cuda.rs`, `rust/windows/src/nvenc.rs` *(net shrink <= 3350 LOC; `nvenc/tests.rs` untouched)*, `docs/nvenc.md` `[verify post-sprint code-map.md]` |
| **21-E: Desktop Pipeline/Presenter Integration & Attribution Sync** | **Stage 2** (after Stage 1 slices merge) | `client/protocol/desktop/src/stream/mod.rs` *(wire modules only; keep <= 2917)*, `client/protocol/desktop/src/stream/pipeline.rs` *(<= 2013)*, `client/protocol/desktop/src/stream/present.rs` *(<= 986)*, `client/protocol/desktop/src/stream/present/pacer.rs` *(<= 1060)*, `client/protocol/desktop/src/stream/render.rs` *(<= 1740)*, `client/protocol/desktop/src/stream/stage_times.rs`, `NOTICE`, `rust/THIRD_PARTY.md` `[verify post-sprint code-map.md]` |

---

## 7. Verification & Measurement Plan

### 7.1 Narrowest Unit, CI & Header/Sample Verification (No GPU Required)

1. **Cross-platform Desktop Rust unit tests (Windows + Linux + macOS CI):**
   - `cargo test -p nightjar-desktop --locked macos_vt linux_perf win_perf stream::present`
   - Verifies Annex-B->AVCC/HVCC/OBU framing, `<= 1` queue drop semantics, `AVSampleBufferDisplayLayer` <-> `CAMetalLayer` switch state machine, ProMotion `presentAtTime` cadence math, Wayland `wl_subsurface` + syncobj timeline invariants, KMS FB LRU eviction, Windows DirectFlip/MPO state machine, and `#112` sticky fallback.
2. **macOS CI Build Verification (`nightjar-macos.yml` — NOT run on local Apple Silicon hardware):**
   - Verified via GitHub Actions `nightjar-macos.yml` on `wip/roadmap/21-desktop-perf-and-nvenc`: compiles `macos_vt.m` + `macos_metal_present.m` with `-fobjc-arc -Wall -Werror`, links `VideoToolbox` + `CoreMedia` + `MetalFX`, and executes `cargo test -p nightjar-desktop --locked` on `aarch64-apple-darwin` & `x86_64-apple-darwin`.
   - **Honesty invariant:** Explicitly marked `"Verified in macOS CI only; not benchmarked on local Apple Silicon hardware"` in `rust/PERFORMANCE.md` and release notes.
3. **Host NVIDIA NVENC Header/Sample Audit & Fake-Driver Suite (`docs/nvenc.md#checking-nvidia-code` — NOT run on NVIDIA GPU hardware):**
   - `cargo test -p butterpollo-core --locked nvenc`
   - `cargo test -p butterpollo-windows --locked nvenc` (runs both existing `nvenc/tests.rs` and new `nvenc/overhaul_tests.rs` across API 11.0, 12.0, 12.1, 12.2, and 13.0!).
   - Author AND independent reviewer (`carbon-reviewer`) verify every foreign struct/field/flag/version macro against vendored `rust/windows/include/nvEncodeAPI.h` (SDK 13.0), NVIDIA Video Codec SDK samples, and `rtx-video-sdk-1.1.0` headers, recording untested hardware assumptions under `docs/nvenc.md#not-run-on-nvidia-hardware`.
4. **Architecture & Size-Ratchet Guardrails:**
   - `python tools/arch_check.py` (verifies `nvenc.rs <= 3711` [shrunk], `nvenc/tests.rs <= 3461`, all new files `<= 800` LOC with `//!` headers, zero unannotated `cfg(unix)` breaking macOS, zero `Family::Rubylight` gating).
   - `python tools/code_map.py check` & `cargo fmt -- --check` (without `--all`).

### 7.2 Serialized Real-Hardware Release Benchmarks (Executed Under `07-hw-verify`)

1. **Windows Desktop Client (RX 7900 XT Desktop & Radeon 780M Laptop — `wave1-desktop-perf-win`):**
   - Pre-flight idle gate (`127.0.0.1:47989/serverinfo` -> `SessionCount == 0 && PendingSessionCount == 0`, isolated test host `127.0.0.1:48123`, `GPU-LOCK` / `LAPTOP-LOCK`).
   - 90 s ABBA release-build runs (`A = 6.2.0 baseline path / NIGHTJAR_WIN_DIRECTFLIP=0`, `B = Item 21 DirectFlip Waitable + D3D12/D3D11 fast path`) at 1080p120 & 4K60 HEVC:
     - Measure `decode_us`, `decode_to_present_us` (p50/p99), frame-interval jitter (`timing_csv.rs`), and confirm `DirectFlip` / Independent Flip / Waitable Object active with `0` `DXGI_ERROR_DEVICE_REMOVED` events and Control Center open/close handoff `<= 1` frame.
2. **Intel Linux Desktop Client (`nuc` via `ssh nuc` — `wave1-desktop-perf-linux-nuc`):**
   - Streams from isolated idle-gated test host to the Intel NUC running release-build `nightjar` on Wayland and KMS:
     - Measure VA-API/Vulkan Video DMA-BUF zero-copy direct scanout hit rate (`>= 95%` fullscreen video-only frames), `0` CPU map fallbacks, `decode_to_present_us` p50/p99 improvement vs baseline (`NIGHTJAR_WAYLAND_DMABUF_DIRECT=0`), and clean Control Center open/close `wl_subsurface` handoff with zero `wl_display` protocol errors.
3. **Mandatory Exit Code 3 Abort Rule:** If ANY host run exits with code `3` (AMD encoder hang), **abort immediately and investigate; never retry**.
4. **Reporting:** Append measured Windows (RX 7900 XT, Radeon 780M) and Intel Linux (NUC) numbers to a dated section of `rust/PERFORMANCE.md`, explicitly stating: `"macOS Apple Silicon fast path verified via nightjar-macos.yml CI only; NVIDIA NVENC verified via SDK 13.0 headers and fake-driver tests only — neither was executed on local Apple Silicon or NVIDIA GPU hardware."`

---

## 8. Post-Discovery & 6.3.0 Rebase Addendum (`#139`, `#77`, `#141`, `#120`, `#116`, `#144`, Win ARM64 & macOS Updater)

- **Host Virtual-Display Refresh Cap & AMD AMF AV1 Skip-Frame Fix (Slice 21-D / `rust/drivers/rubylight-vdisplay-proto`):**
  1. Fix `rust/drivers/rubylight-vdisplay-proto/src/modes.rs:123-129` so ALL `default_modes()` fallback resolutions cap refresh rates to `<= fastest` (keeping `>= 60 Hz` if `fastest < 60_000`), preventing games switching to non-native resolutions from silently bumping a 60 Hz virtual display to 240 Hz (`#144`, `#131`).
  2. Set `Av1RateControlSkipFrameEnable = false` for AV1 in `rust/core/src/encoder_policy.rs:554-563` (capability-checked so older Adrenalin drivers stay unaffected) + add VCN5 (RDNA 4) high-refresh queue-drain telemetry around `amf/submit.rs:326,602`.
  3. Rebase NVENC & host pacing assumptions on `6.3.0` (`391229c60f`, `7945651f26`, `19c6742693`, `59dd5a0507`, `7f0e6e287d`), preserving strict header/sample-only NVENC honesty.
- **Linux Steam Deck Gamescope 4K120 VRR (`#139`), `kms_scanout.rs` 6.3.0 Rebase & aarch64 V4L2 `P010` HDR (`#77`) (Slice 21-B):**
  1. Explicitly gate `linux_dmabuf_direct.rs`'s child `wl_subsurface` **OFF** under `handheld::gamescope()` (Gamescope uses nested WSI `IMMEDIATE` swapchains, not arbitrary subsurfaces), add a dedicated Gamescope 4K120 HDR VRR swapchain/present-queue audit closing `#139`, and rebase `kms_scanout.rs` against `6.3.0` `6c2079fe3f` (ensuring net shrink below ratchet!).
  2. Build `v4l2_m2m.rs` zero-copy `V4L2_PIX_FMT_P010`/`P010M` DMA-BUF export + multi-frame Main10 probing cleanly atop `media.rs` (`a25c5f491b`) & `color_probe.rs` Qualcomm Iris fixes (`#77`), and promote `desktop-linux` `aarch64` in `client/.github/workflows/release.yml:83` from `continue-on-error: true` to blocking once green.
- **macOS Direct VT + Async MetalFX (`#141`), Host Phase-Lock (`#116`) & Broken Update-Check Fix (Slice 21-A):**
  1. Integrate `metalfx.rs` asynchronously inside `macos_metal_present.m`'s single `MTLCommandBuffer` (eliminating `metalfx.rs:91-99`'s ~5 ms synchronous stall), pool fallback SDL Metal textures in `present/upload.rs:35-36,91-94`, wire VideoToolbox 4:4:4 (`media.rs:1626`), and feed `CADisplayLink` / `CAMetalLayer` latch timing into `stream/phase.rs` so macOS participates in host phase-lock (`#116`).
  2. Fix `client/protocol/desktop/src/self_update.rs:186-190` (stop classifying standalone macOS `.app` bundles as `CopyKind::Managed`, which silently hid **Check for updates** in `ui/settings.rs:1866`) and fix `update_check.rs:150` User-Agent (`"macOS"` instead of `"Linux"`).
- **Windows ARM64 & Win10 Exclusive Fullscreen Scaled PyroWave Validation (`#120`) (Slice 21-C):**
  - Explicitly cover Windows ARM64 (`aarch64-pc-windows-gnullvm` / CLANGARM64 D3D12VA headers + Adreno fallback, verified via `windows-11-arm` CI only and never claimed locally benchmarked), and add Windows 10 Exclusive Fullscreen + `NIGHTJAR_DIRECT_PLANES=1` vs `=0` scaled PyroWave swapchain color-metadata verification to `win_perf_tests.rs` (`#120`).
