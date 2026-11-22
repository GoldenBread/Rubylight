# Item 06: Real-Time HDR-to-SDR Tone Mapping (`hdr-sdr-tonemap`)

Status: design, not implemented (Wave 1, Rubylight & Nightjar 7.0.0). Written against `main` `b3d0c26b9b` (`6.2.0`) and [protocol reservations](00-protocol-reservations.md).

## 1. Summary & User Experience

Many PC gamers keep **Windows HDR enabled** on their desktop monitor (or use an HDR virtual display) while frequently streaming to SDR-only clients (older laptops, Steam Deck LCD, SDR handhelds, browser/WebRTC, or stock Moonlight devices with HDR turned off). Today in `rust/windows/src/shaders/color.hlsl`:

1. When the host desktop is **scRGB FP16 HDR** (`pixel == 1`) and the client negotiates an **SDR stream** (`hdr == 0`), `color.hlsl:89-92` divides linear light by `sdrWhiteScale` and performs a hard **`gamma(saturate(rgb))`** clip. Any HDR specular highlight above SDR paper white (e.g., 250–1000+ nits in games, sun shafts, explosions, bright HUD glows) slams hard to `1.0` and clips flat white with severe hue shifts on saturated Rec.2020/P3 colors.
2. When the host display delivers **HDR10 PQ** (`pixel == 2`) and the client negotiates **SDR** (`hdr == 0`), both `rust/windows/src/compute.rs:789` (`validate_colour_input`) and `rust/windows/src/gpu_color.rs:240` refuse the frame outright (`"HDR surface supplied to an SDR converter"`).

Real-time GPU HDR-to-SDR tone mapping fixes both cases directly **inside the existing `color.hlsl` conversion shader** — with **zero extra GPU render/compute pass**, working identically across **D3D12 compute (`compute.rs`)**, **D3D11 graphics/compute (`gpu_color.rs`)**, **AMD AMF**, **NVIDIA NVENC**, **Windows 10 & 11**, **WGC**, **Desktop Duplication**, and both **physical and virtual HDR displays**.

### User experience

- **Automatic, filmic highlights with crisp desktop UI:** When streaming an HDR host display to an SDR client with tone mapping active (`bt2390` or `auto`):
  - Bright HDR highlights smoothly roll off using display-referred **ITU-R BT.2390 EETF** anchored to the live source peak (`compute_hdr_stats.rs` smoothed peak or display `maximum_nits`) and Windows' SDR white level (`sdrWhiteScale * 80` nits).
  - Wide-gamut Rec.2020 / Display-P3 colors compress smoothly into Rec.709 rather than hard-clipping individual RGB channels (eliminating yellow-shifted reds and cyan-shifted blues).
  - Pure SDR windows/text on an HDR desktop (where live frame peak `<= sdr_white_nits`) hit the linear identity region of the EETF and render at **exact 1:1 SDR white** without looking dim or washed out.
- **Stock Moonlight & Nightjar parity:**
  - Stock Moonlight clients (which send no `rlSdrToneMap` launch option) automatically inherit the host's setting — zero wire changes required.
  - Nightjar clients read `HdrToneMapVersion=1` / `video.sdr_tonemap`, send `rlSdrToneMap=auto|bt2390|off` at launch, and can live-toggle **HDR-to-SDR Tone Mapping** (`Host setting` / `Auto` / `BT.2390` / `Off`) inside the Stream Menu via `/api/native/v1/stream/settings` (`id: "hdr_sdr_tonemap"`) **without an encoder restart or IDR keyframe**.

---

## 2. Protocol, Native API & Capability Contract

All identifiers strictly follow [00-protocol-reservations.md](00-protocol-reservations.md). `PROTOCOL_VERSION` and `rubylight_native_api::API_VERSION` (`1`) remain **unchanged**. No new ENet control message ID is allocated.

### 2.1 Discovery, RTSP, Launch & Capability Gating

| Surface | Element / Attribute / Constant | Value |
|---|---|---|
| `/serverinfo` | `HdrToneMapVersion` | `1` (emitted in `Offered::serverinfo_fields`) |
| RTSP `DESCRIBE` | `a=x-rl-video.sdrToneMap:1` | Emitted in `Offered::describe_lines` |
| Launch `/launch`, `/resume` | `rlSdrToneMap` | `auto` \| `bt2390` \| `off` (optional; absent = host/device/app config; unknown value logs WARN and falls back loudly to `off` with session warning `sdr_tonemap`) |
| Native API `/capabilities` | `feature::STREAM_SETTINGS` = `"stream.settings"`, `feature::HDR_SDR_TONEMAP` = `"video.sdr_tonemap"` | **New** feature constants in `rust/core/native-api/src/lib.rs`, advertised in `/capabilities` for every paired caller |
| Native API routes (new) | **New** `GET /api/native/v1/stream/settings` & `POST /api/native/v1/stream/settings` (`launch` permission scope) | Created in this item (`rust/core/native-api/src/stream.rs`, `rust/host/src/native_api/stream_settings.rs`, registered in `rust/host/src/native_api.rs`). Carries `id: "hdr_sdr_tonemap"`, `group: "video"`, `kind: "choice"`, options `inherit` / `auto` / `bt2390` / `off`, plus read-only summary field on `GET /api/native/v1/session` (`hdr_sdr_tonemap_active: Option<bool>`) |
| Nightjar Rust | `HostCapabilities::Feature::HdrSdrToneMap` | Proven by `HdrToneMapVersion >= 1` **or** native `video.sdr_tonemap` |
| Nightjar Java | `HostCapabilities.Feature.HDR_SDR_TONEMAP` | Proven by `HdrToneMapVersion >= 1` **or** native `video.sdr_tonemap` |

### 2.2 Creating Additive `GET` / `POST /api/native/v1/stream/settings`

