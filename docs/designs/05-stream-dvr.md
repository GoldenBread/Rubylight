# Item 05: Host-Side Stream DVR & Instant Replay (`stream-dvr`)

Status: design, not implemented (Wave 1, Rubylight & Nightjar 7.0.0). Rebased 2026-11-25 onto post-refactoring `origin/main` `955ef39598` (`v6.4.1`) and [protocol reservations](00-protocol-reservations.md).

## 1. Summary & User Experience

Host-side Stream DVR gives players console-style **Instant Replay** ("Save last 30–120 seconds") and **Manual Recording** directly on the Rubylight host PC, with **zero re-encoding** and **zero latency added to the streaming pipeline**. Because the host already encodes every video frame (H.264, HEVC, or AV1 via AMD AMF or NVIDIA NVENC) and every audio packet (Opus), Stream DVR taps those already-encoded bitstreams into a bounded RAM ring buffer and remuxes them asynchronously on a dedicated `dvr-mux` worker thread into standard fragmented MP4 (`.mp4`) or Matroska (`.mkv`) files on disk.

### User experience across surfaces

- **In-stream (Nightjar Android & Desktop):**
  - When the host advertises `StreamDvrVersion=1` and Native API feature `stream.dvr` (`HostCapabilities::has(Feature::StreamDvr)`), Nightjar's **Stream Menu** displays a one-press **Save Clip** tile (with a live ring subtitle such as `"Last 60 s ready"` or `"Recording 02:14"`) and a secondary **Start / Stop Recording** toggle.
  - Pressing **Save Clip** sends control message `0x554A` (`DVR_STATE_MESSAGE_TYPE`, `Action::SaveClip`) over the already-open ENet control channel (or `POST /api/native/v1/dvr/clip` when invoked outside the ENet stream, such as WebRTC or companion control). A brief non-blurring corner toast confirms `"Clip saved (60 s, 84 MB)"` once the host's atomic rename completes, or displays a plain-English reason if refused (for example, `"Instant Replay is off in host settings"` or `"Clips are not available with PyroWave"`).
  - Optional controller/keyboard shortcut binding on Nightjar triggers the same `0x554A` `SaveClip` action without opening the Stream Menu, keeping the game uninterrupted.
- **Console 2.0 (Rubylight web UI):**
  - A **Clips** drawer/panel lists saved clips sorted newest-first with app title, date/time, duration, resolution/FPS, codec badge (`H.264` / `HEVC` / `AV1` + `HDR` if 10-bit PQ/HLG), file size, and disk-quota usage bar (`"4.2 GB of 20 GB used"`).
  - Users can download, stream-preview in-browser (for browser-playable fMP4), delete individual clips, or open the clips folder on the host PC.
- **Default-off safety:** Stream DVR ships **off by default** (`dvr_enabled = false`) so an unconfigured host allocates zero ring memory, runs zero tap logic (a single relaxed atomic load returns `false` on the hot path), and spawns no `dvr-mux` thread.
- **Explicit PyroWave exclusion:** PyroWave uses a proprietary wavelet bitstream (`Encoded.framing == Some(...)`, `codec == 3`) that cannot be remuxed into MP4/MKV without transcoding. Rather than silently transcoding or silently dropping clips, a PyroWave session reports DVR `unsupported` with stable wire reason `pyrowave` across `0x554A`, `GET /api/native/v1/dvr`, host logs, and UI.

---

## 2. Protocol, Native API & Capability Contract

All identifiers strictly follow [00-protocol-reservations.md](00-protocol-reservations.md). `PROTOCOL_VERSION` and `rubylight_native_api::API_VERSION` (`1`) remain **unchanged**. Stock Moonlight clients and older Nightjar builds never send `0x554A` or call `/api/native/v1/dvr*` and see zero behavioral change.

### 2.1 Discovery & Capability Gating

| Surface | Field / Constant | Value / Condition |
|---|---|---|
| `/serverinfo` | `StreamDvrVersion` | `1` on 7.0.0+ hosts implementing Stream DVR (emitted in `Offered::serverinfo_fields`; runtime armed/disabled/unsupported state travels in `0x554A` and `GET /dvr`) |
| RTSP `DESCRIBE` | `a=x-rl-control:0x554A` | Listed in `Offered::describe_lines` on streams whose host supports `0x554A` |
| Native API `/capabilities` | `feature::STREAM_DVR` = `"stream.dvr"` | Listed for every paired caller on a 7.0.0+ host; `GET /dvr` reports `state` (`ready`, `disabled`, `unsupported`, `error`) |
| Nightjar Rust | `HostCapabilities::Feature::StreamDvr` | Proven by `StreamDvrVersion >= 1` **and** native `stream.dvr` |
| Nightjar Java | `HostCapabilities.Feature.STREAM_DVR` | Proven by `StreamDvrVersion >= 1` **and** native `stream.dvr` |

> **Design rule on advertising vs. runtime state:** Following `encoder-focus-design.md`, `StreamDvrVersion=1` and `stream.dvr` advertise *host support* for the feature contract; whether instant replay is currently armed or unavailable for the current stream (e.g. `disabled`, `pyrowave`, `disk_full`, `audio_codec_unsupported`) is carried dynamically in `0x554A` status payloads and `GET /api/native/v1/dvr`. Nightjar NEVER gates on `Family::Rubylight` or version strings (`tools/arch_check.py` enforces this).

### 2.2 Control-Stream Message `0x554A` (`DVR_STATE_MESSAGE_TYPE`)

