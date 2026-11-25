# Item 28: Multi-GPU & Hybrid Laptop Adapter Routing, `idd-push` LUID Coherence & Locked-PC Cold Launch (`#117`) (Wave 1)

Status: engineering design (Phase 1), **rebased 2026-11-25 onto post-refactoring `origin/main` (`955ef39598`, `v6.4.1`)** and [`00-protocol-reservations.md`](00-protocol-reservations.md).

> **Post-6.4.1 Verification Note:** Direct source verification on `origin/main` (`955ef39598`) confirmed every underlying bug targeted by Item 28 is **STILL LIVE** in the post-refactoring crates: `rust/core/media-policy/src/encoder_gpu.rs:164` (`one_vendor` short-circuit), `rust/windows/graphics/src/capture/idd_push.rs:112` (`status.render_luid` coherence), `rust/windows/display/src/display/virtual_display.rs` (TDR LUID re-resolution), `rust/windows/io/src/wake.rs:422` (`GetLastInputInfo` without `PowerRequestDisplayRequired` / `GUID_CONSOLE_DISPLAY_STATE`), and `rust/host/stream/src/stream/video/setup.rs:252` (`display_swap_chain() == Some(false)` skipping physical displays). All scope stays intact on the split crates!

## 1. Summary & User Experience

Item 28 eliminates two high-impact Windows host capture failures that hit everyday desktop/laptop players — especially Ryzen desktop users with an enabled 512 MB iGPU alongside a discrete Radeon/GeForce GPU, hybrid MUXless gaming laptops, and users launching streams on a locked or Modern-Standby PC (including after Away Wake Relay `15`):

1. **Multi-GPU & Hybrid Laptop Adapter Routing + `idd-push` LUID Coherence:**
   - **Discrete vs. Integrated GPU Selection Within One Vendor (`rust/core/media-policy/src/encoder_gpu.rs`):** Fixes `encoder_gpu::pinned()` returning `None` whenever `one_vendor` is true (e.g., Ryzen 7000/9000 desktop 512 MB iGPU + RX 7900 XT / RX 9070 XT, or AMD Advantage 780M + RX 7700S laptop), which previously forced headless `/serverinfo` probes and virtual-display rendering onto index 0 (often the 512 MB iGPU without AV1 encode). Uses dedicated VRAM + `DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE` ranking while **strictly honoring explicit `adapter_name` / `adapter_pnp_id` overrides** and **never silently switching encoders on failure**.
   - **Zero-Copy `idd-push` LUID Coherence (`rust/windows/graphics/src/capture/idd_push.rs`):** Fixes `idd_push::Session::new` (`idd_push.rs:783-802`) bypassing `stream_adapter()` and `status.render_luid`. Under Automatic adapter settings, `idd_push::Session::new` now opens its D3D11 device directly on `status.render_luid` (when non-zero and encoding-capable) or via `capture::stream_adapter(...)`, eliminating false LUID mismatches that previously demoted hybrid laptops out of zero-copy `idd-push` onto WGC!
   - **Dynamic TDR / MUX / eGPU LUID Re-Resolution (`rust/windows/display/src/display/virtual_display.rs`):** Re-resolves saved `adapter_pnp_id` -> live DXGI LUID inside `VirtualDisplay::maintain` (`virtual_display.rs:575`) on every driver restart/TDR/recreate instead of reapplying a stale frozen LUID. When an explicitly pinned eGPU/adapter is missing, surfaces a plain-English Console/Stream Doctor warning and recovery action instead of an opaque capture error.
   - **Stream Doctor 2.0 Visibility:** Exposes live `capture_luid`, `vdisplay_render_luid`, `encoder_luid`, and `cross_adapter_copy` state in host logs and Item `11` (`11-stream-doctor.md`).

