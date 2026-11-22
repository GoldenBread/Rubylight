# Item 12: Phone Safe Area & Reverse Landscape (#152)

Status: design, not implemented (Wave 2 — Rubylight & Nightjar 7.0.0). Written against `main` `b3d0c26b9b` (`6.2.0`), `docs/designs/00-protocol-reservations.md`, and `tools/size_ratchet.json`.

## 1. Summary & User Experience

Phone gamers increasingly play with telescopic USB-C controllers (whose connector sits on the right or left side), angled USB-C charging/headphone cables, or side volume/power buttons that require holding the phone rotated 180° into **Reverse Landscape** (`Surface.ROTATION_270`). Today in Nightjar (`client/app/src/main/java/com/limelight/ui/StreamOrientation.java`), `LANDSCAPE` requests `ActivityInfo.SCREEN_ORIENTATION_USER_LANDSCAPE` — which obeys Android's system portrait rotation lock by locking exclusively to standard landscape (`ROTATION_90`), refusing to flip right-side-up when mounted in a reverse-edge controller!

At the same time, modern phones combine asymmetrical hole-punch cameras (which flip from the **left** edge in `ROTATION_90` to the **right** edge in `ROTATION_270`), large OLED corner radii, bottom gesture-navigation pills, and foldable hinges. While `AdaptiveLayout.java` handles basic display cutouts on the root container, video framing lacks a dedicated cutout-safe **Safe-Fit** option, rounded-corner insets are ignored by corner HUD overlays, and on-screen controller buttons or performance stats can sit partially clipped behind camera punches or trigger Android's bottom gesture bar when flipped to Reverse Landscape.