- **ID:** `0x554A` in `RUBYLIGHT_7_CONTROL_IDS` (`0x5548..=0x554F`), bit 15 clear (`& HAPTICS_CAPABILITY == 0`).
- **Direction:** Bidirectional.
- **Delivery:** Reliable ENet control channel (encrypted via standard control-channel AES-GCM; unauthenticated/legacy plaintext control packets are dropped, matching `stream/control.rs::server_command`).
- **Module:** `client/protocol/core/src/stream_dvr.rs` (`no_std`, pure encode/decode + round-trip property tests in `client/protocol/core/src/fuzz.rs`).

#### Wire Format (little-endian, versioned header)

| Offset | Size | Type | Field | Semantics |
|---|---|---|---|---|
| 0 | 1 | `u8` | `version` | `1` (unknown version ignored/rejected cleanly) |
| 1 | 1 | `u8` | `kind` | `1` = `ClientRequest`, `2` = `HostState`, `3` = `ClipResult` |
| 2 | 2 | `u16` | `flags` | Reserved `0` in v1 |
| 4 | 4 | `u32` | `request_id` | Client-chosen monotonic nonce echoed in `ClipResult` / `HostState` (`0` for unsolicited host push) |
| 8 | var | bytes | `body` | Tagged by `kind` below |

- **`kind = 1` (`ClientRequest`, Client -> Host):**
  - `body[0]`: `op` (`1 = QueryState`, `2 = SaveClip`, `3 = StartRecord`, `4 = StopRecord`).
  - `body[1..3]` (`u16` LE): `duration_secs` (optional requested clip window for `SaveClip`; `0` = full configured ring window, clamped by host to `[5, ring_seconds]`).
  - **Host authorization check:** Only the session owner (or a device with `launch` permission on the active stream) may invoke `SaveClip`, `StartRecord`, or `StopRecord`; a guest stream or `InputOnly` role receives `ClipResult { status: Refused, reason: PermissionDenied }`.
- **`kind = 2` (`HostState`, Host -> Client):**
  - `body[0]`: `state` (`0 = Off`, `1 = Armed`, `2 = Recording`, `3 = Saving`, `4 = Unsupported`, `5 = Error`).
  - `body[1]`: `reason` (`DvrReason` enum `u8`: `0 = None`, `1 = Disabled`, `2 = Pyrowave`, `3 = DiskFull`, `4 = QuotaExceeded`, `5 = NoKeyframeYet`, `6 = Busy`, `7 = PermissionDenied`, `8 = AudioCodecUnsupported`, `9 = IoError`, `10 = NotStreaming`, `11 = Unknown`).
  - `body[2..4]` (`u16` LE): `ring_buffered_secs` (current seconds of keyframe-backed video available in RAM).
  - `body[4..6]` (`u16` LE): `ring_capacity_secs` (configured max ring duration).
  - `body[6..10]` (`u32` LE): `recording_elapsed_secs` (`0` unless `state == Recording`).
  - `body[10..18]` (`u64` LE): `quota_used_bytes`.
  - `body[18..26]` (`u64` LE): `quota_limit_bytes`.
- **`kind = 3` (`ClipResult`, Host -> Client):**
  - `body[0]`: `outcome` (`0 = Saved`, `1 = Refused`, `2 = Failed`).
  - `body[1]`: `reason` (`DvrReason` `u8`).
  - `body[2..4]` (`u16` LE): `duration_secs`.
  - `body[4..12]` (`u64` LE): `file_bytes`.
  - `body[12..14]` (`u16` LE): `clip_id_len` (`<= 64` UTF-8 bytes), followed by `clip_id` UTF-8 bytes (ASCII UUID/timestamp slug).

### 2.3 Native API v1 Routes (`client/protocol/native-api/src/dvr.rs`)

All routes mount under `/api/native/v1`, authenticate paired client TLS certificates, and verify device permissions live from `h.paired` on every request (`rust/host/web/src/native_api.rs`). Request structs use `#[serde(deny_unknown_fields)]`; responses use `#[serde(default)]` and `#[serde(other)] Unknown` on enums.

| Method & Route | Permission | Purpose |
|---|---|---|
| `GET /api/native/v1/dvr` | `view` | Live DVR state (`state`, `reason`, `ring_buffered_secs`, `ring_capacity_secs`, `recording`, `recording_elapsed_secs`, `quota_used_bytes`, `quota_limit_bytes`, `clips_dir_available`, `container`) |
| `POST /api/native/v1/dvr/clip` | `launch` | Save instant-replay clip from current session's ring buffer (`{"duration_secs": 60}` optional). Returns `202 Accepted` or `200 OK` with `ClipJob { job_id, clip_id, status }` or error (`409` with stable reason when `pyrowave`, `disabled`, `no_keyframe_yet`, `disk_full`, `not_streaming`) |
| `POST /api/native/v1/dvr/record` | `launch` | Toggle manual recording (`{"action": "start" | "stop"}`). Returns updated `DvrStatus` (and `clip` metadata on `stop`) |
| `GET /api/native/v1/dvr/clips` | `view` | Paginated clip list (`?limit=50&cursor=...`) returning `ClipSummary[]` (`id`, `app_title`, `created_unix_ms`, `duration_ms`, `width`, `height`, `fps`, `codec`, `hdr`, `container`, `size_bytes`, `kind: "replay" | "manual"`) |
| `DELETE /api/native/v1/dvr/clips/{id}` | `host_control` | Permanently delete a clip and its sidecar metadata from disk; updates quota totals atomically |

Console 2.0 also exposes matching session-cookie/CSRF-guarded web routes under `/api/dvr/*` (registered in `token_catalog` in `rust/host/web/src/web.rs`) including `GET /api/dvr/clips/{id}/download` with `Content-Disposition: attachment` and strict path-traversal rejection (`id` validated to `[a-zA-Z0-9_-]{8,64}`).

---

## 3. Architecture & <=800-LOC Module Plan

### 3.1 Zero-Blocking Hot-Path Architecture