Neither `/api/native/v1/stream/settings` nor `rust/host/src/native_api/stream_settings.rs` exists in `6.2.0` `main` (`docs/encoder-focus-design.md` §8 is a design specification, not yet implemented). Item 06 creates these additive v1 endpoints and shared DTOs (keeping `API_VERSION = 1` unchanged) so HDR-to-SDR tone mapping—and future stream-scoped settings such as encoder focus—share one canonical contract:

- **Wire DTOs (`rust/core/native-api/src/stream.rs`, new; exported in `rust/core/native-api/src/lib.rs` alongside `feature::STREAM_SETTINGS = "stream.settings"` and `feature::HDR_SDR_TONEMAP = "video.sdr_tonemap"`):**
  - `StreamSettings { revision: u64, session: Option<SessionFacts>, settings: Vec<StreamSetting> }` (`#[serde(default)]` on response structs).
  - `SessionFacts { streaming: bool, owner: bool, codec: Option<String>, ten_bit: bool, pyrowave: bool, hdr: bool }`.
  - `StreamSetting { #[serde(flatten)] base: radeon::Setting, source: SettingSource, effective: Option<String> }` (`SettingSource`: `Host`, `Device`, `App`, `#[serde(other)] Unknown`; `effective` omitted without an active session or when caller is not the session owner).
  - `StreamSettingsWrite { changes: Vec<radeon::Change> }` with `#[serde(deny_unknown_fields)]`.
- **Host route handler (`rust/host/src/native_api/stream_settings.rs`, new) & router registration (`rust/host/src/native_api.rs`):**
  - Registers `mod stream_settings;`, `GET` and `POST` on `/api/native/v1/stream/settings` (with `.fallback(unknown)`) in `native_api::router()`, and advertises both `feature::STREAM_SETTINGS` and `feature::HDR_SDR_TONEMAP` in `capabilities`.
  - Exposes `pub(crate) fn read(state: &Shared, device: &str) -> StreamSettings` and `pub(crate) fn write(state: &Shared, device: &str, changes: &[radeon::Change]) -> Result<StreamSettings, Failure>` (the couch-menu hook specified in `docs/encoder-focus-design.md` §8).
  - **Authentication & permission scope:** Both `GET` and `POST` authenticate the paired client TLS certificate and require live **`launch`** permission from `h.paired`.
  - **Errors (matching `rust/core/native-api/src/error.rs`):** `not_paired` (`403`), `permission_required` (`403`, missing `launch`), `invalid_value` (`422`, unknown id or value), `not_writable` (`422`, state not `configurable`, carrying `setting` and reason).

`GET /api/native/v1/stream/settings` response example:

```json
{
  "revision": 4,
  "session": { "streaming": true, "owner": true, "codec": "hevc", "ten_bit": false, "pyrowave": false, "hdr": false },
  "settings": [
    {
      "id": "hdr_sdr_tonemap",
      "group": "video",
      "kind": "choice",
      "state": "configurable",
      "value": "auto",
      "source": "host",
      "effective": "bt2390",
      "options": [
        { "value": "inherit", "label": "Host setting" },
        { "value": "auto", "label": "Automatic (when host display is HDR)" },
        { "value": "bt2390", "label": "BT.2390 tone mapping" },
        { "value": "off", "label": "Off (clip highlights)" }
      ],
      "reason": null,
      "message": null
    }
  ]
}
```

