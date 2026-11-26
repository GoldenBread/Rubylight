---
name: check-nvenc
description: Audit NVIDIA NVENC, CUDA interop, NVAPI, and NGX/RTX Video code against vendor headers and fake-driver tests without claiming hardware verification.
---

# Checking NVIDIA / NVENC Code

Use this skill whenever writing or reviewing changes touching NVENC, CUDA interop, NVAPI driver settings/profiles, or NVIDIA NGX / RTX Video / RTX HDR.

## Hard rules (`AGENTS.md` & `docs/nvenc.md`)

- **NEVER claim NVIDIA code was run on hardware.** No NVIDIA GPU runs local or CI tests. Every PR, issue reply, and review summary MUST state explicitly what was checked via vendor headers + fake-driver tests and what remains unverified on real NVIDIA hardware.
- **Fail visibly — never silently switch encoders.** Refused sessions, unsupported 4:4:4/AV1 modes, or exhausted session limits (`NV_ENC_ERR_OUT_OF_MEMORY`, `NV_ENC_ERR_INCOMPATIBLE_CLIENT_KEY`) must report the exact reason rather than falling back to another encoder or silently downgrading 4:4:4 to 4:2:0.
- **Keep third-party credits accurate:** Ported Sunshine/LizardByte/Foundation logic is credited in code comments, `NOTICE`, `rust/THIRD_PARTY.md`, and README credits — never in UI text or release notes.

## Vendor header & code locations

| Surface | Vendor headers / reference | Host source |
| --- | --- | --- |
| NVENC / CUDA ABI | `rust/windows/abi/include/nvEncodeAPI.h` (FFmpeg `nv-codec-headers` n13.0.19.1, SDK 13.0) | `rust/core/media-policy/src/nvenc.rs`, `rust/core/media-policy/src/nvenc/`, `rust/windows/graphics/src/nvenc.rs`, `rust/windows/graphics/src/nvenc/` |
| RTX Video / TrueHDR (NGX) | `rtx-video-sdk-1.1.0` in dependencies folder after `rust\build.ps1 -Nvidia` | `rust/core/media-policy/src/nvidia_{enhance,component}.rs`, `rust/windows/graphics/src/{nvidia_enhance,rtx_video,truehdr}.rs`, `rust/truehdr-runtime/` |
| NVAPI (driver settings, clocks profile) | **No vendored headers in repo** — report explicitly which calls rest on other hosts' code alone | `rust/windows/graphics/src/{nvidia_settings,nvidia_profile}.rs` |

## Audit checklist (author AND reviewer)

1. **FFI & struct ABI check:** Verify every foreign declaration's parameter types, calling convention, struct field layout, bitfields, and `NV_ENC_*_VER` / API version constants directly against `rust/windows/abi/include/nvEncodeAPI.h` (give the reviewer the exact header path).
2. **Call-order & API version floors:**
   - API 11–13 supported; AV1 and D3D12 interface require API 12.0 (driver 522.25+); output block stats require API 12.1 (driver 531.61+); split-frame encode requires API 12.2.
   - Verify resource registration, mapping, fence synchronization (`inputFencePoint` / `outputFencePoint` on D3D12, `rust/windows/graphics/src/nvenc/submit.rs`), CUDA stream binding (`nvEncSetIOCudaStreams` after `nvEncInitializeEncoder`, `rust/windows/graphics/src/nvenc/config.rs`), and teardown order.
   - Verify D3D12 fallback restrictions (no weighted prediction, no `YUV444_10BIT`, subframe readback rules) and sticky demotion to D3D11 on repeated D3D12 failure.
3. **Fake-driver tests & documentation:**
   - Cover every new branch in `rust/windows/graphics/src/nvenc/tests.rs` or its `rust/windows/graphics/src/nvenc/tests/` submodules (or `rust/windows/graphics/src/nvidia_profile.rs`, `rust/windows/graphics/src/nvidia_settings/tests.rs`, `rust/windows/graphics/src/gpu_priority.rs`).
   - Update `docs/nvenc.md` (Parity table + "Not run on NVIDIA hardware" list) for any new unverified NVIDIA behaviour.

## Verification commands

```powershell
$env:TEMP="D:\t"; $env:TMP="D:\t"
python tools/dev.py cargo -- test -p butterpollo-media-policy --locked nvenc
python tools/dev.py cargo -- test -p butterpollo-win-graphics --locked nvenc
python tools/dev.py cargo -- test -p butterpollo-win-graphics --locked nvidia_
python tools/dev.py cargo -- clippy -p butterpollo-media-policy -p butterpollo-win-graphics --all-targets -- -D warnings
python tools/arch_check.py
```