```text
Capture -> Convert -> AMF/NVENC Encoder (session thread)
                            │
                            ├──► video_send::Sender (UDP/FEC/pacing — NEVER waits on DVR)
                            │
                            └──► [if dvr_armed.load(Relaxed)] non-blocking try_send(VideoTap)
                                     (cloned Bytes / Arc<[u8]> after slice reassembly)
                                                │
Audio Capture -> Opus Encode (audio thread)     │
                            │                   │
                            ├──► AudioSink (UDP/WebRTC — NEVER waits on DVR)
                            │                   │
                            └──► [if dvr_armed.load(Relaxed)] non-blocking try_send(AudioTap)
                                                │
                                                ▼
                                   ┌─────────────────────────┐
                                   │  `dvr-mux` worker thread │ (BelowNormal priority)
                                   │  - Bounded RAM Ring     │
                                   │    (video GOPs + Opus)  │
                                   │  - Live Manual Rec      │
                                   │  - fMP4 / MKV Remuxer   │
                                   │  - Atomic Write + Quota │
                                   └─────────────────────────┘
```

#### Critical hot-path invariants

1. **Zero blocking on `session`, `video-packetize`, `video-send`, `audio`, and `control` threads:**
   - The tap uses a bounded `crossbeam_channel` / `std::sync::mpsc::sync_channel` (`CAPACITY = 256` messages, ~2 s burst headroom at 120 fps + 200 Hz audio) accessed exclusively via `try_send`.
   - When DVR is off (`!dvr.is_active()`), the tap costs **one `AtomicBool::load(Ordering::Relaxed)`** — zero allocation, zero memcpy, zero lock.
   - If `dvr-mux` is momentarily busy flushing a clip to a slow HDD and the bounded channel fills, `try_send` drops the tap packet immediately, increments an atomic `dvr_tap_drops` counter, and NEVER stalls the stream. On drop, `dvr-mux` marks a discontinuity and resyncs at the next IDR keyframe.
2. **Whole frames and slice-delivery reassembly:**
   - In `rust/windows/graphics/src/encoder.rs`, `Encoded::unit` is either `Unit::Whole`, `Unit::Slice { index }`, or `Unit::Last { index }`.
   - When `Unit::Whole`, the tap clones `encoded.bytes` (or transfers a copy into a small pooled `Vec<u8>`) right before handing `Encoded` to `video_send` (note: `Encoded::drop` returns `bytes` to `crate::frame_pool`, so cloning into a separate DVR pool keeps `frame_pool` contention-free).
   - When `video_slice_delivery` is active (`Unit::Slice` / `Unit::Last`), `dvr-mux` itself reassembles slices by `frame_index` off the session thread — the session thread simply `try_send`s each slice's byte copy with its `(frame_index, unit, idr, pts_us, codec, width, height, fps_millihz, hdr_mode)` metadata.
3. **Keyframes (IDR) & GOP boundary management for Instant Replay:**
   - Moonlight streams normally run with **infinite GOP** (no periodic keyframes unless a packet is lost or the client requests one). An infinite-GOP stream with no keyframes for 10 minutes would make a 60-second ring buffer unplayable!
   - **Solution (zero extra latency, tiny bitrate cost):** When Stream DVR is armed (`Armed` or `Recording`), `dvr_ring_policy` schedules a periodic **soft keyframe cadence** every `DVR_GOP_SECS = 4` seconds (configurable internal constant, clamped to `[2, 10]` s; at 20 Mbps 60 fps a 4 s GOP adds <2% average bits and stays inside the existing `fec_whole_frame_cap` pacing guard!). When DVR is **off**, infinite GOP is completely untouched.
   - If `SaveClip` is pressed before the first IDR arrives, the ring checks `has_keyframe()`; if empty, it immediately triggers one host IDR request and refuses cleanly with `DvrReason::NoKeyframeYet` (or waits up to 250 ms asynchronously on `dvr-mux` for the first IDR before declining).
   - When evicting old entries from the RAM ring buffer to satisfy either `ring_capacity_secs` or `dvr_max_ram_mb` (default `512` MiB, hard ceiling `2048` MiB), eviction always trims up to the next **IDR boundary** (while preserving the active SPS/PPS/VPS/AV1 sequence header!) so the remaining ring always starts cleanly on a decodable keyframe.
4. **Audio tap & codec policy:**
   - In `rust/host/stream/src/stream/audio_encoder.rs`, `AudioEncoder::Opus(opus)` already produces exact 48 kHz Opus packets with monotonic `position: u64` (48 kHz sample counter from `AudioClock`, drift-free!). The tap `try_send`s `(position_samples, channel_count, mapping_family, opus_bytes)` immediately after `opus.encode_frame(samples)`.
   - **Passthrough audio (`PCM`, `AC3`, `E-AC3`):** In v1, fragmented MP4 (`fMP4`) natively supports H.264/HEVC/AV1 + Opus (`Opus`/`dOps` box per ISO/IEC 14496-12 + Opus-in-ISOBMFF spec). If the stream negotiates passthrough `PCM`, `AC3`, or `E-AC3`, the DVR records **video + audio in `.mkv` (Matroska)** if Matroska muxing is selected, OR for `.mp4` explicitly logs a warning and reports `audio_codec_unsupported` (never silently producing a broken/corrupt MP4). Specifically: v1 implements a clean pure-Rust **fragmented MP4 (fMP4)** muxer for `H.264/HEVC/AV1 + Opus` (zero external DLL/FFmpeg process needed, works on every Windows 10/11 PC, plays directly in Windows Media Player, VLC, Discord, browsers, and Console 2.0!) and refuses non-Opus passthrough audio cleanly with `DvrReason::AudioCodecUnsupported` ("Clips require Opus audio; switch audio codec from passthrough to Opus to record clips").