**Phone Safe Area & Reverse Landscape (#152)** solves both halves cleanly on the client with **zero host protocol changes**:

- **Reverse Landscape & Both-Ways Landscape:** Extends the existing `stream_orientation` picker with **Landscape (either side)** (`SCREEN_ORIENTATION_SENSOR_LANDSCAPE`, flips freely between standard and reverse landscape even when portrait rotation lock is on!) and **Reverse landscape** (`SCREEN_ORIENTATION_REVERSE_LANDSCAPE`, hard-locks 180° for playing in bed without accelerometer drift), plus a one-tap **Flip 180°** quick action in the Stream Menu when in landscape.
- **Deterministic Safe-Area Policy (`nightjar-policy::safe_area`):** Computes exact, non-overlapping screen rectangles from `DisplayCutout`, `RoundedCorner` (API 31+), `mandatorySystemGestures` (gesture bar), `systemBars`, and `FoldingFeature` hinges across `ROTATION_0 / 90 / 180 / 270`:
  - **Video framing (`Fit` / `Safe-Fit` / `Fill` / `Stretch` / `1:1`):**
    - **Safe-Fit** frames the video strictly inside the cutout- and corner-safe rectangle so zero game HUD pixels vanish behind a hole-punch camera in either landscape direction. When connected to a virtual-display host that supports `0x5532` reconfigure (`StreamResizePolicy`), Safe-Fit can optionally size the virtual display to match the exact cutout-free pixel area!
    - **Fit** keeps aspect ratio across the full window (clearing only a half-opened foldable hinge).
    - **Fill** spans edge-to-edge beneath the camera cutout on both `ROTATION_90` and `ROTATION_270` (`LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES` / `ALWAYS`).
  - **Overlays & Controls ALWAYS stay safe:** Performance overlay, Stream Menu hint, Stream Menu / ActionSheet, and on-screen controller (`PadControlView`) ALWAYS clear the active-side camera punch, rounded corners, and bottom gesture bar — even when video itself is in `Fill` mode!
  - **External displays (`StreamPresentation`) stay edge-to-edge:** A TV or monitor connected via USB-C DisplayPort / HDMI never inherits the phone's camera cutout insets, while touch controls on the phone screen keep the phone's safe area.
- **Zero stream-time blur or extra compositor layers:** Safe-area geometry applies purely through view bounds and padding — zero `RenderEffect` blur, zero extra `SurfaceView` layers, zero added frame latency.

## 2. Protocol, Native API & Capability Contract

### Compatibility Invariants

1. **100% Client-Side Architecture:** Per `docs/designs/00-protocol-reservations.md` Section 7, Item 12 introduces **ZERO** new control message IDs, **ZERO** `/serverinfo` elements, **ZERO** RTSP `DESCRIBE` / `ANNOUNCE` attributes, and **ZERO** Native API v1 routes. `PROTOCOL_VERSION` and `API_VERSION` are completely untouched.
2. **Reuses existing `0x5532` (`RECONFIGURE_MESSAGE_TYPE`):** When a stream follows the device's panel shape on a host that already advertises `RECONFIGURE_MESSAGE_TYPE` (checked via existing `MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_RECONFIGURE)` + `HostCapabilities::has(Feature::VirtualDisplay)` — never brand-gated!), switching between `Safe-Fit` and full-panel `Fill`, or folding/unfolding a foldable, simply passes the safe-area rectangle into existing `StreamResizePolicy.nativeSize(...)` and sends standard `0x5532`.
3. **180° Landscape Flip sends ZERO `0x5532` traffic when dimensions match:** Flipping a phone from `ROTATION_90` to `ROTATION_270` mirrors the cutout from left to right while keeping the safe-area `width × height` identical — so Nightjar updates local layout and touch/overlay coordinates instantaneously with **zero host reconfigure, zero IDR, and zero frame stutter**!
4. **Stock Moonlight / Sunshine / Apollo compatibility:** Works identically against every host (Rubylight, Sunshine, Apollo, GameStream), because orientation, cutout safe area, and `VideoScaling` framing are pure client presentation policies.

### Orientation Mode Mapping (`StreamOrientation.java`)

Stored under the **existing** preference key `StreamOrientation.PREF = "stream_orientation"` (zero new preference key!):

| Stored Value | `StreamOrientation` Constant | Android `ActivityInfo` Request | Behavior |
| --- | --- | --- | --- |
| `"auto"` | `AUTO` (`0`, default) | Existing rules (`USER_LANDSCAPE` / `FULL_USER` on tablets/foldables) | Unchanged default for existing users |
| `"landscape"` | `LANDSCAPE` (`1`) | `SCREEN_ORIENTATION_USER_LANDSCAPE` | Standard landscape (existing behavior preserved) |
| `"landscape_flip"` | `LANDSCAPE_FLIP` (`4`, new) | `SCREEN_ORIENTATION_SENSOR_LANDSCAPE` | Landscape on **either side** (flips 180° for USB-C controllers even with Android portrait rotation lock ON; never flips to portrait) |
| `"reverse_landscape"` | `REVERSE_LANDSCAPE` (`5`, new) | `SCREEN_ORIENTATION_REVERSE_LANDSCAPE` | Locks 180° reverse landscape (ideal when lying down with a bottom-port USB-C grip) |
| `"portrait"` | `PORTRAIT` (`2`) | `SCREEN_ORIENTATION_USER_PORTRAIT` | Portrait lock (unchanged) |
| `"rotate"` | `ROTATE` (`3`) | `SCREEN_ORIENTATION_FULL_USER` | All 4 orientations within system lock (unchanged) |

*(Older builds encountering unknown `"landscape_flip"` or `"reverse_landscape"` fall safely back to `AUTO` via `StreamOrientation.fromValue()`.)*

### Video Framing Mapping (`VideoScaling.java`)

Stored under the **existing** preference key `VideoScaling.PREF = "video_scaling"` (`legacyStretch` stays `false` for `SAFE_FIT`, matching `FIT` on older builds):

| Stored Value | `VideoScaling` Constant | Video Safe Area | Overlay / Menu / OSC Safe Area |
| --- | --- | --- | --- |
| `"fit"` | `FIT` (`0`) | Full window minus half-opened fold hinge; preserves aspect ratio | Cutout + rounded corners + gesture bar + fold hinge |
| `"safe_fit"` | `SAFE_FIT` (`4`, new) | Cutout + corner-safe inset + fold hinge; preserves aspect ratio (zero pixels behind camera punch!) | Cutout + rounded corners + gesture bar + fold hinge |
| `"fill"` | `FILL` (`1`) | Full window edge-to-edge (under cutout on both landscape sides) | Cutout + rounded corners + gesture bar + fold hinge |
| `"stretch"` | `STRETCH` (`2`) | Full window edge-to-edge (legacy `checkbox_stretch_video = true`) | Cutout + rounded corners + gesture bar + fold hinge |
| `"native"` | `NATIVE` (`3`) | 1:1 pixels inside safe area unless full-display native stream | Cutout + rounded corners + gesture bar + fold hinge |

## 3. Architecture & <=800-LOC Module Plan

### Deterministic Safe-Area Policy (`client/protocol/policy/src/safe_area.rs`)

All safe-area geometry lives in pure, platform-agnostic Rust in `nightjar-policy` with a lockstep Java mirror verified by shared JSON trace replay (`client/protocol/policy/tests/traces/safe_area.json`):

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Insets { pub left: i32, pub top: i32, pub right: i32, pub bottom: i32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CornerRadii { pub tl: i32, pub tr: i32, pub br: i32, pub bl: i32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect { pub left: i32, pub top: i32, pub right: i32, pub bottom: i32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing { Fit, SafeFit, Fill, Stretch, Native }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SafeAreaInput {
    pub window_w: i32,
    pub window_h: i32,
    pub cutout: Insets,
    pub system_bars: Insets,
    pub mandatory_gestures: Insets,
    pub corners: CornerRadii,
    pub fold_hinge: Option<Rect>,
    pub fold_vertical: bool,
    pub fold_half_opened: bool,
    pub rtl: bool,
    pub windowed: bool,
    pub pip: bool,
    pub external_presentation: bool,
    pub television: bool,
    pub tv_margin_px: i32,
    pub framing: Framing,
    pub follow_screen_virtual_display: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SafeAreaLayout {
    pub video_area: Rect,
    pub overlay_area: Rect,
    pub menu_area: Rect,
    pub osc_area: Rect,
    /// Non-zero safe area fed to StreamResizePolicy.nativeSize when virtual display follows shape.
    pub virtual_display_area: Rect,
}
```

Key deterministic geometry rules:
1. **Rounded-corner chord inset:** For a circular corner of radius `r`, the diagonal clearance needed for a rectangular HUD/button box to stay completely inside the lit OLED curve is `ceil(r * (1 - 1/sqrt(2))) ≈ (r * 293 + 999) / 1000` (exact integer math — zero floating-point drift across Rust/Java!).
2. **External presentation bypass (`external_presentation == true`):** `video_area = Rect(0, 0, window_w, window_h)` with zero phone cutout/corner insets; `StreamPresentation` remains edge-to-edge on TV/monitor while phone-screen overlays keep phone insets.
3. **Picture-in-Picture (`pip == true`):** All insets collapse to `0` (`Rect(0, 0, window_w, window_h)`), matching `AdaptiveLayout.java:144`.
4. **Half-opened fold (`fold_half_opened == true`):** Splits `available` along the hinge via existing `usablePane` semantics; a `FLAT` fold NEVER splits or shrinks the video (preserving `AdaptiveLayout.java:54`).
5. **`video_area` vs `overlay_area` / `osc_area` separation:**
   - `video_area` applies `cutout` + corner horizontal inset when `framing == SafeFit` (or `windowed == true`), and full pane when `framing ∈ {Fit, Fill, Stretch}` (with `Fit` centering inside full pane and `Fill`/`Stretch` covering beneath cutout).
   - `overlay_area` ALWAYS applies `max(cutout, corner_insets)` on all 4 edges + `tv_margin_px` (if TV), and `mandatory_gestures.bottom` so the performance overlay and menu hint never collide with a punch-hole, curved corner, or gesture pill on either `ROTATION_90` or `ROTATION_270`.
   - `osc_area` ALWAYS applies `max(cutout, corner_insets)` horizontally and `max(cutout.bottom, mandatory_gestures.bottom, corner_insets.bottom)` vertically so thumb controls never trigger Android's swipe-home bar.
   - `menu_area` ALWAYS applies `max(cutout, system_bars, corner_insets)` + fold pane so ActionSheet / StreamMenu items remain 100% readable and reachable.
6. **Non-empty clamp guard:** If pathological OEM insets would collapse any rectangle below `MIN_DIMENSION` (`256 px` for video, `120 px` for overlays), policy clamps symmetrically so layout never divides by zero or produces inverted rects (`right < left`).

### Size-Ratchet Guardrail (`tools/size_ratchet.json`)

| Grandfathered / Touched File | Ratchet / Current LOC | Strategy to Avoid Growth |
| --- | --- | --- |
| `client/app/src/main/java/com/limelight/Game.java` `[verify post-sprint code-map.md]` | 1883 LOC (ratchet) | **Zero net LOC growth!** Delegate inset/rotation handling to `StreamSafeAreaController` via existing `GameVideoOutput` hooks. |
| `client/app/src/main/java/com/limelight/GameVideoOutput.java` `[verify post-sprint code-map.md]` | 1346 LOC (ratchet) | Keep <=12-line delegation into new `StreamSafeAreaController.java` (and trim duplicated inset logic so net LOC stays <= 1346!). |
| `client/app/src/main/java/com/limelight/binding/input/pad/PadControlView.java` `[verify post-sprint code-map.md]` | 1023 LOC (ratchet) | Add a 6-line `setSafeAreaBounds(Rect)` setter; all safe-area clamping math stays in `SafeAreaPolicy.java`. |
| `client/app/src/main/java/com/limelight/ui/StreamMenu.java` `[verify post-sprint code-map.md]` | 1321 LOC (ratchet) | Add single "Flip screen 180°" row (visible only in landscape on non-TV phones) via 5-line hook into `StreamSafeAreaController.toggleLandscapeFlip()`. |
| `client/app/src/main/java/com/limelight/ui/ActionSheet.java` `[verify post-sprint code-map.md]` | 1396 LOC (ratchet) | Consume `menu_area` from `AdaptiveLayout` — zero net growth. |
| `client/app/src/main/java/com/limelight/preferences/PreferenceConfiguration.java` `[verify post-sprint code-map.md]` | 1961 LOC (ratchet) | **Zero new keys** (reuses `stream_orientation` and `video_scaling`); zero net growth! |
| `client/app/src/main/java/com/limelight/preferences/StreamSettings.java` `[verify post-sprint code-map.md]` | 3425 LOC (ratchet) | Zero new rows needed — existing `stream_orientation` and `video_scaling` list preferences automatically pick up the new entries from XML arrays! |
| `client/app/src/test/java/com/limelight/preferences/SettingsKeysTest.java` `[verify post-sprint code-map.md]` | 976 LOC (ratchet) | Remains completely unchanged (verifies zero key regressions automatically!). |

### New & Touched Module Breakdown

| Path | Layer | Est. LOC | Responsibility |
| --- | --- | --- | --- |
| `client/protocol/policy/src/safe_area.rs` (new) | `nightjar-policy` | ~380 | Pure safe-area solver (`compute(&SafeAreaInput) -> SafeAreaLayout`), rounded-corner chord math, fold-hinge split, `virtual_display_area` sizing, trace replay harness |
| `client/app/src/main/java/com/limelight/ui/SafeAreaPolicy.java` (new) | `nightjar-android` | ~320 | Lockstep Java implementation of `nightjar_policy::safe_area::compute` + Android `WindowInsetsCompat` & API 31+ `RoundedCorner` extractor (graceful `0` on API < 31) |
| `client/app/src/main/java/com/limelight/ui/StreamSafeAreaController.java` (new) | `nightjar-android` | ~360 | Coordinates `AdaptiveLayout` insets, rotation observation (`DisplayManager.DisplayListener` for `ROTATION_90 ↔ ROTATION_270` since Android does NOT fire `onConfigurationChanged` on 180° flips!), applies `video_area` to `StreamView`, `overlay_area` to `GameOverlays`, `osc_area` to `PadControlView`, and triggers `0x5532` resize ONLY when `virtual_display_area` dimensions change |
| `client/app/src/main/java/com/limelight/ui/StreamOrientation.java` `[verify post-sprint code-map.md]` | `nightjar-android` | 136 → ~185 | Adds `LANDSCAPE_FLIP` (`SCREEN_ORIENTATION_SENSOR_LANDSCAPE`), `REVERSE_LANDSCAPE` (`SCREEN_ORIENTATION_REVERSE_LANDSCAPE`), and `flipLandscape(currentRotation)` helper |
| `client/app/src/main/java/com/limelight/ui/VideoScaling.java` `[verify post-sprint code-map.md]` | `nightjar-android` | 134 → ~160 | Adds `SAFE_FIT` (`4`, `"safe_fit"`) with `legacyStretch(SAFE_FIT) == false`, `usesCutoutSafeArea(mode)` helper |
| `client/app/src/main/java/com/limelight/ui/AdaptiveLayout.java` `[verify post-sprint code-map.md]` | `nightjar-android` | 273 → ~315 | Delegates streaming safe-area math to `SafeAreaPolicy` and notifies `StreamSafeAreaController` with full `SafeAreaLayout` (video, overlay, menu, OSC rects separately instead of one coarse root padding!) |
| `client/app/src/main/java/com/limelight/ui/StreamWindow.java` `[verify post-sprint code-map.md]` | `nightjar-android` | 17 → ~28 | Adds `int videoScalingMode()` so `AdaptiveLayout` knows `SAFE_FIT` vs `FIT` vs `FILL` cleanly |
| `client/app/src/main/java/com/limelight/GameOverlays.java` `[verify post-sprint code-map.md]` | `nightjar-android` | 367 → ~390 | Applies `overlay_area` margins to `performanceOverlayScroll` and `streamMenuHint` so both dodge camera punches & rounded corners in `ROTATION_90` and `ROTATION_270` |
| `client/app/src/main/java/com/limelight/ui/StreamPresentation.java` `[verify post-sprint code-map.md]` | `nightjar-android` | 32 → ~38 | Explicitly sets `external_presentation = true` (zero phone cutout padding on external display) |

*(Note on Desktop parity: Desktop Nightjar has no camera punch-hole or mobile orientation sensor, so desktop UI adds zero irrelevant phone settings — matching `client/AGENTS.md`'s "hide what doesn't apply" rule! `nightjar-policy::safe_area` still compiles cleanly in the workspace across Windows, Linux, and macOS so future handheld notch/corner devices can reuse it immediately.)*

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### Orientation & Safe-Area Layout State Machine

```
[StreamCreated]
      │  (apply StreamOrientation.requested(...) + WindowManager LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES)
      ▼
[AwaitingFirstInsets] ──(onApplyWindowInsets + WindowLayoutInfo)──► [LaidOut(rot, SafeAreaLayout)]
                                                                           │
       ┌──────────────────────────┬────────────────────────────────────────┼────────────────────┐
       ▼                          ▼                                        ▼                    ▼
 (180° flip: ROT90 ↔ ROT270  (90° turn in ROTATE mode               (Foldable open/close   (User toggles
  detected via DisplayListener  or user taps "Flip 180°")            or split-screen drag)   Safe-Fit ↔ Fill)
  — NOTE: Android skips                                                    │                    │
  onConfigurationChanged!)                                                 ▼                    ▼
       │                          │                                 [RecomputeSafeArea]  [RecomputeSafeArea]
       ▼                          ▼                                        │                    │
[RecomputeSafeArea]        [ApplyRequestedOrientation]                     ▼                    │
 (cutout mirrors L↔R;             │                            (virtual_display_area W×H       │
  W×H unchanged!)                 │                             changed & host supports         │
       │                          │                             0x5532?)                        │
       ▼                          ▼                                 ├─► NO: local relayout only
[RelayoutLocalOnly]        [WaitSettleMs(1000 ms if 90°)]           └─► YES: send 0x5532 (debounced)
 (video, HUD, OSC & touch         │                                        │
  coords update in 1 frame;       ▼                                        ▼
  ZERO 0x5532 sent!)       [Send 0x5532 if W×H changed]             [IDR arrives → StreamView updated]
```

**Critical Android Platform Invariant (`ROTATION_90 ↔ ROTATION_270`):**
Android deliberately does **NOT** invoke `Activity.onConfigurationChanged` on a 180° rotation because orientation remains `ORIENTATION_LANDSCAPE` and `screenWidthDp`/`screenHeightDp` do not swap! Therefore `StreamSafeAreaController` MUST register a `DisplayManager.DisplayListener` (cleaned up in `onDestroy`) AND re-query `Display.getRotation()` on every `onApplyWindowInsets` dispatch so a 180° flip immediately re-runs `SafeAreaPolicy.compute`, mirrors the cutout margin from left to right, shifts the performance overlay & on-screen controller cleanly off the camera punch, and updates `VideoScaling.toStream` / `toScreen` touch coordinates on the very next frame!

### Reconnect & Edge-Case Matrix

| Scenario | Expected Behavior |
| --- | --- |
| **Phone mounted in USB-C telescopic controller (Reverse Landscape) with Android portrait rotation lock ON** | `LANDSCAPE_FLIP` (`SCREEN_ORIENTATION_SENSOR_LANDSCAPE`) rotates automatically to `ROTATION_270`; `REVERSE_LANDSCAPE` locks `ROTATION_270` even when tilting back in bed. |
| **180° flip (`ROTATION_90 → ROTATION_270`) during active stream** | `DisplayListener` + `WindowInsets` fire; `SafeAreaPolicy` mirrors cutout from left to right; `StreamView` (in `Safe-Fit`), `GameOverlays`, `StreamMenu`, and `PadControlView` reposition in 1 frame with **zero `0x5532` reconfigure and zero IDR**. |
| **Touch / absolute mouse mapping in `Safe-Fit` vs `Fill`** | `StreamView` is sized to `video_area` (in `Safe-Fit`, inset away from cutout), and `GameTouchInput` maps coordinates relative to `StreamView`'s bounds (`VideoScaling.toStream`), so pointer/touch accuracy remains pixel-exact on both `ROTATION_90` and `ROTATION_270`! |
| **External monitor via USB-C (`StreamPresentation`)** | `StreamPresentation` uses `external_presentation = true` (zero cutout insets); phone touchpad / on-screen controls stay inset on the phone's own screen. |
| **Foldable opened / half-opened (tabletop mode) / closed** | `FLAT` fold treated as single canvas (`AdaptiveLayout.java:54`); `HALF_OPENED` separating fold confines video + overlays to top/primary pane; `0x5532` reconfigures virtual display after settle delay when dimensions change. |
| **Split-screen / freeform desktop window** | `windowed = true` overrides `requested()` to `SCREEN_ORIENTATION_FULL_USER` (never fights window manager!) and keeps `systemBars | displayCutout` insets. |
| **Picture-in-Picture (PiP)** | `pip = true` collapses all insets to `0` and forces `VideoScaling.forPictureInPicture(mode)` (`FIT` or `STRETCH`), hiding overlays & OSC. |
| **Android 9 / 10 (API 28–30, before `RoundedCorner` API 31)** | `CornerRadii` safely reads as `(0, 0, 0, 0)`; `DisplayCutout` and `systemBars` still apply cleanly without reflection or crashes. |
| **Reconnect / host restart** | Orientation lock (`stream_orientation`) and framing (`video_scaling`) persist in `SharedPreferences`; temporary in-stream "Flip 180°" override stays active for the session and resets cleanly when the stream activity finishes. |

### Crash-Restore Journal

- **Zero host or OS-persistent state mutations:** `setRequestedOrientation(...)` and window cutout flags (`LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES`) are strictly scoped to the `Game` Activity window and automatically torn down by Android if the app exits or crashes. No external recovery journal is required.

## 5. Settings & 10/10 Craft Plan

- **Fewer-settings rule (ZERO new preference keys!):**
  - Reuses existing `stream_orientation` (adds `"Landscape (either side)"` and `"Reverse landscape"` choices) and existing `video_scaling` (adds `"Safe fit"` choice).
  - **Zero** new entries in `preferences.xml`, **zero** changes to `SettingsKeysTest.java` key lists, **zero** retired keys reused!
  - Updates the two-sentence help text in `client/scripts/generate-settings.py` (regenerating `client/docs/settings.md`):
    - `stream_orientation`: *"Locks or flips the stream's screen orientation, including reverse landscape for USB-C controllers. Changing rotation during a stream only reconfigures the PC when the screen shape changes; choose Landscape (either side) or Reverse landscape if your controller plugs into the opposite side."*
    - `video_scaling`: *"Chooses how the picture fits your screen, including Safe fit to keep the entire game clear of camera cutouts and rounded corners. Safe fit adds small borders beside a cutout while keeping touch input aligned; choose Fill if you prefer edge-to-edge video."*
- **Plain-English error / notice states:**
  - When tapping *Flip 180°* in split-screen or on an external display: *"Screen flip isn't available in windowed or external-display mode."* (never silent no-op).
- **Dynamic input glyphs:** The in-stream *Flip 180°* quick action and Stream Menu rows inherit Nightjar's dynamic Xbox/PS/Nintendo/Keyboard glyph prompts; hidden automatically on Android TV (`UI_MODE_TYPE_TELEVISION`) and external presentations where flipping a TV makes no sense.
- **Zero stream-time blur / zero extra layers:** Verified by inspection and view-hierarchy test: `StreamView`, `GameOverlays`, and `PadControlView` apply bounds/margins directly — zero `setRenderEffect`, zero `LAYER_TYPE_HARDWARE`, zero alpha-composited blur surfaces over the `SurfaceView`.
- **Zero `"Experimental"`, zero mod names, 15 locales:** New option labels (*Landscape (either side)*, *Reverse landscape*, *Safe fit*, *Flip screen 180°*) translated in all 15 Android `values*/strings.xml` files (and `client/protocol/desktop/src/strings/*.rs` only if any shared string is referenced in desktop tests).

## 6. Disjoint Implementer Slices

| Slice | Owner Scope | Exclusive File List |
| --- | --- | --- |
| **Slice 12-A: Shared Safe-Area Policy (`nightjar-policy`)** *(Stage 1, parallel with 12-D)* | Pure Rust safe-area geometry solver, rounded-corner chord math, fold pane logic, shared JSON trace fixtures | `client/protocol/policy/src/safe_area.rs` (new), `client/protocol/policy/src/lib.rs`, `client/protocol/policy/tests/traces/safe_area.json` (new), `client/protocol/policy/tests/traces.rs` |
| **Slice 12-B: Android Orientation, Scaling & Java Policy Mirror** *(Stage 2, after 12-A + 12-D)* | `StreamOrientation` reverse/sensor-landscape modes, `VideoScaling.SAFE_FIT`, `SafeAreaPolicy.java` (replay-verified against Slice 12-A traces), unit tests | `client/app/src/main/java/com/limelight/ui/SafeAreaPolicy.java` (new), `client/app/src/main/java/com/limelight/ui/StreamOrientation.java`, `client/app/src/main/java/com/limelight/ui/VideoScaling.java`, `client/app/src/main/java/com/limelight/ui/StreamWindow.java`, `client/app/src/test/java/com/limelight/ui/SafeAreaPolicyTest.java` (new), `client/app/src/test/java/com/limelight/ui/StreamOrientationTest.java` (new) |
| **Slice 12-C: Android Activity, Layout, Overlay & OSC Integration** *(Stage 3, after 12-B)* | `StreamSafeAreaController` (`DisplayListener` for 180° flips), `AdaptiveLayout`, `GameVideoOutput`, `GameOverlays`, `PadControlView`, `StreamPresentation`, `StreamMenu` Flip-180° action | `client/app/src/main/java/com/limelight/ui/StreamSafeAreaController.java` (new), `client/app/src/main/java/com/limelight/ui/AdaptiveLayout.java`, `client/app/src/main/java/com/limelight/ui/StreamPresentation.java`, `client/app/src/main/java/com/limelight/GameVideoOutput.java`, `client/app/src/main/java/com/limelight/GameOverlays.java`, `client/app/src/main/java/com/limelight/binding/input/pad/PadControlView.java`, `client/app/src/main/java/com/limelight/ui/StreamMenu.java`, `client/app/src/test/java/com/limelight/ui/StreamSafeAreaControllerTest.java` (new) |
| **Slice 12-D: Resources, 15 Locales & Settings Docs** *(Stage 1, parallel with 12-A)* | XML option arrays (`stream_orientation_values`/`names`, `video_scaling_values`/`names`), all 15 `values*/strings.xml`, `generate-settings.py` & `docs/settings.md` | `client/app/src/main/res/values/arrays.xml`, `client/app/src/main/res/values*/strings.xml`, `client/scripts/generate-settings.py`, `client/docs/settings.md` |

## 7. Verification & Measurement Plan

### Narrowest Unit & Trace Tests (Zero GPU, Zero Host Changes)

1. **Shared Policy & Cross-Language Parity:**
   - `cargo test -p nightjar-policy --locked safe_area` + `cargo test -p nightjar-policy --locked --test traces` — validates all 4 rotations (`0/90/180/270`), asymmetric left vs. right hole-punch cutouts, API 31+ rounded corners, bottom gesture bar, flat vs. half-opened foldables, split-screen, PiP, external `StreamPresentation`, and pathological inset clamping.
   - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.ui.SafeAreaPolicyTest` — replays `client/protocol/policy/tests/traces/safe_area.json` through `SafeAreaPolicy.java` and asserts **exact pixel-for-pixel equality** with Rust `nightjar-policy::safe_area`.
2. **Orientation, Flip 180° & Reconfigure Discipline:**
   - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.ui.StreamOrientationTest --tests com.limelight.ui.StreamSafeAreaControllerTest`:
     - `LANDSCAPE_FLIP` returns `ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE`; `REVERSE_LANDSCAPE` returns `ActivityInfo.SCREEN_ORIENTATION_REVERSE_LANDSCAPE`.
     - Simulated `ROTATION_90 → ROTATION_270` flip (WITHOUT `onConfigurationChanged`) updates `SafeAreaLayout`, mirrors cutout margin from left to right, shifts performance overlay & OSC bounds, preserves `VideoScaling.toStream` accuracy, and asserts **0 calls** to `0x5532` reconfigure!
     - Toggling `FILL → SAFE_FIT` on a virtual-display host with `follow_screen` emits exactly 1 debounced `0x5532` reconfigure with cutout-free dimensions aligned via `StreamResizePolicy.alignDown`.
     - Switching to `StreamPresentation` clears phone cutout insets on external `StreamView`.
3. **Settings & Architecture Guardrails:**
   - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.preferences.SettingsKeysTest` — proves zero retired keys touched and legacy `checkbox_stretch_video` compatibility preserved.
   - `node client/scripts/check-doc-links.mjs` & `python client/scripts/generate-settings.py` (clean diff on `client/docs/settings.md`).
   - `python tools/arch_check.py` — proves zero ratchet growth in `Game.java`, `GameVideoOutput.java`, `PadControlView.java`, `StreamMenu.java`, `PreferenceConfiguration.java`, `StreamSettings.java`.

### Visual Screenshot Checks

- **Android Emulator (`client/scripts/emulator-smoke.py`) & Robolectric Layout Captures:**
  1. Phone with left punch-hole (`ROTATION_90`): capture `Safe-Fit`, `Fit`, and `Fill` with Performance Overlay ON, Stream Menu open, and On-Screen Controller visible — verify zero overlap with cutout, rounded corners, or gesture bar.
  2. Same phone flipped 180° (`ROTATION_270`, Reverse Landscape): capture identical views — verify cutout clearance mirrors cleanly to the right side, overlay/OSC stay clear, and zero blur surfaces exist in the view hierarchy.
  3. Foldable half-opened (tabletop hinge) and split-screen window: verify video stays in primary pane without clipping.

### Serialized Idle-Gated Hardware Criteria

- Client-only change: zero host binary modifications (`PROTOCOL_VERSION` and host pipeline untouched).
- Optional end-to-end validation on physical Android phone (with USB-C telescopic controller in Reverse Landscape) streaming from an isolated idle Rubylight test host:
  1. Confirm `(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` shows `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0`.
  2. Stream from isolated test host (`butterpollo.exe --config-dir C:\tests\safe-area --port 48123 --bind 127.0.0.1`), rotate phone 180° (`ROTATION_90 ↔ ROTATION_270`), verify 0 dropped frames, 0 spurious IDRs in host log, and zero added client present latency (`display_p95_ms` delta <= +0.1 ms).
  3. **Exit code 3 abort rule:** If any companion `e2e.py` or `check.ps1` run exits with code `3` (AMD encoder hang), stop immediately and investigate — never retry.