2. **Locked-PC Cold Launch (`#117`), Display-Power Wake (`PowerRequestDisplayRequired`) & Lock-Screen Capture Continuity:**
   - **True Display-Power Wake (`rust/windows/io/src/wake.rs`):** Upgrades `wake.rs` (`477-498`) to acquire a scoped `PowerRequestDisplayRequired` alongside `PowerRequestSystemRequired` during stream wake/launch, and subscribes to `GUID_CONSOLE_DISPLAY_STATE` (`PowerRegisterSuspendResumeNotification` / `RegisterPowerSettingNotification`) so launch checks real display-power state (`Off` / `Dimmed` / `On`) instead of trusting `GetLastInputInfo()` on the `Default` desktop (which is blind to `Winlogon` when locked!).
   - **Unified Cold-Launch Recovery for Virtual AND Physical Displays (`rust/host/stream/src/stream/video/setup.rs`):** Fixes `setup.rs:247-255`, which previously gated first-frame wake strictly on `prepared.display_swap_chain() == Some(false)` (skipping physical displays in DPMS / Modern Standby sleep where `display_swap_chain() == None`) and fired only once without reactivating the CCD path or reopening capture. Now whenever display power is OFF or no GPU frame arrives within 1 s (on EITHER virtual or physical displays, including clients like Glimmer that do not send synthetic 1-px mouse jiggles), the host acquires `PowerRequestDisplayRequired`, re-triggers `activate_target()` (`virtual_display.rs:189`) on the active input desktop (`Winlogon` when locked), and reopens capture once display power transitions ON — resolving **Rubylight `#117`** cleanly without a 10 s timeout!
   - **Lock-Screen / UAC / Modern Standby Handoff:** Reuses 6.3.0's `DisplayRecoveryRetry` (`3b660899c5`/`fafdaf472a`) and sticky DDX recovery (`7f0e6e287d`) cleanly across `Away Wake -> Locked Winlogon -> Sign-In -> Fullscreen Game` transitions.

---

## 2. Protocol, Native API & Capability Contract

Per [`00-protocol-reservations.md`](00-protocol-reservations.md) §7, Item 28 requires **ZERO wire-protocol or Native API changes**:
- `PROTOCOL_VERSION` and `rubylight_native_api::API_VERSION = 1` stay unchanged.
- Preserves Windows 10 & 11, physical displays, WGC, Desktop Duplication, D3D11, AMD AMF, and NVIDIA NVENC (NVENC adapter-LUID paths verified strictly against vendored headers/samples & fake-driver tests, never claimed run on NVIDIA GPU hardware).
- **Fail visibly, never switch encoders silently:** Explicit user adapter overrides (`adapter_name` / `adapter_pnp_id`) remain authoritative and report clear errors if unavailable.

---

## 3. Architecture & <=800-LOC Module Plan

All paths carry `docs/code-map.md` for Phase 2 reconciliation. Every new `.rs` module stays strictly `<= 800` LOC with `//!` headers; grandfathered files (`capture.rs` 2156, `capture/idd_push.rs` 1492, `wake.rs` 1121, `display_session.rs` 3214, `video/run.rs` 2003) stay at or below their `tools/size_ratchet.json` ceilings.

| Path | Status | Est. LOC | Responsibility |
| --- | --- | ---: | --- |
| `rust/core/media-policy/src/encoder_gpu.rs` | Modify | `<= 650` | Discrete-over-integrated ranking within single-vendor & MUXless hybrid configurations (`DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE` + VRAM), preserving explicit overrides + exhaustive multi-GPU matrix tests. |
| `rust/windows/graphics/src/capture/luid_resolve.rs` | New | ~240 | `//!` Canonical live `adapter_pnp_id` <-> DXGI LUID resolution, `idd-push` `status.render_luid` coherence check, and eGPU/MUX missing-adapter diagnostic formatting. |
| `rust/windows/graphics/src/capture/idd_push.rs` | Modify | `<= 1492` | Delegate device opening to `luid_resolve::open_for_vdisplay(...)` so `idd-push` never falsely mismatches `status.render_luid`. |
| `rust/windows/display/src/display/virtual_display.rs` | Modify | `<= 780` | Re-resolve `adapter_pnp_id` -> live LUID inside `VirtualDisplay::maintain` on every driver restart/TDR/recreate. |
| `rust/windows/io/src/display_power.rs` | New | ~280 | `//!` Scoped `PowerRequestDisplayRequired` + `PowerRequestSystemRequired` guard and `GUID_CONSOLE_DISPLAY_STATE` (`Off`/`Dimmed`/`On`) subscription (Win10 & Win11 safe). |
| `rust/windows/io/src/wake.rs` | Modify | `<= 790` | Delegate display-power state & wake request to `display_power.rs` (eliminating `Default`-desktop-only `GetLastInputInfo()` blindness on `Winlogon`). |
| `rust/host/stream/src/stream/video/cold_launch_wake.rs` | New | ~240 | `//!` Bounded 1 s first-frame recovery state machine for BOTH virtual (`swap_chain == false`) and physical displays: acquires `DisplayRequired`, re-triggers `activate_target()`, and reopens capture on `DisplayState::On` (`#117`). |
| `rust/host/stream/src/stream/video/setup.rs` | Modify | `<= 760` | Wire `cold_launch_wake.rs` into first-frame wait without adding any steady-state latency once frame 1 arrives. |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