5. **Codec parameter sets & HDR metadata in fMP4:**
   - **H.264 (AVC):** Extract SPS/PPS NALUs from Annex-B start codes on every IDR; write `avc1` + `avcC` box, convert Annex-B NALUs to 4-byte length-prefixed AVCC in `mdat`.
   - **HEVC (H.265):** Extract VPS/SPS/PPS NALUs from Annex-B; write `hvc1` + `hvcC` box + `colr` (`nclx`: BT.709 for SDR, BT.2020 + ST.2084 PQ `[16]` or HLG `[18]` for HDR) + `mdcv`/`clli` from `butterpollo_core::hdr::Metadata`!
   - **AV1:** Extract Sequence Header OBU from keyframe OBU stream; write `av01` + `av1C` + `colr` (`nclx`) + `mdcv`/`clli`.
   - **Mid-stream reconfigure (`0x5532` resolution/FPS/HDR change):** When `stream` resolution, codec, or HDR mode changes mid-stream:
     - Active **Manual Recording** cleanly finalizes the current segment (`..._part1.mp4`) and rolls over seamlessly into a new segment (`..._part2.mp4`) with the new `moov` track config.
     - **Instant Replay ring** stamps a `config_epoch` on each packet; on `SaveClip`, it exports from the latest epoch (or truncates to the current epoch if resolution changed < `duration_secs` ago, never mixing mismatched SPS/dimensions in one `moov`).
6. **Atomic file writes & disk quota enforcement:**
   - Clips live under `<profile>\clips\` (or user-configured `dvr_directory`, validated writable at enable time).
   - Every clip writes to `<id>.mp4.tmp`, flushes with `File::sync_all()`, writes sidecar `<id>.json.tmp` -> renames to `<id>.json`, then atomically renames `<id>.mp4.tmp` -> `<id>.mp4`.
   - **Quota policy:** Before writing (and during manual recording at every 2 s fragment boundary), `dvr-mux` checks `quota_limit_bytes` (default `20 GiB`) and free disk space (`GetDiskFreeSpaceExW`, hard floor `1 GiB` free on target volume):
     - If `quota_used_bytes + estimated_clip_bytes > quota_limit_bytes`: if `dvr_quota_policy == "evict_oldest"` (default for instant replay), evict oldest clips (never deleting the currently open manual recording) until under quota; if still over quota or free disk `< 1 GiB`, refuse/stop cleanly with `DvrReason::DiskFull` / `DvrReason::QuotaExceeded` and emit a visible toast + host warning.

### 3.2 `<=800-LOC` Module Plan & `size_ratchet.json` Compliance

Every new Rust module starts with a `//!` doc header and every new Java class starts with a `/**` header per `tools/arch_check.py`. No grandfathered file is allowed to grow beyond minimal 5–25 LOC delegation hooks.

#### Grandfathered files touched (from `tools/size_ratchet.json`)

| Grandfathered file | Ratchet ceiling | How we avoid growing it |
|---|---|---|
| `rust/host/stream/src/stream/video/run.rs` | `1956` | Single 4-line call `dvr.tap_video(&output, &stream)` in `send_frames` + periodic soft-IDR check via `dvr.wants_idr(now)` (helper lives in new `stream/dvr.rs`; extract 10+ lines of nearby inline helper if net lines would exceed ceiling) |
| `rust/host/stream/src/stream/control.rs` | `2675` | Single match arm `DVR_STATE_MESSAGE_TYPE => crate::stream::dvr::handle_control(...)` (<=6 lines; extract equivalent lines or keep <=2675) |
| `rust/host/stream/src/stream.rs` | `1324` | Add `pub(crate) dvr: Option<Arc<DvrHandle>>` field wiring (<=5 lines) |
| `rust/host/web/src/native_api.rs` | `853` | Add `mod dvr;`, 5 route registrations in `router()`, and 1 `feature::STREAM_DVR` push (<=10 lines; trim/delegated handlers live in `native_api/dvr.rs`) |
| `rust/host/stream/src/state.rs` | `2247` | Add `pub dvr: Arc<crate::dvr_store::DvrStore>` field on `Host` (<=4 lines) |
| `rust/host/web/src/web.rs` | `1731` | Add `/api/dvr/*` dispatch to `crate::web::dvr::handle` (<=8 lines) |
| `rust/core/config/src/config.rs` | `1373` | Delegate `dvr_*` accessors to new `rust/core/config/src/config/dvr.rs` (<=5 lines in `config.rs`) |
| `rust/core/session/src/session.rs` | `1803` | Single `pub dvr_state: Arc<...>` or atomic flag field (<=4 lines) |
| `rust/web/src/lib/api.ts` | `1653` | Put DVR API functions in new `rust/web/src/lib/api/dvr.ts` and re-export (1 line in `api.ts`) |
| `client/protocol/policy/src/http/capabilities.rs` | `1257` | Add `StreamDvr` enum variant & 3-line evidence match (offset by compacting/extracting evidence helpers if needed to stay `<=1257`) |
| `client/protocol/client/src/http/native.rs` | `821` | Put DVR client calls in new `client/protocol/client/src/http/native_dvr.rs` (1 `mod` line in `native.rs`) |
| `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java` | `1347` | Add `STREAM_DVR` enum constant + capability check (<=5 lines) |
| `client/app/src/main/java/com/limelight/ui/StreamMenu.java` | `1321` | Delegate DVR tile rendering & clicks to new `StreamMenuDvrSection.java` (<=6 lines) |
| `client/protocol/desktop-media/src/stream/mod.rs` | `2914` | Delegate `0x554A` events/actions to new `stream/dvr.rs` (<=8 lines) |
| `client/protocol/desktop-config/src/strings/*.rs` | `1750` | **Do NOT edit `strings/*.rs` (0 headroom).** Route all new Stream DVR Desktop translations to the canonical 7.0.0 per-locale overflow files `client/protocol/desktop-config/src/strings_70/<code>.rs` (established in Design 02, chained in `strings.rs` and verified by `strings/tests.rs`). |

