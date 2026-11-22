# Item 09: Host Hub Sync (Profiles, Covers & Capabilities)

Status: design, not implemented (Wave 2 — Rubylight & Nightjar 7.0.0). Written against `main` `b3d0c26b9b` (`6.2.0`), `docs/designs/00-protocol-reservations.md`, and `tools/size_ratchet.json`.

## 1. Summary & User Experience

A player who pairs multiple Nightjar devices to the same Rubylight PC — a phone for travel, a handheld for the couch, a living-room TV, and a desktop preview client — currently manages stream profiles, cover art downloads, input deck layouts, and host capability state separately on each device. Changing a box-art cover on the host causes every device to re-fetch blindly; tuning a handheld's Wi-Fi vs. cellular profile does not travel to a freshly reinstalled client; and when the host PC is asleep or unreachable, a new or cleared client loses capability visibility until the next online poll.

**Host Hub Sync** turns the player's own Rubylight PC into a zero-cloud, LAN- and Internet-Play-reachable sync hub over additive Native API v1 (`hub.sync`):

- **Consistent, ETag/SHA-256 cached covers:** The host computes a deterministic SHA-256 content digest for every app cover it serves and publishes the manifest in `GET /hub/snapshot`. Clients compare the digest against their local disk cache (`store.art_path` on desktop, `ArtCache` on Android) and fetch `GET /hub/covers/{app_id}` only when the digest changed, using `If-None-Match` (`304 Not Modified`) to eliminate redundant PNG/JPEG/WebP transfers.
- **Device, network & layered controller profiles:** Each paired device stores its named stream profiles (for example, *Home Wi-Fi 4K*, *Cellular 720p*, *Docked TV*) and its layered **Visual Controller Editor** profiles (`Global → Per-Controller-Model → Per-Game` — dead zones, response curves, trigger actuation, DualSense adaptive trigger feel, back paddles `L4/R4/L5/R5`, gyro studio config, trackpad modes, 4–12 slice radial menus, haptics/lightbar, and Modern Touch Controller layouts #151, consumed by Item 13 `client-input-deck`) on the host. All profiles carry optimistic revision numbers (`revision` / `If-Match`) and support portable JSON import/export, so concurrent edits never silently overwrite one another.
- **Read-only host override visibility:** The snapshot surfaces the effective host/app/device override summary from `rust/core/src/device_overrides.rs` (`app × device → app → device → host`), letting Nightjar explain clearly when a host-configured override takes precedence rather than letting local profile edits appear broken.
- **Offline-first local cache:** Every paired Nightjar device caches the last verified hub snapshot and SHA-256 covers locally. While the PC is asleep or away, browsing the library, inspecting profiles, and queuing edits all work offline; once the host returns (LAN or `rubylight-traverse` Internet Play), the outbox reconciles automatically.
- **Strict zero-cloud, per-device TLS scoping:** Nothing leaves the player's own PC and paired devices. Every request authenticates by the device's pinned mTLS client certificate (`caller(&h, &c)`) and enforces per-device UUID isolation: a device may only mutate its own profiles unless it holds `host_control`.

## 2. Protocol, Native API & Capability Contract

### Compatibility Invariants

1. **`PROTOCOL_VERSION` is unchanged.** Classic GameStream/Moonlight endpoints (`/serverinfo`, `/applist`, `/appasset`, `/launch`, `/resume`) and stock Moonlight clients remain byte-for-byte unaffected.
2. **`rubylight_native_api::API_VERSION` remains `1`.** All types and routes are strictly additive under `/api/native/v1`: response structs use `#[serde(default)]` for new fields, request structs use `#[serde(deny_unknown_fields)]`, and enums include `#[serde(other)] Unknown`.
3. **Zero new control-stream message IDs or RTSP `DESCRIBE` lines.** Sync is purely HTTPS over the existing pinned streaming port (`47984` / configured HTTPS port).
4. **Capability-gated, never brand-gated.** Nightjar gates every Hub Sync call and UI indicator strictly on `HostCapabilities::has(Feature::HubSync)` (Rust `client/protocol/client/src/http/capabilities.rs` and Java `HostCapabilities.java`), proven solely by `hub.sync` in `GET /api/native/v1/capabilities`. Never check `Family::Rubylight` or version strings (`tools/arch_check.py` `rubylight-family-gating`).

### Feature & Capability Mapping

| Layer | Symbol | Wire Value / Proof |
| --- | --- | --- |
| `rubylight-native-api` | `feature::HUB_SYNC` | `"hub.sync"` (always listed when host supports Hub Sync; callers still check `permissions.list` / `permissions.launch` / `permissions.host_control`) |
| `nightjar-core` (Rust) | `Feature::HubSync` | Proven by `hub.sync` in `HostCapabilities::with_native` |
| `nightjar-android` (Java) | `HostCapabilities.Feature.HUB_SYNC` | Proven by `hub.sync` in `HostCapabilities.withNativeCapabilities` |
| `rubylight-native-api` error | `ErrorCode::RevisionConflict` | `"revision_conflict"` → HTTP `409 Conflict` (older clients decode as `ErrorCode::Unknown` and branch on status `409`) |
| `rubylight-native-api` error | `ErrorCode::TooLarge` | `"too_large"` → HTTP `413 Payload Too Large` (existing error code; returned when `PUT /api/native/v1/hub/profiles/{id}` exceeds 64 KiB) |

### Native API v1 Routes (`/api/native/v1`)

| Method & Path | Permission | Headers | Purpose |
| --- | --- | --- | --- |
| `GET /hub/snapshot` | `list` (`LIST_APPS`) | Optional `If-None-Match: "<snapshot_rev>"` | Full or conditional (`304 Not Modified`) hub state: capabilities summary, cover manifest, caller's profiles (plus shared profiles; `?all_devices=1` requires `host_control`), input deck index, effective host override summary |
| `PUT /hub/profiles/{id}` | `launch` (own device) or `host_control` (shared / other device) | Required `If-Match: "<rev>"` (`"0"` for create) | Create or update one profile atomically with optimistic revision check; attaches explicit per-route `.layer(DefaultBodyLimit::disable())` + `axum::body::to_bytes(req.into_body(), 64 * 1024)` in `rust/host/src/native_api.rs` (overriding the router-wide 16 KiB `DefaultBodyLimit::max(16 * 1024)`) and returns `ErrorCode::TooLarge` (`"too_large"`, HTTP `413`) above 64 KiB; returns updated profile + `ETag: "<new_rev>"` |
| `DELETE /hub/profiles/{id}` | `launch` (own device) or `host_control` (shared / other device) | Required `If-Match: "<rev>"` | Delete one profile with optimistic revision check; returns `200 OK` with `{ "deleted": true, "snapshot_revision": N }` |
| `GET /hub/covers/{app_id}` | `list` (`LIST_APPS`) | Optional `If-None-Match: "sha256:<hex>"` | Binary cover image (`image/png`, `image/jpeg`, `image/webp`) with `ETag: "sha256:<hex>"` and `Cache-Control: private, max-age=0, must-revalidate`; returns `304 Not Modified` on ETag match |

### Identity Scoping & Permission Matrix

Caller resolution reuses `caller(&h, &c)` in `rust/host/src/native_api.rs` and normalizes dual-screen identities with `butterpollo_core::state::device_uuid(&client.uuid)` (stripping `SECOND_SCREEN_SUFFIX = "#screen2"`).

| Action | Caller owns `owner_device_uuid` + `launch` | Caller owns `owner_device_uuid`, `list` only | Another device's profile (no `host_control`) | `shared` profile (no `host_control`) | Any profile WITH `host_control` |
| --- | --- | --- | --- | --- | --- |
| `GET /hub/snapshot` | Own + shared profiles | Own + shared profiles (read-only) | Never returned | Returned (read-only) | Own + shared; `?all_devices=1` returns all |
| `PUT /hub/profiles/{id}` (own) | `200 OK` / `201 Created` | `403 permission_required` | `403 host_control_required` | `403 host_control_required` | `200 OK` / `201 Created` |
| `DELETE /hub/profiles/{id}` | `200 OK` | `403 permission_required` | `403 host_control_required` | `403 host_control_required` | `200 OK` |
| `GET /hub/covers/{app_id}` | `200` / `304` | `200` / `304` | N/A (app-scoped) | N/A (app-scoped) | `200` / `304` |

### Wire Schemas

#### `GET /api/native/v1/hub/snapshot` (`200 OK`, `ETag: "rev:42"`)

```json
{
  "schema_version": 1,
  "snapshot_revision": 42,
  "host_unique_id": "a1b2c3d4-...",
  "generated_unix_ms": 1795221800000,
  "caller_device_uuid": "dev-handheld-01",
  "capabilities_snapshot": {
    "api_version": 1,
    "features": ["library", "session", "hub.sync", "stream.doctor_v2", "input.deck"],
    "permissions": {
      "list": true,
      "view": true,
      "launch": true,
      "host_control": false
    },
    "codecs": ["h264", "hevc", "hevc10", "av1", "av1_10"],
    "virtual_display": true,
    "hdr": true
  },
  "covers": [
    {
      "app_id": 1042,
      "app_uuid": "app-uuid-1042",
      "etag": "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
      "bytes": 184320,
      "width": 600,
      "height": 900,
      "mime": "image/png",
      "updated_unix_ms": 1795200000000
    }
  ],
  "profiles": [
    {
      "id": "handheld-lan-90",
      "revision": 7,
      "scope": "device",
      "owner_device_uuid": "dev-handheld-01",
      "name": "Home Wi-Fi 90 Hz",
      "network_matcher": { "kind": "lan" },
      "stream": {
        "width": 1280,
        "height": 800,
        "fps": 90,
        "bitrate_kbps": 35000,
        "codec": "hevc",
        "hdr": true,
        "audio_layout": "stereo",
        "upscaling": "sgsr1",
        "video_scaling": "safe_fit",
        "orientation": "landscape_flip"
      },
      "input_deck_id": "deck-fps-default",
      "updated_unix_ms": 1795221100000,
      "updated_by_device": "dev-handheld-01"
    }
  ],
  "input_decks": [
    {
      "id": "deck-fps-default",
      "layer_scope": "per_game",
      "controller_model": "dualsense",
      "app_id": 1042,
      "revision": 3,
      "scope": "device",
      "owner_device_uuid": "dev-handheld-01",
      "name": "FPS DualSense + Gyro",
      "content_sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "updated_unix_ms": 1795220500000
    }
  ],
  "host_overrides_summary": [
    {
      "app_id": 1042,
      "source": "app_device",
      "locked_keys": ["fps_limit", "hdr"]
    }
  ]
}
```

#### `PUT /api/native/v1/hub/profiles/{id}` (`If-Match: "7"`, `#[serde(deny_unknown_fields)]`)

- `{id}` validation: ASCII `[a-z0-9][a-z0-9_-]{0,63}`; rejects path separators, `#`, whitespace, or empty IDs with `400 invalid_request`.
- Size & quota caps: in `rust/host/src/native_api.rs`, attach an explicit per-route `.layer(DefaultBodyLimit::disable())` + `axum::body::to_bytes(req.into_body(), 64 * 1024)` override on `PUT /api/native/v1/hub/profiles/{id}` (overriding the router-wide 16 KiB `DefaultBodyLimit::max(16 * 1024)`), and enforce the 64 KiB body cap in `rust/host/src/native_api/hub.rs` returning `ErrorCode::TooLarge` (`"too_large"`, HTTP `413`) above 64 KiB; max `64` profiles per device, max `256` profiles host-wide (`422 invalid_value` with plain-English limit message).
- `upscaling` validation on wire: accepts `"off" | "bilinear" | "fsr1" | "sgsr1" | "metalfx" | null` (`"sgsr2"` is mapped to `"sgsr1"` on read from legacy clients and rejected on new writes if unknown).
- **Conflict response (`409 Conflict`, `code: "revision_conflict"`):** Returns standard `ErrorBody` PLUS a sibling `hub_conflict` response header `X-Hub-Current-Revision: <rev>` (and optional `conflict` extension or immediate client re-fetch of `GET /hub/snapshot`) so clients know the exact server revision without changing `ErrorBody`'s backwards-compatible shape.

```json
{
  "error": {
    "code": "revision_conflict",
    "message": "This profile was updated on another device (revision 8). Review the latest version before saving.",
    "setting": "handheld-lan-90",
    "retry_after_ms": null
  }
}
```

## 3. Architecture & <=800-LOC Module Plan

### Size-Ratchet Guardrail (`tools/size_ratchet.json`)

The following touched files are grandfathered above 800 LOC and **MUST NOT grow** beyond their recorded ceilings; all new logic goes into dedicated `<= 800`-LOC modules with required `//!` (Rust) or `/**` (Java) module headers (`tools/arch_check.py`):

| Grandfathered File | Ratchet Ceiling | Strategy to Avoid Growth |
| --- | --- | --- |
| `rust/host/src/native_api.rs` `[verify post-sprint code-map.md]` | 853 LOC | Add `mod hub;`, 4 route registrations in `router()` (attaching explicit per-route `.layer(DefaultBodyLimit::disable())` + `axum::body::to_bytes(req.into_body(), 64 * 1024)` on `PUT /api/native/v1/hub/profiles/{id}` to override the router-wide 16 KiB `DefaultBodyLimit::max(16 * 1024)`), and 1 feature push; keep all handler & 64 KiB `ErrorCode::TooLarge` (`"too_large"`, HTTP `413`) logic in `rust/host/src/native_api/hub.rs`. |
| `rust/host/src/state.rs` `[verify post-sprint code-map.md]` | 2247 LOC | Add 1 field `pub hub: Arc<crate::hub_store::HubStore>` and initialize in `Host::load`; zero methods added to `state.rs`. |
| `rust/host/src/nvhttp/native_tests.rs` `[verify post-sprint code-map.md]` | 1036 LOC | Add `mod hub;` only; all Hub Sync route & concurrency tests live in new `rust/host/src/nvhttp/native_tests/hub.rs`. |
| `client/protocol/client/src/http/native.rs` `[verify post-sprint code-map.md]` | 821 LOC | Keep `native.rs` unchanged or 1-line `pub mod hub;` re-export; put all HTTP methods in `client/protocol/client/src/http/hub.rs`. |
| `client/protocol/client/src/http/capabilities.rs` `[verify post-sprint code-map.md]` | 1257 LOC | Minimal additive diff (~6 lines for `Feature::HubSync` + `with_native`); no inline tests added here (put contract fixtures in `capabilities_contract.rs`). |
| `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java` `[verify post-sprint code-map.md]` | 1347 LOC | Minimal additive diff (~6 lines for `HUB_SYNC` + `withNativeCapabilities`). |
| `client/protocol/desktop/src/hosts.rs` `[verify post-sprint code-map.md]` | 1648 LOC | Delegate hub polling and outbox flush to new `client/protocol/desktop/src/hosts/hub_sync.rs`. |
| `client/protocol/desktop/src/config.rs` `[verify post-sprint code-map.md]` | 2639 LOC | Keep synced profile projection in new `client/protocol/desktop/src/hosts/hub_cache.rs`; do not expand `config.rs`. |
| `client/protocol/desktop/src/strings/*.rs` `[verify post-sprint code-map.md]` | 1750 LOC | **Zero growth!** Route all new 7.0.0 Hub Sync Desktop translations to `client/protocol/desktop/src/strings_70/<code>.rs` (14 non-English locale tables; English literals live at call sites). |
| `client/app/src/main/java/com/limelight/computers/ComputerManagerService.java` `[verify post-sprint code-map.md]` | 1573 LOC | Invoke `HubSyncController.onHostOnline(...)` via 3-line hook; all logic in `client/app/src/main/java/com/limelight/nativeapi/HubSyncController.java`. |
| `client/app/src/main/java/com/limelight/preferences/HostStreamSettings.java` `[verify post-sprint code-map.md]` | 1723 LOC | Open a dedicated `HubProfileSheet.java` helper instead of embedding profile conflict UI inline. |

### New & Touched Module Breakdown

| Path | Layer | Est. LOC | Responsibility |
| --- | --- | --- | --- |
| `rust/core/native-api/src/hub.rs` (new) | Wire types | ~240 | `HubSnapshot`, `CoverEntry`, `SyncProfile`, `StreamProfileSpec`, `InputDeckSummary`, `HostOverrideSummary`, `ProfileWrite`, `ProfileDeleteResult` |
| `rust/core/native-api/src/lib.rs`, `error.rs` `[verify post-sprint code-map.md]` | Wire types | +15 | Export `pub mod hub;`, `feature::HUB_SYNC`, `ErrorCode::RevisionConflict` (HTTP 409) |
| `rust/core/src/hub_sync.rs` (new) | Host core policy | ~420 | Pure state store model (`HubState`), ID/quota/schema validation, `If-Match` revision CAS, device UUID scoping (`device_uuid`), cover digest index, `device_overrides::for_device` locked-key projection |
| `rust/host/src/hub_store.rs` (new) | Host persistence | ~360 | `HubStore` wrapping `Mutex<HubState>`, atomic persistence (`hub-sync.json` via `state::write_json`), lazy SHA-256 cover hashing with `(mtime, len)` file-stamp cache (never re-hashing unchanged box art!) |
| `rust/host/src/native_api/hub.rs` (new) | Host HTTP | ~380 | Axum handlers for `GET /hub/snapshot`, `PUT/DELETE /hub/profiles/{id}` (reading up to 64 KiB under per-route `.layer(DefaultBodyLimit::disable())` + `axum::body::to_bytes(req.into_body(), 64 * 1024)` and returning `ErrorCode::TooLarge` / `"too_large"` / HTTP `413` above 64 KiB), `GET /hub/covers/{app_id}` (streaming file bytes with ETag + `304`, never blocking tokio runtime via `spawn_blocking`) |
| `client/protocol/policy/src/hub_sync.rs` (new) | Shared client policy | ~420 | Offline-first reconciliation state machine (`SyncEngine`), 3-way profile merge (`Base + Local + Remote → Merged | Conflict`), cover diff planner (`covers_to_fetch`), outbox compaction |
| `client/protocol/client/src/http/hub.rs` (new) | Client HTTP | ~300 | `HostClient::hub_snapshot(if_none_match)`, `put_hub_profile`, `delete_hub_profile`, `hub_cover(app_id, if_none_match)` |
| `client/protocol/desktop/src/hosts/hub_sync.rs` (new) | Desktop client | ~380 | Worker-lane sync driver (`Lane::Asked`), atomic disk cache (`hub/<host_id>.json` + `outbox.json`), cover pipeline handoff to `hosts/covers.rs` |
| `client/protocol/desktop/src/ui/hub_conflict.rs` (new) | Desktop UI | ~220 | Plain-English conflict resolution dialog (*Keep this device's changes* / *Use PC's version* / *Save as copy*) |
| `client/protocol/desktop/src/strings_70/<code>.rs` (14 locale files) | Desktop i18n | ~30/locale | 7.0.0 Desktop translations for Hub Sync & conflict UI across all 14 non-English locales (`de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`), keeping grandfathered `strings/*.rs` <= 1,750 LOC |
| `client/app/src/main/java/com/limelight/nativeapi/HubSyncController.java` (new) | Android client | ~360 | Background sync executor, disk cache (`hub_<uniqueId>.json`), outbox replay, ETag cover fetcher integrating with existing `ArtCache` |
| `client/app/src/main/java/com/limelight/ui/HubProfileSheet.java` (new) | Android UI | ~260 | Synced profile picker, host-override badges, conflict chooser (zero blur, Material surface matching Nightjar) |
| `client/protocol/android/src/hub_sync.rs` (new) | Android JNI | ~200 | JNI bridge exposing `nightjar_policy::hub_sync` 3-way merge & `nightjar_core::http::hub` calls to Java |

### Platform & Backend Preservation

- **Zero stream-path involvement:** Hub Sync runs exclusively on the launcher/background HTTPS lane (`Lane::Asked` on desktop, background executor on Android). It adds **0.00 ms** to capture, D3D12/D3D11 conversion, AMD AMF, NVIDIA NVENC, PyroWave, packetization, or decode.
- **Windows 10, physical displays, WGC, Desktop Duplication, D3D11:** Completely untouched; `hub-sync.json` lives beside `apps.json` and `state.json` in `C:\ProgramData\Butterpollo\config\` (or isolated `--config-dir`).
- **Desktop cross-OS:** `hosts/hub_sync.rs` uses pure Rust `std::fs` + `nightjar_core::http` (no `cfg(unix)` needed; compiles identically on Windows, Linux, and macOS).

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### Client Sync & Outbox State Machine

```
[ColdStart / OfflineCacheLoaded]
       │ (host online + Feature::HubSync)
       ▼
[RefreshingSnapshot] ──(304 Not Modified & outbox empty)──► [Synced]
       │ (200 OK new snapshot)                                 ▲
       ▼                                                       │
[ReconcilingOutbox] ──(each pending op: PUT/DELETE If-Match)───┘
       │ (409 RevisionConflict)
       ▼
[ConflictPending] ──(user chooses KeepMine / UseHost / SaveCopy)──► [ReconcilingOutbox]
       │ (network error / host offline / 5xx)
       ▼
[OfflineDegraded] (serves cached snapshot & cached SHA-256 covers; retains outbox)
```

### Reconnect & Edge-Case Matrix

| Scenario | Expected Behavior |
| --- | --- |
| **Host offline / sleeping** | Client boots from cached `hub/<host_id>.json`; covers render from SHA-256 disk cache; profile edits append to local `outbox.json` and show a subtle "Saved locally — will sync when PC is online" badge. |
| **Paired device revoked or `launch` permission stripped** | Next `PUT`/`DELETE` returns `403 not_paired` or `403 permission_required`; client pauses outbox replay, preserves local draft, and surfaces a plain-English error without retry-storming. |
| **Dual-screen (`#screen2`) client request** | Host maps `dev-uuid#screen2` → `dev-uuid` via `state::device_uuid()`, sharing the parent device's profile ownership without cross-device leakage. |
| **Concurrent edit from phone & desktop** | Second writer's `If-Match: "7"` hits server revision `8` → `409 revision_conflict`. Client runs `nightjar_policy::hub_sync::merge_three_way(base, local, remote)`: disjoint field edits auto-merge and retry once with `If-Match: "8"`; same-field collisions enter `ConflictPending` for explicit user choice. |
| **Host cover updated in SteamGridDB / console** | Cover file's `(mtime, len)` changes → `HubStore` lazily recomputes SHA-256 and bumps `snapshot_revision`. Next `GET /hub/snapshot` returns new `etag`; clients fetch only that single `app_id` and evict the stale file. |
| **Older Rubylight / Sunshine / Apollo host** | `HostCapabilities::has(Feature::HubSync)` is `false`; Nightjar skips `/hub/*` entirely and uses existing local profiles and `/appasset` covers with zero error toasts. |
| **Host restarts mid-write** | `HubStore` writes `hub-sync.json` through `butterpollo_core::state::write_json` (temp file + flush + atomic rename). Partially written temp files are ignored on boot; `snapshot_revision` never goes backward. |
| **Device unpaired in console** | `HubStore` retains orphaned device profiles until explicit cleanup or host pruning of unpaired UUIDs after 30 days, and never serves them to any other non-`host_control` device. |
| **Internet Play (`rubylight-traverse`)** | Same pinned HTTPS tunnel; `If-None-Match` (`304`) avoids re-sending multi-MB snapshots/covers over cellular links. |

### Persistence & Crash-Restore Journal

- **Host durable file:** `<profile_dir>/hub-sync.json` (versioned `{ "version": 1, "snapshot_revision": u64, "profiles": [...], "input_decks": [...] }`), written exclusively via `butterpollo_core::state::write_json` (atomic temp-file + `write_through` rename).
- **Corrupt-file recovery:** If `hub-sync.json` fails to parse on startup, `HubStore` renames it to `hub-sync.json.corrupt-<unix_ts>`, records a visible host warning in `host_warnings.rs` ("Synced device profiles could not be read and were backed up"), and initializes an empty revision-1 store so streaming is never blocked.
- **Client durable files:** `<cache_dir>/hub/<host_unique_id>/snapshot.json` and `outbox.json`, both written via atomic write-and-rename; cover blobs stored at `<cache_dir>/hub/covers/<sha256_prefix>/<sha256>.img` (content-addressed, inherently crash-safe because a blob is renamed to its digest only after full download + SHA-256 verification).

## 5. Settings & 10/10 Craft Plan

- **Fewer-settings rule (zero new toggles):** Hub Sync is **automatic and always-on** whenever the host advertises `hub.sync` — it adds zero preferences to `PreferenceConfiguration.java`, `preferences.xml`, `config.rs`, or `SettingsKeysTest.java` (zero new keys, zero migrations, zero retired-key risk). It is a safe, strictly additive metadata sync with zero stream-path impact, so no default-off experimental gate is needed.
- **Plain-English error & status copy:**
  - Sync conflict: *"This profile changed on another device. Choose which version to keep."* (Actions: **Keep mine**, **Use PC's**, **Save as new**.)
  - Permission denied: *"This device can view profiles from this PC, but needs Launch permission in the host console to save changes."*
  - Offline: *"PC is offline. Changes are saved on this device and will sync automatically when the PC reconnects."*
  - Host override badge: *"Overridden on PC for this game"* (tapping opens a plain explanation of `app × device → app → device → host`).
- **Dynamic input glyphs:** Conflict dialog and profile picker on Android TV, handhelds, and desktop bind confirm/back/delete hints dynamically to the active controller family (Xbox A/B, PlayStation Cross/Circle, Nintendo B/A, or keyboard Enter/Esc) via existing glyph helpers.
- **Zero stream-time blur / zero `"Experimental"` / zero mod names:** Hub Sync UI runs only outside streams (or in the non-blurred flat sheet surface); never uses the word `"Experimental"`; never references any third-party co-op or frame-gen mod name anywhere in strings or JSON.
- **15-locale coverage:** Every new Nightjar Desktop string keeps its English literal at the call site (`tr`/`trf`/`en`) and routes all 14 translated tables (`de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`) to `client/protocol/desktop/src/strings_70/<code>.rs` (never growing grandfathered `client/protocol/desktop/src/strings/*.rs` past 1,750 LOC), alongside all 15 Android `values*/strings.xml` files. Host console warnings/docs use plain English matching `rust/assets/locale/`.

## 6. Disjoint Implementer Slices

All slices below have **strictly non-overlapping file lists** for Phase 3 `parallel()` execution (Slice B depends on Slice A's wire types; Slices C and D run in parallel once A & B land):

| Slice | Owner Scope | Exclusive File List |
| --- | --- | --- |
| **Slice 09-A: Wire, Lockstep Capabilities & Core Policy** *(Stage 1)* | Native API types + host core hub model + shared client reconciliation policy + all 4 lockstep `HostCapabilities` contract files | `rust/core/native-api/src/hub.rs` (new), `rust/core/native-api/src/lib.rs`, `rust/core/native-api/src/error.rs`, `rust/core/native-api/tests/fixtures/hub_snapshot.json` (new), `rust/core/native-api/tests/fixtures.rs`, `rust/core/src/hub_sync.rs` (new), `rust/core/src/lib.rs`, `client/protocol/policy/src/hub_sync.rs` (new), `client/protocol/policy/src/lib.rs`, `client/protocol/policy/tests/traces/hub_sync.json` (new), `client/protocol/client/src/http/capabilities.rs`, `client/protocol/client/tests/capabilities_contract.rs`, `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java`, `client/app/src/test/resources/protocol/capabilities/*` |
| **Slice 09-B: Host Persistence & HTTP Routes** *(Stage 2, parallel with 09-C/09-D)* | Atomic host store, lazy SHA-256 cover indexer, Axum handlers (with `.layer(DefaultBodyLimit::disable())` + `axum::body::to_bytes(req.into_body(), 64 * 1024)` on `PUT /api/native/v1/hub/profiles/{id}` so `> 64 KiB` returns JSON `ErrorBody` `ErrorCode::TooLarge` HTTP `413`), route tests, docs | `rust/host/src/hub_store.rs` (new), `rust/host/src/native_api/hub.rs` (new), `rust/host/src/native_api.rs`, `rust/host/src/state.rs`, `rust/host/src/nvhttp/native_tests/hub.rs` (new), `rust/host/src/nvhttp/native_tests.rs`, `docs/native-api.md` |
| **Slice 09-C: Nightjar Rust Core & Desktop** *(Stage 2, parallel with 09-B/09-D)* | `nightjar-core` HTTP client, desktop worker, cache, conflict UI, 15 locales | `client/protocol/client/src/http/hub.rs` (new), `client/protocol/client/src/http/native.rs`, `client/protocol/desktop/src/hosts/hub_sync.rs` (new), `client/protocol/desktop/src/hosts/hub_cache.rs` (new), `client/protocol/desktop/src/hosts.rs`, `client/protocol/desktop/src/ui/hub_conflict.rs` (new), `client/protocol/desktop/src/strings_70/<code>.rs` (14 non-English locale tables; never growing grandfathered `strings/*.rs` past 1,750 LOC) |
| **Slice 09-D: Nightjar Android** *(Stage 2, parallel with 09-B/09-C)* | JNI bridge, Android sync controller, profile/conflict sheet, 15 `strings.xml` | `client/protocol/android/src/hub_sync.rs` (new), `client/protocol/android/src/lib.rs`, `client/app/src/main/java/com/limelight/nativeapi/HubSyncController.java` (new), `client/app/src/main/java/com/limelight/ui/HubProfileSheet.java` (new), `client/app/src/main/java/com/limelight/computers/ComputerManagerService.java`, `client/app/src/main/res/values*/strings.xml`, `client/app/src/test/java/com/limelight/nativeapi/HubSyncControllerTest.java` (new) |

*(All four lockstep capability contract files land together in Stage 1 `Slice 09-A`, so Stage 2 `09-B`, `09-C`, and `09-D` compile and pass both `capabilities_contract.rs` and `HostCapabilitiesContractTest` in parallel.)*

## 7. Verification & Measurement Plan

### Narrowest Unit, Contract & Interop Tests

1. **Wire & core policy (no GPU, any OS):**
   - `cargo test -p rubylight-native-api --locked` — serde round-trip fixtures, `#[serde(deny_unknown_fields)]` rejection on `ProfileWrite`, `ErrorCode::RevisionConflict` → 409, unknown-field tolerance on `HubSnapshot`.
   - `cargo test -p butterpollo-core --locked hub_sync` — ID validation, per-device quota enforcement, `#screen2` normalization, CAS revision conflicts, `device_overrides` precedence projection.
   - `cargo test -p nightjar-policy --locked hub_sync` — 3-way merge (disjoint auto-merge vs. conflicting field), outbox compaction, cover ETag diffing.
2. **Host HTTP & TLS scoping (GPU-free loopback):**
   - `cargo test -p butterpollo --locked nvhttp::native_tests::hub` — proves:
     - Device A cannot `PUT`/`DELETE` Device B's profile (`403 host_control_required`), nor see Device B's private profiles in `GET /hub/snapshot`.
     - Device with `host_control` can mutate `shared` profiles and read `?all_devices=1`.
     - `If-Match` mismatch returns `409 revision_conflict` without partial writes.
     - `PUT /api/native/v1/hub/profiles/{id}` accepts valid bodies between 16 KiB and 64 KiB (proving the per-route `.layer(DefaultBodyLimit::disable())` + `axum::body::to_bytes(req.into_body(), 64 * 1024)` override over the router-wide 16 KiB `DefaultBodyLimit::max(16 * 1024)`) and rejects bodies above 64 KiB with `ErrorCode::TooLarge` (`"too_large"`, HTTP `413`).
     - `GET /hub/covers/{app_id}` with matching `If-None-Match` returns `304 Not Modified` with zero body bytes; cover change invalidates ETag.
     - Atomic reload across simulated host restart preserves `snapshot_revision`.
     - `PROTOCOL_VERSION` and stock Moonlight `/serverinfo` tests (`extension_tests.rs`, `parity_tests.rs`) remain unchanged.
3. **Nightjar core & Android:**
   - `cargo test -p nightjar-core --locked capabilities_contract` + `hub` loopback tests.
   - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.nativeapi.HubSyncControllerTest --tests com.limelight.preferences.SettingsKeysTest` — offline cache boot, outbox replay on reconnect, zero key regressions.
   - `python tools/arch_check.py` & `python tools/code_map.py check` — verify `<= 800` LOC on every new module, zero ratchet regressions (including grandfathered `client/protocol/desktop/src/strings/*.rs <= 1,750` LOC), zero `Family::Rubylight` gating.

### Visual Screenshot Checks

- **Desktop:** Rasterize `ui/hub_conflict.rs` conflict dialog and synced-profile badge in `client/protocol/desktop/src/ui/tests.rs` + `shot.rs` (light/dark, narrow handheld vs. 16:9 desktop, German long strings).
- **Android:** Robolectric / emulator screenshots of `HubProfileSheet` in phone portrait, phone landscape, and Android TV 10-foot focus state verifying dynamic controller glyphs and zero clipping.

### Hardware & Performance Criteria (Serialized, Idle-Gated)

- Hub Sync touches no GPU or capture path, so no GPU benchmark is required for default-on readiness.
- Before any isolated-host smoke verification on the RX 7900 XT host:
  1. Verify `(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` shows `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0` and the user is idle.
  2. Run isolated test host (`butterpollo.exe --config-dir C:\tests\hub-sync --port 48123 --bind 127.0.0.1`) with two paired test certs; verify `GET /hub/snapshot` p99 < 15 ms for a 200-app library after warm cover-hash cache, and `304` cover hits transfer 0 image bytes.
  3. **Exit code 3 abort rule:** If any companion `e2e.py` or `check.ps1` smoke run exits with code `3` (AMD encoder hang), stop immediately and investigate — never retry blindly.