- `state` values:
  - `configurable`: SDR stream or no active session (writes the caller's own device override in the store behind `/api/clients/update`; `"inherit"` removes it; applies on the very next frame if caller owns an active SDR stream).
  - `read_only`: reason `app_override` (app pins `sdr_tonemap`), `stream_is_hdr` (current stream negotiated `hdr != 0`, so HDR-to-SDR tone mapping is inactive; message: `"This stream is already HDR, so HDR-to-SDR tone mapping is not needed."`), or `permission`.
- **Live apply:** `POST /api/native/v1/stream/settings` with `{"changes":[{"id":"hdr_sdr_tonemap","value":"bt2390"}]}` persists the device override and—when the caller owns the active SDR stream—updates `Session::sdr_tonemap: AtomicU8` immediately; the session loop picks it up on the next frame and switches `Converter`'s cbuffer/PSO with **zero encoder rebuild and zero IDR**, returning the updated `StreamSettings` JSON.

---

## 3. Architecture & <=800-LOC Module Plan

### 3.1 Exact Math: Display-Referred BT.2390 EETF + Smooth Gamut Compression

All math is defined once in pure OS-free Rust (`rust/core/src/sdr_tonemap.rs`) and implemented identically in `rust/windows/src/shaders/color.hlsl` and CPU reference tests in `rust/windows/src/color.rs`.

#### Step 1: Normalize HDR input to linear Rec.2020 luminance (nits) **before** resizing

- Today (`color.hlsl:89-92`), `load()` converts `pixel == 1` to gamma-encoded sRGB and clips it *before* `resize()`/`bilinear()`. That both loses highlight energy prior to downscaling and would execute tone mapping `4x..81x` per output pixel on 4K->1080p/720p downscales.
- Under tone mapping (compile-time `bool tonemap = true` folded by `fxc`), `load()` keeps linear light:
  - `pixel == 1` (scRGB FP16, Rec.709 primaries where `1.0 = 80` nits): pointer composites in linear scRGB (existing line 78), then `load()` returns linear scRGB untouched.
  - `pixel == 2` (HDR10 PQ, Rec.2020 primaries where `1.0 = 10000` nits): pointer composites in linear Rec.2020 (`depq(rgb)`), and `load()` returns `depq(rgb) * 125.0` (linear Rec.2020 in 80-nit scRGB units).
- `resize()` / `bilinear()` runs on linear light once, and `nonlinear()` applies tone mapping **exactly once per output pixel**:
  - Convert to linear Rec.2020 nits:
    - If `pixel == 1`: `light2020 = gamut2020(rgb) * 80.0`
    - If `pixel == 2`: `light2020 = rgb * 80.0` (already Rec.2020)
  - Clamp negative scRGB sub-black noise: `light2020 = max(light2020, 0.0)`.

#### Step 2: Luminance-preserving ITU-R BT.2390-11 EETF

Let:
- `L_src = max(toneMapPeakNits, toneMapTargetNits)` (source mastering/live peak in nits, clamped `[100.0, 10000.0]`).
- `L_dst = clamp(toneMapTargetNits, 80.0, 480.0)` (target SDR white anchor = `sdrWhiteScale * 80.0` nits from Windows' SDR content brightness slider).
- Compute Rec.2020 pixel luminance: `Y_in = dot(light2020, float3(0.2627, 0.6780, 0.0593))`.
- **Fast identity bypass for SDR-range scenes:** When `L_src <= L_dst * 1.02` (e.g., pure SDR desktop or SDR app running on an HDR monitor where live peak stays at `sdrWhiteScale * 80`), `Y_out = min(Y_in, L_dst)` — **bit-exact 1:1 SDR white preservation with zero dimming**.
- Otherwise, evaluate BT.2390 EETF in PQ space (`pq_nits(n) = pq(n / 10000.0)`):
  - `E_max = pq_nits(L_src)` (precomputed on CPU or `pq(L_src / 10000.0)` once per pixel; even better, pass `maxLum` and `KS` directly or compute from `toneMapPeakNits`/`toneMapTargetNits`).
  - `E_1 = clamp(pq_nits(Y_in) / E_max, 0.0, 1.0)`
  - `maxLum = clamp(pq_nits(L_dst) / E_max, 0.0, 1.0)`
  - `KS = max(1.5 * maxLum - 0.5, 0.0)`
  - If `E_1 < KS`: `E_2 = E_1` (linear pass-through for shadows, midtones, and diffuse SDR content!)
  - Else (`KS <= E_1 <= 1.0`): Hermite spline:
    - `t = (E_1 - KS) / max(1.0 - KS, 1e-6)`
    - `t2 = t * t; t3 = t2 * t;`
    - `E_2 = (2*t3 - 3*t2 + 1)*KS + (t3 - 2*t2 + t)*(1.0 - KS) + (-2*t3 + 3*t2)*maxLum`
  - Recover target luminance: `Y_out = depq_scalar(E_2 * E_max) * 10000.0`.
  - Scale RGB preserving chromaticity ratios: `scale = Y_in > 1e-6 ? (Y_out / Y_in) : 0.0`, `mapped2020 = (light2020 * scale) / L_dst` (now normalized so `1.0` = SDR peak white!).

#### Step 3: Smooth Rec.2020 -> Rec.709 Gamut Compression

Hard-clipping `saturate(rec2020_to_709(mapped2020))` breaks hue on vivid Rec.2020 reds/greens/cyans (one channel goes negative while another exceeds `1.0`). Instead:

1. Convert `mapped2020` to Rec.709 linear via the exact inverse matrix already in `color.hlsl:55-57`:
   `rgb709 = rec2020_to_709(mapped2020)`.
2. Compute achromatic anchor `Y709 = clamp(dot(rgb709, float3(0.2126, 0.7152, 0.0722)), 0.0, 1.0)` (or `achromatic = max(max(rgb709.r, rgb709.g), rgb709.b)` for ACES-style distance compression).
3. Apply smooth **soft-knee desaturation toward `Y709`** for out-of-gamut channels (`min_c = min(rgb709.r, min(rgb709.g, rgb709.b))`, `max_c = max(rgb709.r, max(rgb709.g, rgb709.b))`):
   - **Negative-channel compression (chroma outside Rec.709 triangle):** When `min_c < 0.0` and `Y709 > 1e-6`, compute distance `d = -min_c / (Y709 - min_c)` and smoothly compress chroma via hyperbolic knee (`k = 0.85`, `d_out = compress_knee(d, 0.85, 1.0)`) so `min_c` approaches `0.0` smoothly without hard discontinuity along the Rec.709 gamut boundary.
   - **Upper-headroom highlight desaturation (`max_c > 1.0`):** When a saturated highlight exceeds `1.0` in one Rec.709 primary while `Y709 <= 1.0`, blend smoothly toward `Y709` (`alpha = (max_c - 1.0) / max(max_c - Y709, 1e-6)`), preserving luminance `Y709` and preventing channel clipping posterization.
4. Encode final SDR transfer:
   - If `colorMatrix == 2` (10-bit BT.2020 SDR stream): skip Rec.709 matrix and apply `bt2020_sdr(saturate(mapped2020))` directly!
   - Otherwise (standard Rec.709 / Rec.601 SDR): return `gamma(saturate(rgb709))`.

### 3.2 Zero-Overhead Constant Buffer & Shader Entry Point Strategy

Look closely at `cbuffer Config : register(b0)` in `rust/windows/src/shaders/color.hlsl:4-10` (80 bytes = `[u32; 20]` shared identically by `compute.rs` and `gpu_color.rs`):

```hlsl
cbuffer Config : register(b0) {
    uint2 sourceSize; uint2 targetSize;
    uint pixel; uint hdr; uint colorMatrix; uint fullRange;
    uint tenBit; float sdrWhiteScale; float hdrScale; uint padding;
    int2 pointerPosition; uint2 pointerSize;
    uint pointerMode;
    uint toneMapMode;        // was pointerPadding.x (slot 17): 0 = off, 1 = bt2390
    float toneMapPeakNits;   // was pointerPadding.y (slot 18): smoothed live/display peak nits
    float toneMapTargetNits; // was pointerPadding.z (slot 19): sdrWhiteScale * 80.0
};
```

- **Zero buffer resize, zero root-signature change, zero extra binding!** Slots 17–19 were previously unused zeroes (`gpu_color.rs:217-219`).
- **Byte-for-byte preservation of the default SDR-on-SDR and HDR-on-HDR shaders:**
  - Just as `encoder-focus-design.md` kept `yuv420_cs` untouched via `yuv420_focus_cs`, we guard the tone-mapping path behind a compile-time `bool tonemap` literal parameter (folded away by `fxc` exactly like `bool menu` at `color.hlsl:204`!) and register dedicated `*_tonemap` entry points in `rust/windows/build_shaders.rs`:
    - Compute (`cs_5_0`): `yuv420_tonemap_cs`, `yuv420_focus_tonemap_cs`, `chroma_pack_tonemap_cs`, `pyro_tonemap_cs`.
    - D3D11 pixel shaders (`ps_5_0`): `luma_tonemap`, `chroma_tonemap`, `chroma_pack_y_tonemap`, `chroma_pack_uv_tonemap`, `pyro_y_tonemap`, `pyro_u_tonemap`, `pyro_v_tonemap`, `pyro_444_tonemap`, `yuv444_ayuv_tonemap`, `yuv444_planes_tonemap`.
  - When `toneMapMode == 0` (every SDR-on-SDR stream, every HDR-on-HDR stream, and `rlSdrToneMap=off`), the converters dispatch the **existing unmodified PSOs** — `yuv420_cs` DXBC hash is **100% byte-identical**, guaranteeing `+0.00 ms` on the default path!
  - When `hdr == 0 && (pixel == 1 || pixel == 2) && toneMapMode == 1`, `compute.rs` and `gpu_color.rs` bind the `*_tonemap` PSO on the **exact same single conversion pass** (lazy-created on first HDR-to-SDR frame so SDR-only sessions don't even allocate the extra PSO).
- **Important `hdr_stats_cs` fix (`color.hlsl:680-681`):**
  - Currently `hdr_stats_cs` calls `load(...)`, which under `hdr == 0 && pixel == 1` clamps to SDR sRGB! We extract a tiny `load_hdr_raw(int2 p)` helper (reading raw `source` + pointer without SDR clamping) so `hdr_stats_cs` accurately measures the true HDR peak luminance (`max_pq` / 99.9th percentile) even when `hdr == 0`!
  - On D3D12 (`compute.rs`), when HDR-to-SDR tone mapping is active, `enable_hdr_stats()` runs asynchronously after `convert()` (zero CPU wait via `latest_hdr_stats()`). A pure-core `PeakTracker` (`rust/core/src/sdr_tonemap.rs`) smooths the 99.9th-percentile peak with fast attack (`tau_up = 0.25 s`, avoiding clipped flashes) and slow release (`tau_down = 2.0 s`, preventing exposure pumping), falling back cleanly to `display_metadata.maximum_nits` (from `IDXGIOutput6::GetDesc1` / `butterpollo_core::hdr::Metadata`) on D3D11 (`gpu_color.rs`) or before the first stats pass completes.
- **AMF VPE & NVENC interplay:**
  - `butterpollo_core::amf_vpe::wanted_for` returns `false` whenever HDR-to-SDR tone mapping is active (`hdr == 0 && source_is_hdr && tonemap_active`), routing through our deterministic `color.hlsl` shader instead of AMF's opaque VPE converter.
  - Works identically for **NVIDIA NVENC** on both D3D12 compute (`compute.rs`) and D3D11 graphics (`gpu_color.rs`, including native 4:4:4 AYUV/Planar16!). All NVENC changes are strictly verified against vendored headers/samples (`docs/nvenc.md#checking-nvidia-code`) and `nvenc/tests.rs` fake-driver tests — never claimed as run on NVIDIA hardware.
  - **PQ (`pixel == 2`) SDR validation:** Update `validate_colour_input` (`compute.rs:789`) and `gpu_color.rs:240` to allow `pixel == 2 && hdr == 0` whenever tone mapping is enabled (or `pixel == 2` linear SDR conversion is active), while retaining the guard if an invalid unmapped surface somehow bypasses configuration.

### 3.3 `<=800-LOC` Module Plan & `size_ratchet.json` Compliance

#### Grandfathered files touched (from `tools/size_ratchet.json`)

| Grandfathered file | Ratchet ceiling | How we avoid growing it |
|---|---|---|
| `rust/windows/src/compute.rs` `[verify post-sprint code-map.md]` | `2079` | Delegate tonemap PSO creation/selection & `PeakTracker` hook to new `rust/windows/src/compute_tonemap.rs` (net <= +8 lines in `compute.rs`; extract `validate_colour_input` into `compute_tonemap.rs` so `compute.rs` stays net-neutral!) |
| `rust/windows/src/gpu_color.rs` `[verify post-sprint code-map.md]` | `812` | Only 12 lines over 800! Move tonemap PS bundle & `tonemap_constants()` into new `rust/windows/src/gpu_color/tonemap.rs` and move `constants()` helper there too so `gpu_color.rs` **shrinks** below 800 LOC! |
| `rust/windows/src/encoder.rs` `[verify post-sprint code-map.md]` | `985` | Add 4-line `set_sdr_tonemap(&mut self, ...)` forwarding to AMF/NVENC/PyroWave |
| `rust/windows/src/nvenc.rs` `[verify post-sprint code-map.md]` | `3703` | Forward `set_sdr_tonemap` to `self.compute` / `self.color` converter (<=6 lines) |
| `rust/host/src/stream/video/run.rs` `[verify post-sprint code-map.md]` | `1956` | Single 4-line call `tonemap.apply(&s, &image, active)` per frame (all policy/state lives in new `rust/host/src/stream/tonemap.rs`) |
| `rust/host/src/stream.rs` `[verify post-sprint code-map.md]` | `1324` | Wire initial `sdr_tonemap` launch/config resolution (<=6 lines) |
| `rust/core/src/config.rs` `[verify post-sprint code-map.md]` | `1373` | Add 4-line `pub fn sdr_tonemap(&self)` delegating to `crate::sdr_tonemap::parse` |
| `rust/core/src/session.rs` `[verify post-sprint code-map.md]` | `1803` | Add `pub sdr_tonemap: AtomicU8` + `sdr_tonemap_active: AtomicBool` (<=4 lines) |
| `rust/host/src/native_api.rs` `[verify post-sprint code-map.md]` | `853` | Add `mod stream_settings;`, register **new** `GET` + `POST /stream/settings` in `router()`, and add `feature::STREAM_SETTINGS` + `feature::HDR_SDR_TONEMAP` to `capabilities` (keep `<= 853` LOC by delegating all logic to new `native_api/stream_settings.rs` and compacting if needed) |
| `rust/web/src/lib/schema/video.ts` `[verify post-sprint code-map.md]` | `1461` | Add 1 select definition (`sdr_tonemap`) via compact helper or extract shared options |
| `client/protocol/client/src/http/capabilities.rs` `[verify post-sprint code-map.md]` | `1257` | Add `HdrSdrToneMap` variant (<=4 lines) |
| `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java` `[verify post-sprint code-map.md]` | `1347` | Add `HDR_SDR_TONEMAP` variant (<=4 lines) |
| `client/app/src/main/java/com/limelight/preferences/HostStreamSettings.java` `[verify post-sprint code-map.md]` | `1723` | Delegate the new `/api/native/v1/stream/settings` choice row to new `NativeStreamSettingsSection.java` (`<= 200` LOC, `/**` header) so `HostStreamSettings.java` stays `<= 1723` LOC |
| `client/protocol/desktop/src/strings/*.rs` `[verify post-sprint code-map.md]` | `1750` | **Do NOT edit `strings/*.rs` (0 headroom).** Route all new HDR-to-SDR tone-map Desktop translations to the canonical 7.0.0 per-locale overflow files `client/protocol/desktop/src/strings_70/<code>.rs` (established in Design 02, chained in `strings.rs` and verified by `strings/tests.rs`) |

#### New `<=800-LOC` modules to create

| Planned Path | Est. LOC | Ownership (`//!` / `/**`) |
|---|---|---|
| `rust/core/src/sdr_tonemap.rs` `[verify post-sprint code-map.md]` | ~320 | `Requested { Off, Auto, Bt2390 }`, `Effective`, `PeakTracker` (fast-attack/slow-release EMA), exact CPU reference `bt2390_eetf()`, `gamut_compress_2020_to_709()`, `tonemap_pixel()`, wire parser for `rlSdrToneMap` |
| `rust/core/native-api/src/stream.rs` `[verify post-sprint code-map.md]` | ~160 | `//!` New Native API v1 stream-settings DTOs (`StreamSettings`, `SessionFacts`, `StreamSetting`, `SettingSource`, `StreamSettingsWrite`), matching `docs/encoder-focus-design.md` §8 |
| `rust/host/src/native_api/stream_settings.rs` `[verify post-sprint code-map.md]` | ~240 | `//!` New Axum handlers for `GET` & `POST /api/native/v1/stream/settings` (`launch` permission scope, paired-device override persistence, live `Session::sdr_tonemap` apply, plus `pub(crate) fn read`/`write` couch-menu hook) |
| `rust/windows/src/compute_tonemap.rs` `[verify post-sprint code-map.md]` | ~220 | Lazy D3D12 tonemap PSO cache (`yuv420_tonemap_cs`, `yuv420_focus_tonemap_cs`, `chroma_pack_tonemap_cs`, `pyro_tonemap_cs`), `hdr_stats` integration for SDR streams, constant packing |
| `rust/windows/src/gpu_color/tonemap.rs` `[verify post-sprint code-map.md]` | ~240 | Lazy D3D11 tonemap PS/CS bundle (`luma_tonemap`, `chroma_tonemap`, 4:4:4/PyroWave variants) and shared `cbuffer Config` packer |
| `rust/windows/src/compute_tonemap_tests.rs` `[verify post-sprint code-map.md]` | ~340 | GPU `#[ignore]` tests verifying D3D12 & D3D11 shader output matches `rust/core/src/sdr_tonemap.rs` + `rust/windows/src/color.rs` within `<= 1` LSB across scRGB FP16 and HDR10 PQ ramps, plus unchanged `yuv420_cs` DXBC hash assertion |
| `rust/host/src/stream/tonemap.rs` `[verify post-sprint code-map.md]` | ~180 | Per-session `ToneMapController` combining `Session::sdr_tonemap`, `PeakTracker`, display `Metadata`, and live `sdrWhiteScale`; logs mode transitions and 5 s telemetry if changed |
| `client/app/src/main/java/com/limelight/preferences/NativeStreamSettingsSection.java` `[verify post-sprint code-map.md]` | ~180 | `/**` Renders generic `/api/native/v1/stream/settings` choice entries (including `hdr_sdr_tonemap`) inside `HostStreamSettings` and Stream Menu without growing `HostStreamSettings.java` |
| `client/protocol/desktop/src/strings_70/<code>.rs` (14 per-locale files from Design 02) `[verify post-sprint code-map.md]` | +~15 LOC each (<=800) | All 14 non-English Desktop UI strings for HDR-to-SDR Tone Mapping (English literals at call sites; zero lines added to grandfathered `strings/*.rs`) |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### 4.1 Resolution State Machine (`butterpollo_core::sdr_tonemap::resolve`)

Inputs per frame: `(requested: Requested, stream_hdr: bool, source_pixel: Pixel, display_peak_nits: f32, live_peak_nits: Option<f32>, sdr_white_nits: f32)`.

| `requested` | `stream_hdr` | `source_pixel` | Effective Shader Mode | `toneMapPeakNits` Source |
|---|---|---|---|---|
| Any | `true` (HDR stream) | Any | `Inactive` (existing HDR PSO) | N/A |
| `Off` | `false` (SDR stream) | `Bgra8` / `RgbaF16` / `Rgba10Pq` | `Off` (legacy `saturate` / linear clamp for PQ) | N/A |
| `Auto` or `Bt2390` | `false` | `Bgra8` (SDR display) | `Inactive` (existing unmodified `yuv420_cs` PSO!) | N/A |
| `Auto` or `Bt2390` | `false` | `RgbaF16` (scRGB HDR) | `Bt2390` (`*_tonemap` PSO) | `PeakTracker(live_peak_nits, display_peak_nits)` |
| `Auto` or `Bt2390` | `false` | `Rgba10Pq` (HDR10 PQ) | `Bt2390` (`*_tonemap` PSO) | `PeakTracker(live_peak_nits, display_peak_nits)` |

- **Zero state mutation on the OS:** HDR-to-SDR tone mapping runs strictly inside the host's per-stream shader pass and never toggles Windows HDR, EDID, ICC profiles, or display topology. Therefore **no crash-restore journal entry is needed** — if the host crashes, Windows displays remain completely untouched.

### 4.2 Reconnect & Runtime Transition Matrix

| Scenario | Behavior |
|---|---|
| **Windows HDR toggled ON/OFF mid-stream (Win+Alt+B)** | Capture switches between `Pixel::Bgra8` and `Pixel::RgbaF16`/`Pixel::Rgba10Pq`. `ToneMapController` detects `source_pixel` change on the very next frame, resets `PeakTracker` to the new display metadata, and switches PSO cleanly without dropping the stream or restarting the encoder. |
| **User drags Windows "SDR content brightness" slider mid-stream** | Display `sdrWhiteScale` updates at the 1 s `metadata_due` tick (`run.rs:1665`), updating `toneMapTargetNits` smoothly on the next frame. |
| **Client toggles `hdr_sdr_tonemap` live via Stream Menu (`/stream/settings`)** | `Session::sdr_tonemap` atomic updates; next frame switches PSO (`yuv420_cs` <-> `yuv420_tonemap_cs`), zero encoder restart, zero IDR. |
| **Client reconnects (`/resume`) with different SDR/HDR capability (e.g., HDR TV -> SDR Phone via Continue Here)** | New stream negotiates `hdr = 0` while display stays HDR (`RgbaF16`/`Rgba10Pq`); `ToneMapController` engages BT.2390 immediately on frame 1 so the SDR phone never sees blown-out highlights. |
| **D3D12 compute demotes to D3D11 graphics (`ComputeDemotion`)** | `gpu_color.rs` uses the exact same `cbuffer Config` slots 17–19 and `luma_tonemap`/`chroma_tonemap` shaders; `PeakTracker` smoothly falls back from live `hdr_stats` to static `display_metadata.maximum_nits`. |
| **Stock Moonlight client (no `rlSdrToneMap`)** | Uses host/device/app `sdr_tonemap` setting transparently; zero extra control traffic or DESCRIBE dependency. |
| **Encoder focus + HDR-to-SDR tone mapping both enabled** | `yuv420_focus_tonemap_cs` combines both in one pass (hashing post-tonemap SDR luma/chroma so still-age detection works cleanly on the encoded SDR picture). |
| **Couch menu overlay active during HDR-to-SDR tone mapping** | Couch menu runs in its own `yuv420_menu_cs` pass with sRGB panel white at SDR `1.0`, so the menu is never double-tonemapped! |

---

## 5. Settings & 10/10 Craft Plan

### 5.1 Fewer-Settings Rule & Default-Off Gate

- **One setting only:** `sdr_tonemap = off | auto | bt2390` (host config + per-device + per-app override in `override_allowed`, surfaced in Console 2.0 under Picture/HDR and in Nightjar via `/api/native/v1/stream/settings` — **zero new local Android/Desktop preference keys**).
- **Default-off gate until measured:** Ships with default **`off`** in the initial Phase 3 merge; Wave 1 hardware verification (`07-hw-verify.md`) measures D3D12 & D3D11 convert p99 (`<= +0.05 ms` on SDR-on-SDR and `<= +0.15 ms` on HDR-to-SDR) and golden luminance/gamut scores on RX 7900 XT + Radeon 780M. Only once all gates pass is the default promoted to **`auto`** (just like `encoder-focus-design.md`'s default-on rule).
- **2-sentence help text (Console 2.0 + Nightjar):**
  - `"Maps HDR games smoothly to SDR screens so bright highlights keep detail instead of clipping white. Runs inside the existing colour pass with negligible GPU cost; leave on Automatic unless you prefer raw highlight clipping."`

### 5.2 10/10 Craft & Policy Compliance

- **Zero `"Experimental"` in UI:** No "Experimental" badge or wording anywhere.
- **Zero mod names in UI/Nightjar:** Strictly host-neutral terminology ("HDR-to-SDR tone mapping", "BT.2390").
- **Zero stream-time blur:** Stream Menu row uses the standard opaque/elevated `StreamMenu` choice row without backdrop blur.
- **Dynamic input glyphs:** Choice confirmation/back prompts in Nightjar Stream Menu render active remote/controller/keyboard glyphs; touch and mouse show no button hints (`client/design/README.md:143`).
- **Plain-English non-configurable states:**
  - On an HDR stream: row disabled with subtitle `"Stream is already HDR"`.
  - On an invalid launch value (`rlSdrToneMap=bogus`): session warning `"Unknown HDR-to-SDR tone mapping mode requested; highlight tone mapping is off for this stream."` (never silently ignored).
- **All 15 locales:** Every label, option, description, and warning translated across all 15 locales (`en`, `de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`) across `rust/assets/locale/*.json`, `client/app/src/main/res/values*/strings.xml`, and `client/protocol/desktop/src/strings_70/<code>.rs` (canonical 7.0.0 overflow files from Design 02; zero lines added to grandfathered `strings/*.rs`).

---

## 6. Disjoint Implementer Slices

All slices have **strictly non-overlapping file lists** for Phase 3 `parallel()`.

| Slice | Parallel Group | Exclusive File List (zero overlap!) |
|---|---|---|
| **6A: Core BT.2390 Math, PeakTracker & Config** | Stage 1 | `rust/core/src/lib.rs`, `rust/core/src/sdr_tonemap.rs` (new), `rust/core/src/config.rs`, `rust/core/src/config/overrides.rs`, `rust/core/src/config/tests.rs`, `rust/core/src/amf_vpe.rs`, `rust/core/src/extensions.rs`, `rust/core/src/rtsp.rs` |
| **6B: Native API Wire Types & Fixtures** | Stage 1 (parallel with 6A) | `rust/core/native-api/src/lib.rs` (`pub mod stream;`, `feature::STREAM_SETTINGS = "stream.settings"`, `feature::HDR_SDR_TONEMAP = "video.sdr_tonemap"`), `rust/core/native-api/src/stream.rs` (new: `StreamSettings`, `SessionFacts`, `StreamSetting`, `SettingSource`, `StreamSettingsWrite`), `rust/core/native-api/src/session.rs`, `rust/core/native-api/tests/fixtures.rs`, `rust/core/native-api/tests/fixtures/stream_settings_tonemap.json` (new), `rust/core/native-api/tests/fixtures/stream_settings_write.json` (new), `docs/native-api.md` |
| **6C: Shaders, D3D12 Compute, D3D11 GPU Color & CPU Reference** | Stage 2 (after 6A) | `rust/windows/src/shaders/color.hlsl`, `rust/windows/build_shaders.rs`, `rust/windows/src/color.rs`, `rust/windows/src/compute.rs`, `rust/windows/src/compute_tonemap.rs` (new), `rust/windows/src/compute_hdr_stats.rs`, `rust/windows/src/gpu_color.rs`, `rust/windows/src/gpu_color/tonemap.rs` (new), `rust/windows/src/compute_tonemap_tests.rs` (new), `rust/windows/src/encoder.rs`, `rust/windows/src/amf.rs`, `rust/windows/src/nvenc.rs`, `rust/windows/src/nvenc/tests.rs` |
| **6D: Host Stream Controller, New `/stream/settings` Endpoints & Console 2.0** | Stage 3 (after 6B+6C) | `rust/core/src/session.rs`, `rust/host/src/stream/tonemap.rs` (new), `rust/host/src/stream.rs`, `rust/host/src/stream/video/run.rs`, `rust/host/src/extensions.rs`, `rust/host/src/nvhttp/launch.rs`, `rust/host/src/native_api.rs` (registers `mod stream_settings;`, `GET` + `POST /api/native/v1/stream/settings` routes, and `feature::STREAM_SETTINGS` + `feature::HDR_SDR_TONEMAP` in `capabilities`), `rust/host/src/native_api/stream_settings.rs` (new: implements additive `GET`/`POST /api/native/v1/stream/settings` matching `docs/encoder-focus-design.md` §8 with `launch` permission scope, paired-device override persistence, live `Session::sdr_tonemap` apply, and `pub(crate) fn read`/`write`, consuming 6B's `rust/core/native-api/src/stream.rs` DTOs), `rust/host/src/nvhttp/native_tests/tonemap.rs` (new), `rust/host/src/stream/reconnect_tests/tonemap_tests.rs` (new), `rust/host/src/console/settings.rs`, `rust/web/src/lib/schema/video.ts`, `rust/web/SettingsSchema.test.mjs`, `rust/assets/locale/*.json`, `docs/configuration.md` |
| **6E: Nightjar Capabilities, Launch Param & Stream Menu** | Stage 2 (parallel with 6C after 6B) | `client/protocol/client/src/http/capabilities.rs`, `client/protocol/client/src/http/native.rs` (`stream_settings()` / `write_stream_settings()`), `client/protocol/client/tests/capabilities_contract.rs`, `client/protocol/desktop/src/stream/negotiate.rs`, `client/protocol/desktop/src/strings_70/*.rs` (per-locale `strings_70/<code>.rs`; never editing grandfathered `strings/*.rs`), `client/protocol/desktop/src/strings/tests.rs`, `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java`, `client/app/src/main/java/com/limelight/nvstream/http/NvHTTP.java`, `client/app/src/main/java/com/limelight/preferences/HostStreamSettings.java`, `client/app/src/main/java/com/limelight/preferences/NativeStreamSettingsSection.java` (new), `client/app/src/main/res/values*/strings.xml`, `client/app/src/test/resources/protocol/capabilities/*` |

*(Every path above is marked `[verify post-sprint code-map.md]` for Phase 2 reconciliation. Slice 6B creates the `rust/core/native-api/src/stream.rs` DTOs and feature constants in Stage 1 so 6D and 6E share one wire contract with zero file overlap; Slice 6D creates the `rust/host/src/native_api/stream_settings.rs` endpoints and router registration in `rust/host/src/native_api.rs`.)*

---

## 7. Verification & Measurement Plan

### 7.1 Narrowest Unit, Reference & Contract Tests (No GPU Required)

- **Core math & policy tests:**
  - `cargo test -p butterpollo-core --locked sdr_tonemap`:
    - Monotonicity of `bt2390_eetf` across `0..=10000` nits (`E_out(a) <= E_out(b)` for `a <= b`).
    - Pure SDR identity check: for `peak_nits == sdr_white_nits == 203.0`, every `0..=203` nit ramp value maps within `1e-5` of `L / 203.0` (zero desktop dimming).
    - Highlight preservation: 400, 1000, 2000, 4000 nit inputs produce strictly distinct monotonically increasing SDR codes below `255` (never collapsing to flat `255`).
    - Wide-gamut Rec.2020 pure primaries (`(1, 0, 0)`, `(0, 1, 0)`, `(0, 0, 1)` in Rec.2020) compress into `[0, 1]` Rec.709 with hue preserved and zero NaN/Inf.
    - `PeakTracker` fast-attack (`0.25 s`) and slow-release (`2.0 s`) step response with zero oscillation.
    - `rlSdrToneMap` parser rejects unknown values loudly.
- **Native API DTO & fixture tests:**
  - `cargo test -p rubylight-native-api --locked` (round-trips `StreamSettings`, `StreamSetting`, `StreamSettingsWrite` fixtures and asserts `feature::STREAM_SETTINGS == "stream.settings"` and `feature::HDR_SDR_TONEMAP == "video.sdr_tonemap"`).
- **NVIDIA header/sample & fake-driver check (`docs/nvenc.md#checking-nvidia-code`):**
  - `cargo test -p butterpollo-windows --locked nvenc` (confirms D3D12 compute + D3D11 `gpu_color` + native 4:4:4 AYUV/Planar16 paths forward `toneMapMode`/`toneMapPeakNits`/`toneMapTargetNits` identically; never claimed as run on NVIDIA hardware).
- **Host reconnect & native API tests:**
  - `cargo test -p butterpollo --locked tonemap` (verifies new `GET` & `POST /api/native/v1/stream/settings` routes, `launch` permission scope, `not_paired` `403`, `permission_required` `403`, `invalid_value` `422`, `not_writable` `422`, `inherit` clearing device override, live toggle, `rlSdrToneMap` launch opt-in, stock Moonlight unchanged, HDR stream marks setting `read_only` with `stream_is_hdr`, and `/capabilities` advertising `stream.settings` + `video.sdr_tonemap`).
- **Nightjar & arch checks:**
  - `cargo test -p nightjar-core --locked capabilities_contract`
  - `cargo test -p nightjar-desktop --locked strings`
  - `python tools/arch_check.py` (specifically verifying `gpu_color.rs <= 800` and `compute.rs <= 2079`!)
  - `python tools/code_map.py check`

### 7.2 GPU Parity & Visual Screenshot Checks

- **Bit-exact GPU vs. CPU tests (`#[ignore]`, user idle):**
  - `cargo test -p butterpollo-windows --locked compute_tonemap -- --ignored --test-threads=1 --nocapture`:
    - Asserts `yuv420_cs` DXBC SHA-256 hash is **unchanged** from baseline.
    - Runs both D3D12 (`compute.rs`) and D3D11 (`gpu_color.rs`, NV12, P010, AYUV, Planes444) over synthetic `Pixel::RgbaF16` (0–4000 nit ramp + Rec.2020 color checker + SDR white UI patch) and `Pixel::Rgba10Pq` textures and asserts every pixel matches `butterpollo_core::sdr_tonemap::tonemap_pixel` within `<= 1` 8-bit/10-bit code unit (LSB).
    - Verifies `hdr_stats_cs` reads unclipped HDR peak when `hdr == 0 && (pixel == 1 || pixel == 2)`.
- **Visual inspection:** Console 2.0 `SettingsSchema.test.mjs` + `screenshots.cjs` showing the Picture/HDR select row; side-by-side SDR output frame captures (hard clip vs. BT.2390) archived in the Wave 1 hardware run directory.

### 7.3 Serialized Idle-Gated Hardware Criteria

Executed under [07-hw-verify.md](07-hw-verify.md) on **RX 7900 XT desktop** and **Radeon 780M laptop**:

1. **Idle gate:** `/serverinfo` confirms `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0`, `GPU-LOCK` held, isolated test host on `--port 48123 --bind 127.0.0.1`.
2. **ABBA timing budget (`e2e.py --record` + `timing_report.py`, 1080p60 & 4K60 HEVC):**
   - **Default SDR-on-SDR path:** convert p99 delta `<= +0.05 ms` (expected `0.00 ms`, identical shader), encode p99 `<= +0.10 ms`, `present_to_sent` p99 `<= +0.20 ms`, `slice_delivery_overflows` `0` increase.
   - **Active HDR-to-SDR path (both D3D12 compute and D3D11 graphics fallback):** convert p99 overhead `<= +0.15 ms` at 4K60 (single pass, zero extra barrier), `present_to_sent` p99 within `+0.20 ms`, `0` frame drops.
3. **Default-on promotion criterion:** Promote `sdr_tonemap` default from `off` to `auto` ONLY after both RX 7900 XT and Radeon 780M pass all golden luminance/gamut checks and ABBA timing bounds.
4. **Exit code 3 abort rule:** Exit code **3** from `e2e.py` or `check.ps1` is an AMD encoder hang — **STOP immediately**, preserve `gpu_hang` report, and investigate without retrying.

---

## 8. Post-Discovery & 6.3.0 Rebase Addendum (Critical `#142` Luminance & Dynamic VDD EDID)

- **Critical 6.3.0 (`7f0e6e287d`) `sdr_white_nits` Bug Fix:** On `origin/main` `6.3.0` (`rust/host/src/stream/video/run.rs:1599-1607`), `Encoder::set_luminance(image.gpu.sdr_white_nits(), 1.0)` ONLY fires when `stream.hdr == true` (or RTX HDR is active), leaving converters frozen at `[100.0, 1.0]` for all `stream.hdr == false` sessions! Slice 6B/6C MUST drive `Encoder::set_luminance(image.gpu.sdr_white_nits(), 1.0)` inside `ToneMapController` whenever Source Pixel is `RgbaF16` / `Rgba10Pq` and SDR tone-mapping is active — **even when `stream.hdr == false`** — and clearly separate Windows `DISPLAYCONFIG_SDR_WHITE_LEVEL` (`80..1000` nits from `7f0e6e287d`) from BT.2390 peak target so pure SDR desktop content never dims or clips on 200–480+ nit HDR monitors.
- **6.3.0 (`3b660899c5`) `fallback_hdr_if_sdr()` & 10-Bit SDR Interplay:** Define explicit state transitions when `fallback_hdr_if_sdr()` triggers: if source stays `Bgra8`, keep tone-mapping `Inactive` (never double-switch HDR state); when `ten_bit == true && !stream_hdr` with an HDR desktop source, engage BT.2390 cleanly onto the 10-bit SDR target.
- **Dynamic `rubylight-vdisplay` EDID Mastering Luminance:** Replace hardcoded zeros (`max_frame_avg_nits: 0, min_luminance_millinits: 0`) in `rust/windows/src/display/rubylight_vdd.rs:240-242` with real client HDR mastering values, and trigger a controlled virtual-display EDID refresh when client roaming / Continue Here (`01`/`08`) shifts peak luminance by `> 100` nits (e.g., 400-nit OLED handheld -> 1000-nit TV) so DXGI `MaxLuminance` matches the active screen.