#### New `<=800-LOC` modules to create

| Planned Path | Est. LOC | Ownership (`//!` / `/**`) |
|---|---|---|
| `client/protocol/core/src/stream_dvr.rs` | ~220 | Wire codecs for `0x554A` (`DVR_STATE_MESSAGE_TYPE`), `ClientRequest`, `HostState`, `ClipResult`, `DvrReason`, unit tests |
| `client/protocol/native-api/src/dvr.rs` | ~180 | Native API v1 types (`DvrStatus`, `ClipRequest`, `RecordRequest`, `ClipSummary`, `ClipList`, `DvrState`, `DvrReason`) + fixture serde tests |
| `rust/core/media-policy/src/dvr.rs` | ~380 | Pure OS-free DVR state machine, bounded RAM GOP ring buffer (`RingBuffer`), byte/time/keyframe eviction, slice reassembler, soft-IDR scheduler, quota policy |
| `rust/core/media-policy/src/dvr/fmp4.rs` | ~520 | Pure-Rust ISO BMFF fragmented MP4 muxer (`ftyp`/`moov`/`avcC`/`hvcC`/`av1C`/`dOps`/`colr`/`mdcv`/`clli`/`moof`/`mdat`), Annex-B -> AVCC/HVCC converter, OBU parser |
| `rust/core/config/src/config/dvr.rs` | ~120 | Typed `DvrConfig` parser/validator (`dvr_enabled`, `dvr_ring_seconds`, `dvr_max_ram_mb`, `dvr_quota_gb`, `dvr_directory`) |
| `rust/host/stream/src/dvr_store.rs` | ~340 | Host-wide clip catalog, atomic `.tmp` write/rename, startup orphan `.tmp` cleanup, disk-space & quota enforcement (`GetDiskFreeSpaceExW` via `butterpollo-windows`) |
| `rust/host/stream/src/stream/dvr.rs` | ~420 | Per-session `DvrHandle` (lock-free `AtomicBool` + bounded `sync_channel`), `dvr-mux` worker thread (`BelowNormal`), `0x554A` control handler, session teardown flush |
| `rust/host/web/src/native_api/dvr.rs` | ~260 | Axum handlers for `GET /dvr`, `POST /dvr/clip`, `POST /dvr/record`, `GET /dvr/clips`, `DELETE /dvr/clips/{id}` |
| `rust/host/web/src/web/dvr.rs` | ~220 | Console 2.0 `/api/dvr/*` handlers including safe clip download & open-folder action |
| `rust/web/src/lib/api/dvr.ts` | ~120 | Typed Console 2.0 DVR API client |
| `rust/web/src/components/dvr/ClipsDrawer.svelte` | ~380 | Console 2.0 Clips drawer (list, quota meter, download, preview, delete) |
| `client/protocol/client/src/http/native_dvr.rs` | ~160 | `nightjar-core` HTTP wrappers for `/api/native/v1/dvr*` |
| `client/protocol/desktop-media/src/stream/dvr.rs` | ~240 | Desktop DVR state tracking, `0x554A` dispatch, Stream Menu tile & shortcut trigger |
| `client/app/src/main/java/com/limelight/ui/StreamMenuDvrSection.java` | ~240 | Android Stream Menu **Save Clip** + **Record** tile & status toast |
| `client/protocol/desktop-config/src/strings_70/<code>.rs` (14 per-locale files from Design 02) | +~25 LOC each (<=800) | All 14 non-English Desktop translations for Stream DVR (English literals at call sites; zero lines added to grandfathered `strings/*.rs`) |