```text
[Stream Launch -> Capture Setup]
   ├──(frame arrives < 1 s)──▶ [Streaming Steady State] (0 overhead; release wake boost on clean stream)
   └──(display OFF or no frame at 1 s: virtual OR physical display)
          ▼
     [ColdLaunchWake (#117)]
       1. Acquire scoped PowerRequestDisplayRequired + SystemRequired
       2. If virtual display & swap_chain==false -> activate_target() on active desktop (Winlogon-safe)
       3. Wait up to 4 s for GUID_CONSOLE_DISPLAY_STATE::On -> reopen capture cleanly
       ├──(frame arrives)──▶ [Streaming Steady State] (log recovery timing)
       └──(still blocked after bounded retries)──▶ Fail visibly with plain-English display-power/adapter cause
```

- **Crash-Restore Safety:** `PowerRequestDisplayRequired` is held strictly by an RAII handle released in `Drop` (and automatically cleared by the Windows kernel if `butterpollo.exe` exits or crashes — zero permanent sleep-prevention leak!).

---

## 5. Settings & 10/10 Craft Plan

- **Zero new user-facing settings** (multi-GPU routing stays under existing Automatic / explicit GPU selector; `#117` wake recovery is 100% automatic).
- **Zero default-path latency:** Steady-state streaming adds 0 GPU passes and 0 per-frame work once frame 1 arrives.
- **Plain-English diagnostics:** Missing pinned eGPU/adapter and locked-display wake refusals emit crisp plain-English copy into Console & Stream Doctor 2.0 (zero `"Experimental"`, zero mod names).

---

## 6. Disjoint Implementer Slices (Phase 3 `parallel()`)

| Slice | Stage | Exclusive Owned Files (Zero Overlap) |
| --- | --- | --- |
| **28-A: Pure Core Multi-GPU Ranking (`encoder_gpu.rs`)** | **Stage 1** | `rust/core/media-policy/src/encoder_gpu.rs` |
| **28-B: Windows `idd-push` LUID Coherence & VDD Re-Resolution** | **Stage 2** (parallel with 28-C) | `rust/windows/graphics/src/capture/luid_resolve.rs` (new), `rust/windows/graphics/src/capture/idd_push.rs`, `rust/windows/display/src/display/virtual_display.rs` |
| **28-C: Windows Display-Power Wake & `#117` Cold-Launch Recovery** | **Stage 2** (parallel with 28-B) | `rust/windows/io/src/display_power.rs` (new), `rust/windows/io/src/wake.rs`, `rust/host/stream/src/stream/video/cold_launch_wake.rs` (new), `rust/host/stream/src/stream/video/setup.rs` |

---

## 7. Verification & Measurement Plan

1. **Deterministic Unit Tests (No GPU Required):**
   - `cargo test -p butterpollo-core --locked encoder_gpu` — tests Ryzen 512 MB iGPU + RX 7900 XT (single-vendor), AMD 780M + RX 7700S (AMD Advantage), AMD iGPU + NVIDIA dGPU (MUXless), explicit `adapter_name`/`adapter_pnp_id` override precedence, and missing eGPU fallback reporting.
   - `cargo test -p butterpollo-windows --locked luid_resolve display_power` & `cargo test -p butterpollo --locked cold_launch_wake` — tests LUID re-resolution after mock TDR/re-enum, `DisplayState::Off -> On` wake sequencing, RAII power-request release on drop/abort, and zero-overhead skip when frame 1 arrives promptly.
2. **Real Hardware Verification (Executed Under `07-hw-verify`, Serialized Idle-Gated):**
   - **RX 7900 XT desktop + Radeon 780M laptop:** Verify cold launch from locked/display-off state (`#117`) succeeds in `<= 3 s` without synthetic client mouse jiggle on both virtual and physical displays; verify zero-copy `idd-push` stays active on virtual display with matching `capture_luid == vdisplay_render_luid == encoder_luid`; verify `present_to_sent p99` delta `<= +0.05 ms` vs baseline and zero exit-code-3 hangs.
   - **NVIDIA honesty:** Multi-GPU NVIDIA dGPU selection & NVENC LUID wiring verified against DXGI/NVENC headers and unit tests only; never claimed executed on NVIDIA GPU hardware.