*(Note: `rust/host/stream/src/stream/audio.rs` is 495 LOC and `audio_encoder.rs` is 251 LOC — both well below the 800-LOC ceiling; tapping Opus output in `audio_encoder.rs` adds ~8 lines. Also all stream config reads go through `rust/host/stream/src/stream/config.rs`, never raw `Config` reads inside `stream/`, satisfying `arch_check.py`'s `raw-config-read` lint.)*

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### 4.1 DVR State Machine (`butterpollo_core::dvr::State`)

```text
                  ┌──────────────────────────────────────┐
                  │                Off                   │
                  │ (dvr_enabled=false; zero threads/RAM)│
                  └──────────────────┬───────────────────┘
                                     │ stream starts & dvr_enabled=true
                    ┌────────────────┴────────────────┐
                    ▼                                 ▼
         ┌────────────────────┐            ┌────────────────────┐
         │    Unsupported     │            │       Armed        │◄─────────┐
         │ (pyrowave / non-   │            │ (ring filling,     │          │
         │  Opus passthrough) │            │  4 s soft IDR)     │          │
         └────────────────────┘            └────┬──────────┬────┘          │
                                                │          │               │
                                       SaveClip │          │ StartRecord   │
                                                ▼          ▼               │
                                       ┌──────────────┐ ┌──────────────┐   │
                                       │    Saving    │ │  Recording   │   │
                                       │ (snapshot -> │ │ (streaming   │   │
                                       │  atomic .mp4)│ │  fMP4 frags) │   │
                                       └──────┬───────┘ └──────┬───────┘   │
                                              │ done           │ StopRecord│
                                              └────────────────┴───────────┘
                                                      │ I/O or DiskFull
                                                      ▼
                                               ┌──────────────┐
                                               │    Error     │ (recovers to Armed
                                               │ (loud warn)  │  after quota freed)
                                               └──────────────┘
```

- **Concurrent `SaveClip` during `Recording`:** Permitted! `SaveClip` snapshots the RAM ring buffer (whose ring continues running during manual recording) into its own atomic replay clip without interrupting the active manual recording file.
- **Concurrent `SaveClip` while `Saving`:** Returns `ClipResult { outcome: Refused, reason: Busy }` (at most one replay flush at a time to bound disk I/O bursts).

### 4.2 Reconnect & Edge-Case Matrix

| Scenario | Host & DVR Behavior |
|---|---|
| **Client disconnects cleanly / network drops (session enters `Suspended`)** | If `Recording`, `dvr-mux` immediately finalizes the active `.mp4.tmp` -> `.mp4` atomically (never losing the recorded footage!) and clears the RAM ring on stream end. When client reconnects (`/resume`), a fresh `DvrHandle` arms cleanly for the new stream. |
| **Client roams IP / network path** | Stream session continues uninterrupted; DVR ring and active recording keep running seamlessly across the roam. |
| **Second device joins as guest (`multiclient`)** | Only the session owner's stream taps video/audio into DVR. Guest control requests on `0x554A` receive `PermissionDenied`. |
| **Continue Here (owner moves stream to another device)** | Old stream stops -> active manual recording finalizes atomically; new owner stream arms fresh ring buffer. |
| **Mid-stream reconfigure (`0x5532` resolution, FPS, or HDR toggle)** | Active manual recording finalizes segment `_partN.mp4` and opens `_part(N+1).mp4`; replay ring advances `config_epoch` so clips never corrupt `moov` track headers. |
| **Encoder recreation / GPU device loss (`DXGI_ERROR_DEVICE_REMOVED`)** | `dvr-mux` marks a timestamp continuity gap and waits for the rebuilt encoder's first IDR before appending; active recording splits cleanly if codec parameters changed. |
| **Stock Moonlight client streams with `dvr_enabled = true`** | Host DVR works transparently! User can trigger Save Clip / Record from Console 2.0 or Native API companion, while stock Moonlight sees zero extra protocol traffic. |
| **PyroWave stream** | `DvrHandle` enters `Unsupported(Pyrowave)` immediately; zero ring RAM allocated, zero `dvr-mux` thread spawned, `0x554A` and `GET /dvr` report `unsupported` (`pyrowave`). |
| **Host clean shutdown (`session_shutdown`)** | `DvrHandle::drop` signals `dvr-mux` with a bounded 3-second drain timeout to finalize any active manual recording atomically before process exit. |

### 4.3 Crash-Restore Journal & Orphan Cleanup

- Stream DVR modifies **no OS-global state** (no display, driver, audio route, or game file mutations), so a host crash leaves at most an in-progress `<id>.mp4.tmp` / `<id>.json.tmp` file in the clips directory.
- **Startup cleanup (`DvrStore::recover_at_startup`):** Called during host initialization (`rust/host/src/main.rs` before HTTP/RTSP servers accept requests):
  1. Scans `<clips_dir>` for `*.mp4.tmp` and `*.json.tmp`.
  2. For a manual recording `.mp4.tmp` that already contains complete flushed `moof`/`mdat` fragments (because `fMP4` flushes fragment-by-fragment every 2 s!), `DvrStore::recover_at_startup` validates the box structure: if valid with >= 1 complete fragment, it recovers the clip as `<id>-recovered.mp4`, writes its sidecar metadata, and emits an informational log entry; otherwise it deletes the torn `.tmp` file so no corrupt/orphaned bytes consume user quota.
  3. Recalculates `quota_used_bytes` from verified `.mp4` files on disk.

---

## 5. Settings & 10/10 Craft Plan

### 5.1 Fewer-Settings Rule & Default-Off Gate

Only **4 host settings** in a single folded Console 2.0 group (**Stream DVR & Clips**, under `rust/web/src/lib/schema/video.ts` `docs/code-map.md` and `rust/host/web/src/console/settings.rs` `docs/code-map.md`) and **zero local preference keys** on Nightjar (so `SettingsKeysTest` is untouched):

| Key | Type | Default | Help text (exact 2-sentence rule: what you get, what it costs, when to change it) |
|---|---|---|---|
| `dvr_enabled` | bool | `false` | Keeps the last moments of your stream in host memory so you can save clips or record without re-encoding. Uses RAM for the replay buffer and adds a keyframe every few seconds; leave off unless you save clips. |
| `dvr_ring_seconds` | choice (`30`, `60`, `90`, `120`) | `60` | Sets how many seconds of recent gameplay Instant Replay keeps ready to save. Longer windows hold more host RAM (capped automatically at the memory limit); lower to 30 seconds on 16 GB systems. |
| `dvr_quota_gb` | int (`5..=500`) | `20` | Limits total disk space used by saved clips, automatically removing the oldest replay clips when full. Raising the limit keeps more clips at the cost of drive space; adjust to match your storage drive. |
| `dvr_directory` | path | `""` (`<profile>\clips`) | Chooses the folder on this PC where saved clips and recordings are stored. Clips write sequentially after you press Save; point this at a secondary drive if your system drive is small. |

- Hidden tuning / safety ceiling (absent from Console UI): `dvr_max_ram_mb` (default `512`, clamped `128..=2048`), keeping 4K60 60 Mbps streams strictly bounded even on 120 s rings.
- **Default-off gate:** `dvr_enabled` stays `false` by default until Wave 1 HW verification (`07-hw-verify.md`) confirms zero stream-path latency regression; even after passing, DVR remains opt-in because holding 512 MiB of RAM and writing clips to disk should require explicit user consent.

### 5.2 10/10 Craft & Policy Compliance

- **Zero `"Experimental"` anywhere in UI:** Never label DVR, Save Clip, or Clips drawer as "Experimental" in Console 2.0 or Nightjar.
- **Zero mod names in UI/Nightjar:** No third-party mod names appear anywhere in UI, help text, or clip metadata.
- **Zero stream-time blur:** Nightjar's Stream Menu Save Clip tile and confirmation toast use crisp solid/elevated surface cards (`nightjar-desktop` egui / Android `StreamMenu.java`), NEVER backdrop blur over live video frames.
- **Dynamic input glyphs:** Nightjar's Save Clip shortcut hint and Stream Menu action footer adapt dynamically to the active input device via `nightjar-policy::input_glyphs` (TV remote `OK` / `Hold OK`, Xbox `A`/`Y` or `X`/`Y`, PlayStation `×`/`△` or `□`/`△`, Nintendo `B`/`X` or `Y`/`X`, keyboard key-cap glyph); **touch and mouse show NO button hints** (`client/design/README.md:143` — clean buttons only, zero key-cap or tap hints).
- **Plain-English error states:** Every `DvrReason` maps to a clear, actionable sentence:
  - `Disabled` -> `"Instant Replay is turned off on this PC. Enable Stream DVR in Rubylight settings to save clips."`
  - `Pyrowave` -> `"Clips cannot be saved while streaming with PyroWave. Switch to H.264, HEVC, or AV1 to use Stream DVR."`
  - `DiskFull` / `QuotaExceeded` -> `"Not enough free disk space on the host PC to save this clip. Free up space or delete older clips."`
  - `NoKeyframeYet` -> `"Waiting for the stream to capture a full frame. Try saving again in a moment."`
  - `AudioCodecUnsupported` -> `"Clips require Opus audio. Switch audio codec from passthrough to Opus to record clips."`
- **All 15 locales:** Every new UI string (Stream Menu tiles, toasts, error messages, Console 2.0 settings & Clips drawer) lands in all 15 locales: `en`, `de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant` across `rust/assets/locale/`, `client/protocol/desktop-config/src/strings_70/<code>.rs` (canonical 7.0.0 overflow files from Design 02; zero lines added to grandfathered `strings/*.rs`), and `client/app/src/main/res/values-*/strings.xml`.

---

## 6. Disjoint Implementer Slices

Designed for Phase 3 `parallel()` execution with **strictly non-overlapping file lists**. Slices `5A` and `5B` run in parallel; `5C` (entire `butterpollo` host backend including `stream/dvr/**`, `native_api/dvr.rs`, `web/dvr.rs`, and `console/settings.rs`), `5D` (**ONLY `rust/web/**` frontend files** — zero intra-crate `butterpollo` compile dependency on `5C`!), and `5E` run in parallel once `5A`+`5B` land.

| Slice | Parallel Group | Exclusive File List (zero overlap!) |
|---|---|---|
| **5A: Protocol & Native API Wire Contracts** | Stage 1 | `client/protocol/core/src/ids.rs`, `client/protocol/core/src/lib.rs`, `client/protocol/core/src/stream_dvr.rs` (new), `client/protocol/core/src/fuzz.rs`, `client/protocol/native-api/src/lib.rs`, `client/protocol/native-api/src/dvr.rs` (new), `client/protocol/native-api/tests/fixtures.rs`, `client/protocol/native-api/tests/fixtures/dvr_*.json` (new), `docs/control-messages.md`, `docs/native-api.md` |
| **5B: Core Ring Buffer, fMP4 Muxer & Config** | Stage 1 (parallel with 5A) | `rust/core/src/lib.rs`, `rust/core/media-policy/src/dvr.rs` (new), `rust/core/media-policy/src/dvr/fmp4.rs` (new), `rust/core/config/src/config.rs`, `rust/core/config/src/config/dvr.rs` (new), `rust/core/config/src/config/overrides.rs`, `rust/core/config/src/config/tests.rs` |
| **5C: Host Engine, Taps, Control, Native API & Web/Console Backend** | Stage 2 (after 5A+5B) | `rust/core/media-policy/src/extensions.rs`, `rust/core/session/src/session.rs`, `rust/host/stream/src/dvr_store.rs` (new), `rust/host/stream/src/stream/dvr.rs` (new; `rust/host/stream/src/stream/dvr/**`), `rust/host/stream/src/stream.rs`, `rust/host/stream/src/stream/video/run.rs`, `rust/host/stream/src/stream/audio_encoder.rs`, `rust/host/stream/src/stream/control.rs`, `rust/host/stream/src/extensions.rs`, `rust/host/stream/src/state.rs`, `rust/host/src/main.rs`, `rust/host/web/src/native_api.rs`, `rust/host/web/src/native_api/dvr.rs` (new), `rust/host/web/src/web.rs`, `rust/host/web/src/web/dvr.rs` (new), `rust/host/web/src/console/settings.rs`, `rust/assets/locale/*.json`, `docs/configuration.md`, `tests/host/tests/nvhttp_parity/native_tests/dvr.rs` (new), `tests/host/tests/reconnect_tests/dvr_tests.rs` (new) |
| **5D: Console 2.0 Web Frontend (`rust/web/**` Only)** | Stage 2 (parallel with 5C — zero `butterpollo` crate dependency) | `rust/web/src/lib/api.ts`, `rust/web/src/lib/api/dvr.ts` (new), `rust/web/src/lib/schema/video.ts`, `rust/web/src/components/dvr/ClipsDrawer.svelte` (new), `rust/web/DvrClips.test.mjs` (new) |
| **5E: Nightjar Core, Desktop & Android** | Stage 2 (parallel with 5C+5D) | `client/protocol/policy/src/http/capabilities.rs`, `client/protocol/client/src/http/native.rs`, `client/protocol/client/src/http/native_dvr.rs` (new), `client/protocol/client/src/control/messages.rs`, `client/protocol/client/src/control/events.rs`, `client/protocol/client/src/control/session.rs`, `client/protocol/client/tests/control_messages_contract.rs`, `client/protocol/client/tests/capabilities_contract.rs`, `client/protocol/desktop-media/src/stream/mod.rs`, `client/protocol/desktop-media/src/stream/dvr.rs` (new), `client/protocol/desktop-config/src/strings_70/*.rs` (per-locale `strings_70/<code>.rs`; never editing grandfathered `strings/*.rs`), `client/protocol/desktop-config/src/strings/tests.rs`, `client/protocol/android/src/lib.rs`, `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java`, `client/app/src/main/java/com/limelight/nvstream/jni/RustBridge.java`, `client/app/src/main/java/com/limelight/ui/StreamMenu.java`, `client/app/src/main/java/com/limelight/ui/StreamMenuDvrSection.java` (new), `client/app/src/main/res/values*/strings.xml`, `client/app/src/test/resources/protocol/capabilities/*` |

*(Every path above is marked `docs/code-map.md` for Phase 2 reconciliation before worktrees spawn.)*

---

## 7. Verification & Measurement Plan

### 7.1 Narrowest Unit, Property & Interop Tests (No GPU Required)

- **Wire & fuzz tests:**
  - `cargo test -p rubylight-protocol --locked stream_dvr` and `ids` (verifies `0x554A` uniqueness, bit 15 clear, `0x554F` free, round-trip encode/decode).
  - `PROPTEST_CASES=2000 cargo test -p rubylight-protocol --locked fuzz` (zero panics on arbitrary bytes).
- **Core ring & fMP4 muxer tests:**
  - `cargo test -p butterpollo-core --locked dvr` (GOP-aligned eviction, RAM byte cap enforcement, slice reassembly, H.264/HEVC/AV1 + Opus fMP4 box validity, `colr`/`mdcv`/`clli` HDR boxes, mid-stream reconfigure epoch split, crash `.tmp` recovery).
  - External mux verification in CI/local test: decode generated synthetic H.264/HEVC/AV1 + Opus `.mp4` bytes with `ffprobe` / `ffmpeg -v error -i clip.mp4 -f null -` (0 errors, exact track metadata, A/V duration delta `<= 20 ms`).
- **Host session, native API & interop tests:**
  - `cargo test -p butterpollo --locked dvr` (runs `reconnect_tests/dvr_tests.rs` and `nvhttp/native_tests/dvr.rs`: stock client untouched, `0x554A` save/record/refusal, PyroWave `pyrowave` refusal, guest `PermissionDenied`, disk-quota FIFO eviction, orphan `.tmp` recovery, disconnect finalizes recording).
  - `cargo test -p butterpollo --locked interop_tests` (Nightjar engine <-> host `0x554A` & `/api/native/v1/dvr` end-to-end in-process).
- **Nightjar tests:**
  - `cargo test -p nightjar-core --locked capabilities_contract control_messages_contract`
  - `cargo test -p nightjar-desktop --locked dvr strings`
  - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.ui.StreamMenuTest --tests com.limelight.preferences.SettingsKeysTest`
- **Architecture & code map gates:**
  - `python tools/arch_check.py` (800-LOC ceilings, `//!` headers, zero `Family::Rubylight` gating, zero raw config reads in `stream/`)
  - `python tools/code_map.py check`

### 7.2 Visual & Screenshot Checks

- **Console 2.0:** `cd rust/web && npm run check && node --test DvrClips.test.mjs SettingsSchema.test.mjs` + Playwright screenshot capture in `rust/web/scripts/screenshots.cjs` showing the Clips drawer (populated, empty, and quota-near-full states).
- **Nightjar Desktop:** `shot.rs` rasterized screenshots of Stream Menu showing Save Clip tile (Armed, Recording timer, Unsupported-PyroWave disabled state) verifying zero backdrop blur and dynamic controller glyphs.
- **Nightjar Android:** Robolectric layout tests in `StreamMenuTest` verifying Save Clip tile visibility gated strictly by `HostCapabilities.Feature.STREAM_DVR`.

### 7.3 Serialized Idle-Gated Hardware Criteria

Executed exclusively under [07-hw-verify.md](07-hw-verify.md) on **RX 7900 XT desktop** and **Radeon 780M laptop** (NVIDIA NVENC tap path is identical `Encoded` struct output — checked against `nvenc.rs` and `nvenc/tests.rs` fake-driver tests, never claimed as run on NVIDIA hardware):

1. **Idle gate:** Verify `(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` shows `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0`, `GPU-LOCK` held, `machine_load.py` quiet CPU, isolated test host on `--port 48123 --bind 127.0.0.1`.
2. **ABBA latency gate (`e2e.py --record` + `timing_report.py`, 1080p60 HEVC 20 Mbps and 4K60 HEVC 60 Mbps, 90 s arms):**
   - DVR **off**: `0.00 ms` delta vs baseline (single relaxed atomic check).
   - DVR **armed (60 s ring)** with 3 concurrent `POST /dvr/clip` flushes during the 90 s stream:
     - Convert p99 delta `<= +0.05 ms`
     - Encode p99 delta `<= +0.10 ms`
     - `present_to_sent` p99 delta `<= +0.20 ms`
     - `slice_delivery_overflows`: `0` increase
     - `dvr_tap_drops == 0` on NVMe SSD; audio thread drop count `0`
     - Saved `.mp4` passes `ffprobe` + `ffmpeg -f null -` with A/V sync offset `<= 20 ms`, duration within `±4 s` (1 GOP) of requested window.
3. **Exit code 3 abort rule:** If `e2e.py` or `check.ps1` exits with code **3** (AMD encoder hang), **STOP immediately**, preserve `result.json` (`gpu_hang`), and investigate — **NEVER retry**.
