# Away Wake Relay over Traverse Peer Mesh (7.0.0 — Wave 3)

[Protocol reservations](00-protocol-reservations.md) · [Code map](../code-map.md) · [Nightjar code map](../../client/docs/code-map.md)

Status: engineering design (Phase 1). Written against `main` `b3d0c26b9b` (`6.2.0`) and `00-protocol-reservations.md`.

## 1. Summary & User Experience

Wake-on-LAN (WoL) magic packets are Layer-2/subnet-broadcast frames that routers drop across the internet. Today, waking a sleeping gaming PC from outside the home LAN requires router port forwarding, DDNS, a VPN, or a third-party cloud relay account. **Away Wake Relay** solves this cleanly on top of Rubylight's existing zero-server `rubylight-traverse` peer mesh (`client/protocol/traverse/`):

1. **Any always-on paired peer on the same home LAN** can act as an opted-in **Wake Relay**:
   - another awake Rubylight host on that LAN (e.g., an HTPC or home server);
   - a lightweight mini-PC / NUC / Linux box running Rubylight's headless relay mode (`butterpollo --wake-relay` or `nightjar-cli wake-relay`); or
   - an already-awake **Desktop Nightjar** (Windows/Linux/macOS) whose user explicitly turned on *"Act as a LAN wake relay"*.
2. **One-tap Wake from anywhere:** When a user on mobile data or hotel Wi-Fi taps **Wake** on an offline PC in Nightjar, Nightjar tries direct LAN WoL first (if on a LAN). If unreachable and an enrolled Wake Relay exists for that PC, Nightjar looks up the relay's encrypted BEP 44 DHT record via `rubylight-traverse`, hole-punches a sealed UDP session to the relay, sends a cryptographically signed, replay-protected Wake Request, and displays live progress: *"Contacting wake relay (Living Room Mini-PC)…" → "Wake signal sent on home network…" → "Waiting for Gaming PC to start…" → "Online"*.
3. **Zero cloud accounts, zero port forwarding, zero unauthenticated amplification:** Every DHT record is encrypted and signed with HKDF-derived keys from a per-device/per-relay secret exchanged over paired LAN HTTPS. An unauthenticated packet on the internet cannot trigger a single LAN broadcast or elicit any UDP reply.
4. **Honest offline errors:** If the relay itself is asleep/offline, the target PC's NIC wake settings were disabled, or the user never enrolled a relay, Nightjar says so immediately in plain English with the exact fix.

## 2. Protocol, Native API & Capability Contract

### Compatibility invariants

- **`PROTOCOL_VERSION` stays unchanged.** Away Wake Relay adds zero ENet control message IDs, zero input magics, and zero RTSP `DESCRIBE` lines (`00-protocol-reservations.md` §4).
- **`rubylight_native_api::API_VERSION` stays `1`.** Strictly additive endpoints under `/api/native/v1`, `#[serde(default)]` responses, `#[serde(deny_unknown_fields)]` requests, `Unknown` catch-all enum variants.
- **Capability-gated, never brand-gated.** Nightjar gates Away Wake Relay UI strictly on `HostCapabilities::has(Feature::AwayWakeRelay)` (`HostCapabilities.Feature.AWAY_WAKE_RELAY` on Java), never on `Family::Rubylight`.

### Capability advertisement

| Surface | Field / String | Condition |
| --- | --- | --- |
| `/serverinfo` | `<AwayWakeRelayVersion>1</AwayWakeRelayVersion>` | `Offered::away_wake_relay == true` (host `internet_play == true` AND explicit opt-in `away_wake_relay == true`) |
| Native API `Capabilities::features` | `"wake.relay"` (`feature::WAKE_RELAY`) | Host has `away_wake_relay == true` and caller has `launch` or `host_control` |

`HostCapabilities::Feature::AwayWakeRelay` (`AWAY_WAKE_RELAY` in Java) is proven when `AwayWakeRelayVersion >= 1` **and** native `"wake.relay"` is present.

### Cryptographic key hierarchy & `rubylight-traverse` extension

Builds directly on `client/protocol/traverse/src/crypto.rs` (`DOMAIN = b"rubylight internet play v1"`, HKDF-SHA256, Ed25519, ChaCha20-Poly1305) and `record.rs` (`WINDOW_SECONDS = 3600`, `CLIENT_FRESH_SECONDS = 120`):

1. **Per-target relay enrollment secret (`relay_master_secret: [u8; 32]`):** Generated when the host administrator enrolls a LAN relay peer via `POST /wake/relay/enroll` (over paired LAN HTTPS). Both the target host and the LAN relay store `(relay_id, target_host_id, target_macs, target_broadcasts, target_http_port, relay_master_secret, epoch)`.
2. **Per-device wake credential (`device_wake_secret: [u8; 32]`):** Derived on the target host for each paired client device with `launch` permission:
   ```text
   device_wake_secret = HKDF-SHA256(
       salt = "rubylight internet play v1",
       ikm  = relay_master_secret,
       info = "wake relay device secret" || target_host_id || device_cert_sha256 || epoch_be_u64
   )
   ```
   Handed to the paired client device over LAN HTTPS via `GET /api/native/v1/wake/relay` as a `WakeRelayBundle`. Because `device_wake_secret` is bound to `device_cert_sha256` and `epoch`, unpairing one device and bumping `epoch` revokes that device's wake authority cleanly without re-enrolling the LAN relay hardware!
3. **Domain-separated DHT records (`record::Role`):**
   - `Role::WakeRelay = 3`: The LAN relay publishes an encrypted BEP 44 mutable item per enrolled `(target_host_id, epoch)` (republished every `30 min`, matching `traverse/src/host.rs::PUBLISH_EVERY`) using key derived with label `b"wake relay dht"` so it NEVER collides with `Role::Host (1)` or `Role::Client (2)` records. Plaintext carries `{ relay_session: [u8; 16], endpoints: Vec<SocketAddr>, nat: Nat, epoch: u64 }`.
   - `Role::WakeRequester = 4`: When waking from away, the remote Nightjar publishes a short-lived requester record (`CLIENT_FRESH_SECONDS = 120`) derived from `device_wake_secret` + punches the relay's endpoints.
4. **Signed, replay-protected Traverse wire datagrams (`wire::VERSION = 1`):**
   Extend `client/protocol/traverse/src/wire.rs::Kind` with two new datagram kinds, both sealed with ChaCha20-Poly1305 under ephemeral per-punch `PathKeys` (`host_to_client` / `client_to_host`):
   - `Kind::WakeRequest = 7` (Requester → Relay):
     ```text
     Offset  Len  Field (plaintext inside AEAD)
     0       1    wake_version = 1
     1       1    flags (bit 0 = probe_host_up)
     2       2    reserved (0)
     4       8    epoch (u64 LE)
     12      8    unix_timestamp (u64 LE; must be within +/- 120 s of relay clock)
     20      16   nonce ([u8; 16]; CSPRNG random, checked against relay replay cache)
     36      32   device_id_hash (SHA-256 of paired device cert)
     68      16   target_host_hash (first 16 bytes of SHA-256(target_host_id))
     84      64   ed25519_sig (signed by HKDF-derived Ed25519 key from device_wake_secret
                               over b"rl-wake-v1" || epoch || unix_timestamp || nonce
                               || device_id_hash || target_host_hash)
     Total plaintext = 148 bytes.
     ```
   - `Kind::WakeStatus = 8` (Relay → Requester):
     ```text
     Offset  Len  Field (plaintext inside AEAD; strictly <= 96 bytes -> zero amplification!)
     0       1    wake_version = 1
     1       1    status:
                    1 = PacketSent        (WoL magic packet emitted on LAN)
                    2 = WaitingForBoot    (polling target local /serverinfo)
                    3 = HostAwake         (target /serverinfo responded!)
                    4 = TimedOut          (45 s elapsed after WoL, no /serverinfo)
                    5 = RateLimited       (retry_after_sec populated)
                    6 = RejectedAuth      (bad sig, revoked epoch, or unknown device)
                    7 = RejectedReplay    (duplicate nonce or clock skew > 120 s)
                    8 = TargetNotOnLan    (no matching LAN interface/broadcast)
     2       2    retry_after_sec (u16 LE)
     4       16   echo_nonce ([u8; 16])
     20      4    elapsed_ms (u32 LE)
     24      ..   optional diagnostic detail (<= 64 bytes UTF-8)
     Total plaintext <= 88 bytes (< 148-byte WakeRequest).
     ```

### Anti-amplification & anti-abuse guarantees

1. **Zero unauthenticated traffic:** Any UDP packet failing `wire::is_wire`, unknown `path_tag`, or ChaCha20-Poly1305 authentication is dropped silently with **0 bytes sent** (never an error reply).
2. **Strict amplification factor `< 1.0`:** `WakeStatus` is at most 88 bytes plaintext vs. 148 bytes for `WakeRequest`, and a relay emits at most 1 `PacketSent` + 1 terminal `HostAwake`/`TimedOut` per validated request.
3. **Replay protection:** Relay keeps a bounded `VecDeque` + `HashSet` of the last 512 `(nonce, expires_at)` tuples across a `±120 s` window (`240 s` retention). Reused nonces or stale timestamps return `RejectedReplay` without emitting any WoL packet.
4. **Rate limiting:** Per-`(device_id_hash, target_host_hash)` token bucket allows at most **1 WoL burst** (3 UDP magic packets across target's subnet broadcasts on ports 9 & 7) per **10 seconds**, capped at **10 bursts per 10 minutes**.

### Native API v1 routes (`rust/core/native-api/src/wake_relay.rs`)

| Method & Path | Permission | Purpose |
| --- | --- | --- |
| `GET /api/native/v1/wake/relay` | `launch` or `host_control` | Returns relay readiness, list of enrolled LAN relay peers (name, id, last_seen_unix, online), and (for paired callers with `launch`) the caller's `WakeRelayBundle` for away wake. |
| `POST /api/native/v1/wake/relay/enroll` | `host_control` | Enrolls or revokes a LAN relay peer (`action: "enroll" \| "revoke" \| "rotate_epoch"`), validating that the peer shares an IPv4 LAN subnet (`Adapter::shares_subnet`, `rust/core/src/wake.rs:87`) before issuing `relay_master_secret`. |
| `POST /api/native/v1/wake/relay/wake` | `launch` | Direct HTTPS trigger on a relay host (when reachable over LAN or an existing Traverse tunnel) taking `{ target_host_id, epoch, unix_timestamp, nonce_hex, signature_hex }` and returning `WakeStatusResponse`. |

## 3. Architecture & <=800-LOC Module Plan

All repo paths below carry `[verify post-sprint code-map.md]` for Phase 2 re-verification. Every new file stays strictly `<= 800` LOC with `//!`/`/**` headers and passes `tools/arch_check.py`.

### Grandfathered files in `tools/size_ratchet.json` (DO NOT GROW)

| File | Ratchet ceiling (LOC) | Strategy to stay at or below ceiling |
| --- | --- | --- |
| `rust/host/src/internet_play.rs` `[verify post-sprint code-map.md]` | 1010 | Add only a 2-line hook starting/stopping `crate::wake_relay` when `internet_play` restarts or paired devices change. |
| `rust/host/src/wake.rs` `[verify post-sprint code-map.md]` | 1121 | Reuse existing `readiness()`, `adapters()`, and `WakeGrace` unmodified; zero relay networking added to `wake.rs`. |
| `rust/host/src/native_api.rs` `[verify post-sprint code-map.md]` | 853 | Add `mod wake_relay;` + 3 route registrations + 1 capability line (`<= 853`). |
| `client/protocol/traverse/src/host.rs` `[verify post-sprint code-map.md]` | 1112 | Keep existing streaming host service unchanged; Wake Relay runs its own lightweight `WakeRelayService` in `traverse/src/wake_relay_node.rs` reusing `Node`, `stun`, `dht`, and `punch`. |
| `client/protocol/traverse/src/tunnel.rs` `[verify post-sprint code-map.md]` | 1207 | Untouched — Wake Relay uses single sealed `WakeRequest`/`WakeStatus` datagrams over the punched path, no TCP stream tunnel required! |
| `client/protocol/client/src/http/capabilities.rs` `[verify post-sprint code-map.md]` | 1257 | Minimal `Feature::AwayWakeRelay` addition (`<= 1257`). |
| `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java` `[verify post-sprint code-map.md]` | 1347 | Minimal `AWAY_WAKE_RELAY(0)` addition (`<= 1347`). |
| `client/protocol/desktop/src/hosts.rs` `[verify post-sprint code-map.md]` | 1648 | Delegate remote wake & desktop relay worker to `hosts/wake_relay.rs` (3-line hook). |
| `client/app/src/main/java/com/limelight/computers/HostActions.java` `[verify post-sprint code-map.md]` | 1484 | Delegate away-wake fallback to `computers/WakeRelayClient.java` (3-line hook in `sendWol`). |
| `client/app/src/main/java/com/limelight/computers/ComputerManagerService.java` `[verify post-sprint code-map.md]` | 1573 | Store opaque `WakeRelayBundle` JSON alongside existing `internetPlay` bundle via `WakeRelayStore.java` (<=4 lines). |

### Planned new <=800-LOC modules

| New module path | Crate / App | Target LOC | Responsibility |
| --- | --- | --- | --- |
| `client/protocol/traverse/src/wake_relay.rs` `[verify post-sprint code-map.md]` | `rubylight-traverse` | ~480 | Sans-IO Wake Relay crypto/record/wire logic: `WakeRelayKeys`, `WakeRequest`/`WakeStatus` codecs, Ed25519 sign/verify, 512-entry sliding replay cache, per-device rate limiter, full NAT-simulator tests (`sim.rs`). |
| `client/protocol/traverse/src/wake_relay_node.rs` `[verify post-sprint code-map.md]` | `rubylight-traverse` | ~420 | Socket-driving `WakeRelayPeer` (STUN + DHT publish + punch answer + WoL callback) and `WakeRequester` (DHT lookup + punch + signed request + status stream). |
| `rust/core/native-api/src/wake_relay.rs` `[verify post-sprint code-map.md]` | `rubylight-native-api` | ~220 | Additive v1 serde types (`WakeRelayInfo`, `WakeRelayBundle`, `RelayEnrollRequest`, `RelayWakeRequest`, `WakeStatusResponse`) + fixture tests. |
| `rust/core/src/wake/relay.rs` `[verify post-sprint code-map.md]` | `butterpollo-core` | ~340 | Pure enrollment/revocation store model, 102-byte WoL magic packet builder (`6x 0xFF + 16x MAC`), LAN subnet verification (`Adapter::shares_subnet`), epoch rotation. |
| `rust/host/src/wake_relay.rs` `[verify post-sprint code-map.md]` | `butterpollo` | ~440 | Host runtime: persists `wake-relay.json`, runs `WakeRelayPeer` when enrolled as a LAN relay, emits LAN UDP broadcast WoL packets, probes target `/serverinfo`, wires `--wake-relay` headless mode. |
| `rust/host/src/native_api/wake_relay.rs` `[verify post-sprint code-map.md]` | `butterpollo` | ~260 | Axum handlers for `GET /wake/relay`, `POST /wake/relay/enroll`, `POST /wake/relay/wake`. |
| `client/protocol/client/src/wake_relay.rs` `[verify post-sprint code-map.md]` | `nightjar-core` | ~310 | Client remote wake orchestrator: tries LAN WoL first (1.5 s), falls back to Traverse `WakeRequester` using cached `WakeRelayBundle`, streams progress events to UI, hands off to normal Traverse `ClientTunnel` once `HostAwake`. |
| `client/protocol/desktop/src/hosts/wake_relay.rs` `[verify post-sprint code-map.md]` | `nightjar-desktop` | ~380 | Desktop opt-in LAN Wake Relay background worker (Windows/Linux/macOS; Linux-only code strictly `target_os = "linux"`), enrollment UI actions, live wake status feedback. |
| `client/protocol/android/src/wake_relay.rs` `[verify post-sprint code-map.md]` | `nightjar-android` | ~180 | JNI entry points driving `nightjar_core::wake_relay` from Android (requester-only; Android phones do NOT run as always-on relays due to Doze). |
| `client/app/src/main/java/com/limelight/computers/WakeRelayClient.java` `[verify post-sprint code-map.md]` | Android `app` | ~260 | Android away-wake runner + offline PC card progress state + plain-English failure dialogs. |
| `rust/web/src/components/network/WakeRelayPanel.svelte` `[verify post-sprint code-map.md]` | Console `rust/web` | ~290 | Console enrollment panel under Settings → Network (shows enrolled LAN relays, online status, Revoke / Rotate keys, and `--wake-relay` companion setup steps). |

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### Requester state machine (Nightjar client)

```text
Idle
  ──(User taps Wake on offline PC)──▶ TryLanWol (emit local broadcast if on private subnet, wait 1.5 s)
TryLanWol
  ├──(PC answers /serverinfo)──▶ TargetAwake
  ├──(no WakeRelayBundle stored)──▶ Failed(NoRelayEnrolled)
  └──(unreachable & bundle present)──▶ LookupRelayDht (query current + previous 3600s DHT windows, timeout 8s)
LookupRelayDht
  ├──(relay record found or cached endpoint valid)──▶ PunchRelay (timeout 10s)
  └──(not found / DHT unreachable)──▶ Failed(RelayOffline)
PunchRelay
  ├──(PunchAck received)──▶ SendSignedWake (sealed WakeRequest with fresh nonce + timestamp)
  └──(timeout)──▶ Failed(RelayOffline)
SendSignedWake
  ├──(WakeStatus::PacketSent / WaitingForBoot)──▶ WaitForTarget (poll target Traverse/LAN up to 45s)
  ├──(WakeStatus::HostAwake)──▶ TargetAwake (auto-connect Traverse tunnel)
  ├──(WakeStatus::RejectedAuth | RejectedReplay)──▶ Failed(AuthOrClockError)
  ├──(WakeStatus::RateLimited(s))──▶ Failed(RateLimited(s))
  └──(no reply after 3 sealed retries @ 1s)──▶ Failed(RelayUnreachable)
WaitForTarget
  ├──(target /serverinfo responds or WakeStatus::HostAwake)──▶ TargetAwake
  └──(45s timeout)──▶ Failed(TargetDidNotWake)
```

### Relay peer state machine (`WakeRelayPeer`)

```text
Disabled ──(explicit opt-in + >=1 enrolled target)──▶ Starting
Starting ──(STUN + DHT bootstrap)──▶ Publishing
Publishing ──(publish Role::WakeRelay record every 30m, STUN every 5m)──▶ Listening
Listening
  ├──(active stream running on this PC)──▶ ListeningQuiet (defers non-urgent STUN/DHT ticks off stream hot path; still answers incoming WakeRequest!)
  ├──(sealed WakeRequest received)──▶ Validate (AEAD -> epoch -> device_id -> Ed25519 -> timestamp ±120s -> replay cache -> rate limit)
  │      ├──(invalid AEAD / unknown tag)──▶ DropSilently (0 bytes sent)
  │      ├──(auth/replay/rate failure)──▶ Reply(WakeStatus::Rejected* / RateLimited)
  │      └──(valid)──▶ EmitLanWol (3 magic packets on target broadcasts) → Reply(PacketSent)
  │                     → ProbeTargetLocalHttp (every 2s up to 45s) → Reply(HostAwake | TimedOut)
  └──(revoked / disabled / shutdown)──▶ Disabled
```

### Reconnect, power transition & revocation matrix

| Scenario | Expected behavior |
| --- | --- |
| **Target wakes from S3 / Modern Standby** | Target host's existing `WakeGrace` (`rust/core/src/wake/grace.rs`, power request `GRACE_REASON`) holds the PC awake while the remote client establishes its `rubylight-traverse` tunnel and starts the stream, preventing immediate re-sleep before RTSP connects. |
| **Client switches Wi-Fi → 5G mid-wake** | `WakeRequester` retries with a **new** 16-byte nonce (reusing a nonce would fail replay check by design); if the target already booted, `ProbeTargetLocalHttp` answers `HostAwake` immediately without re-blasting WoL. |
| **Paired device unpaired on target host** | Target host removes the device from `h.paired`, increments `epoch` in `wake-relay.json`, pushes updated `epoch` to enrolled LAN relays over LAN sync (and includes `revoked_devices` list). Unpaired device's cached `WakeRelayBundle` is rejected with `RejectedAuth`. |
| **Relay PC restarts or loses power** | On reboot, service/app reloads `wake-relay.json`, runs STUN, republishes DHT record within ~5 s; remote clients seamlessly discover the new external port/session. |
| **Target PC's IP changes via DHCP on LAN** | Relay emits WoL to the target's physical MAC on the adapter's subnet-directed broadcast (MAC-based, independent of IP) and resolves the target's current local IP via mDNS + last-known IP fallback for the `HostAwake` probe. |
| **Both target AND relay behind symmetric NAT** | `rubylight-traverse` detects `Nat::Symmetric` on both sides and reports a clear plain-English network notice (*"Both networks use strict NAT, so direct relay wake could not connect"*), never spinning forever. |

### Enrollment durability & crash-restore journal

- **Durable store (`config_dir/wake-relay.json`):** Written atomically (`wake-relay.json.tmp` → `sync_all()` → `rename`) on every enrollment, revocation, or epoch rotation. Never stores raw TLS private keys — only scoped `relay_master_secret` bytes (OS ACL restricted to Administrators/SYSTEM on Windows service install, `0600` on Linux/macOS).
- **Zero unconsented OS/NIC edits:** Away Wake Relay NEVER modifies Windows NIC drivers or power settings on its own; NIC wake readiness continues to use Rubylight's existing consented `--apply-wake-settings` flow (`rust/host/src/wake.rs`).

## 5. Settings & 10/10 Craft Plan

### Fewer-settings compliance & explicit opt-in

- **Explicit opt-in, default off:**
  - Host Console: `away_wake_relay` (bool, default `false`), defined in `rust/core/src/config/settings30.rs` and `rust/web/src/lib/schema/extensions.ts` (plus `rust/host/src/console/settings.rs`) and surfaced under *Settings → Network* right below `internet_play`. Disabled/hinted when `internet_play` is off (since Traverse DHT is required). Two-sentence help: *"Allows another always-on Rubylight PC or desktop Nightjar on your home network to wake this PC from away without port forwarding. Uses encrypted peer-to-peer wake requests; nothing is sent to a third-party server."*
  - Desktop Nightjar: Single opt-in switch under *Settings → More settings → Streaming* (*Act as LAN wake relay*, default `false`). Hidden on Android phones/tablets/TVs because mobile Doze suspends background DHT maintenance.
- **Zero extra switches on the Offline PC card:** Nightjar's existing **Wake** action (`pcview_menu_send_wol` / short *"Wake"*) automatically tries LAN first and falls back to the enrolled Away Wake Relay seamlessly.

### Plain-English error states & UI rules

- **Relay offline:** *"Your wake relay (Living Room Mini-PC) is offline, so Gaming PC could not be woken from away. Turn on the relay device or wake Gaming PC from your home network."*
- **Target did not wake:** *"The wake signal reached your home network, but Gaming PC did not respond within 45 seconds. Check Wake-on-LAN readiness in Rubylight's console."*
- **No relay enrolled:** *"Gaming PC cannot be woken from outside your home network yet. Enroll an always-on Rubylight PC or desktop Nightjar on the same network."*
- **Auth revoked / clock skew:** *"The wake relay rejected this request. Check that your device clock is accurate, or reconnect on your home network to refresh wake access."*
- **Zero "Experimental" in UI:** Note that existing `rust/core/native-api/src/wake.rs` has an internal wire enum variant `TargetState::Experimental` for Wi-Fi WoL; user-facing UI MUST NEVER render the word `"Experimental"` — render *"Wi-Fi (depends on adapter and router)"* instead.
- **Zero stream-time impact / zero mod names:** Wake Relay never adds any stream-time overlay or blur, never names mods, uses Nightjar's dynamic input glyphs on the PC card Wake button (`A` on gamepad / `OK` on remote / `Enter` on keyboard, with **no button hints** shown for touch or mouse per `client/design/README.md:143`), and ships complete translations across all 15 Nightjar locales (`en`, `de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`, with all new Desktop translations routed to `client/protocol/desktop/src/strings_70/<code>.rs`) and all 22 host locales in `rust/assets/locale/`.

## 6. Disjoint Implementer Slices (Phase 3 Staged `parallel()`)

Executed through `orchestrator_core.py`'s staged slice runner with strictly non-overlapping file ownership. **Stage 1** (`15A` + `15B`) lands all Traverse protocol primitives, core policy, Native API v1 types, and shared registration files before **Stage 2** (`15C`, `15D`, `15E`) fans out in parallel.

### Stage 1 — Slice 15A: Traverse Wake Relay Protocol, Crypto & NAT Simulator *(parallel with 15B)*
- `client/protocol/traverse/src/lib.rs` `[verify post-sprint code-map.md]`
- `client/protocol/traverse/src/record.rs` *(add `Role::WakeRelay`, `Role::WakeRequester`)* `[verify post-sprint code-map.md]`
- `client/protocol/traverse/src/wire.rs` *(add `Kind::WakeRequest`, `Kind::WakeStatus`)* `[verify post-sprint code-map.md]`
- `client/protocol/traverse/src/wake_relay.rs` *(new)* `[verify post-sprint code-map.md]`
- `client/protocol/traverse/src/wake_relay_node.rs` *(new)* `[verify post-sprint code-map.md]`

### Stage 1 — Slice 15B: Core Policy, Native API v1 Types & Shared Registration *(parallel with 15A; lands before Stage 2)*
- `rust/core/native-api/src/lib.rs` *(export `pub mod wake_relay;` and `feature::WAKE_RELAY = "wake.relay"`)* `[verify post-sprint code-map.md]`
- `rust/core/native-api/src/wake_relay.rs` *(new)* `[verify post-sprint code-map.md]`
- `rust/core/native-api/tests/fixtures/wake_relay.json` *(new)* `[verify post-sprint code-map.md]`
- `rust/core/src/wake.rs` *(expose `pub mod relay;`)* `[verify post-sprint code-map.md]`
- `rust/core/src/wake/relay.rs` *(new)* `[verify post-sprint code-map.md]`
- `rust/core/src/extensions.rs` *(add `Offered::away_wake_relay` and `<AwayWakeRelayVersion>1</AwayWakeRelayVersion>`)* `[verify post-sprint code-map.md]`
- `rust/host/src/extensions.rs` *(populate `Offered::away_wake_relay` from `internet_play && away_wake_relay`)* `[verify post-sprint code-map.md]`
- `rust/core/src/config/settings30.rs` *(add `away_wake_relay` default `false` to `SETTINGS_30`, `LIVE`, and `Config::away_wake_relay()`, and update the 12-entry `there_are_12_unique_settings` / schema-sync tests)* `[verify post-sprint code-map.md]`
- `rust/web/src/lib/schema/extensions.ts` *(add `away_wake_relay` setting entry in lockstep with `settings30.rs`'s `the_console_schema_lists_exactly_these_settings_with_these_defaults` test)* `[verify post-sprint code-map.md]`
- `rust/host/src/native_api.rs` *(advertise `feature::WAKE_RELAY`, declare `mod wake_relay;`, and mount `/wake/relay`, `/wake/relay/enroll`, `/wake/relay/wake`; keep net LOC `<= 853`)* `[verify post-sprint code-map.md]`
- `rust/host/src/native_api/wake_relay.rs` *(new; Axum handlers backed by `butterpollo_core::wake::relay`)* `[verify post-sprint code-map.md]`
- `client/protocol/client/src/http/capabilities.rs` *(add `Feature::AwayWakeRelay` to `Feature`, `Feature::ALL`, `Evidence`, `with_native`; keep `<= 1257`)* `[verify post-sprint code-map.md]`
- `client/protocol/client/tests/capabilities_contract.rs` *(add `Feature::AwayWakeRelay` to exhaustive `feature_name` match)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java` *(add `AWAY_WAKE_RELAY(0)`; keep `<= 1347`)* `[verify post-sprint code-map.md]`
- `client/app/src/test/resources/protocol/capabilities/*` *(recorded fixtures for `AwayWakeRelayVersion` + `wake.relay`)* `[verify post-sprint code-map.md]`

### Stage 2 — Slice 15C: Host Relay Service, CLI & Native Integration Tests *(runs after Stage 1, parallel with 15D/15E)*
- `rust/host/src/wake_relay.rs` *(new)* `[verify post-sprint code-map.md]`
- `rust/host/src/internet_play.rs` *(2-line lifecycle hook)* `[verify post-sprint code-map.md]`
- `rust/host/src/main.rs` *(add `--wake-relay` CLI flag dispatch)* `[verify post-sprint code-map.md]`
- `rust/host/src/nvhttp/native_tests/wake_relay.rs` *(new)* `[verify post-sprint code-map.md]`

### Stage 2 — Slice 15D: Nightjar Engine & Desktop Relay/Requester *(runs after Stage 1, parallel with 15C/15E)*
- `client/protocol/client/src/wake_relay.rs` *(new)* `[verify post-sprint code-map.md]`
- `client/protocol/client/src/http/native.rs` `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/hosts/wake_relay.rs` *(new)* `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/hosts.rs` *(3-line hook)* `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/ui/settings.rs` *(1 opt-in row under Streaming)* `[verify post-sprint code-map.md]`
- `client/protocol/desktop/src/strings_70/<code>.rs` *(all new Desktop Wake Relay translations across the 15 locales, keeping grandfathered `strings/*.rs` untouched)* `[verify post-sprint code-map.md]`

### Stage 2 — Slice 15E: Nightjar Android Requester, Host Web Console & Docs *(runs after Stage 1, parallel with 15C/15D)*
- `client/protocol/android/src/wake_relay.rs` *(new)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/computers/WakeRelayClient.java` *(new)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/computers/HostActions.java` *(3-line hook)* `[verify post-sprint code-map.md]`
- `client/app/src/main/java/com/limelight/computers/ComputerManagerService.java` *(bundle storage hook)* `[verify post-sprint code-map.md]`
- `client/app/src/main/res/values*/strings.xml` `[verify post-sprint code-map.md]`
- `client/app/src/test/java/com/limelight/computers/WakeRelayClientTest.java` *(new)* `[verify post-sprint code-map.md]`
- `rust/web/src/components/network/WakeRelayPanel.svelte` *(new)* `[verify post-sprint code-map.md]`
- `rust/host/src/console/settings.rs` `[verify post-sprint code-map.md]`
- `rust/assets/locale/` *(host Wake Relay strings across all 22 host locale files)* `[verify post-sprint code-map.md]`
- `docs/internet-play.md`, `docs/native-api.md`, `docs/configuration.md` `[verify post-sprint code-map.md]`
- `rust/release/notes.d/7-0-away-wake-relay.md` *(new)* `[verify post-sprint code-map.md]`

## 7. Verification & Measurement Plan

### Narrowest unit, security, and NAT-simulator tests

1. **Sans-IO Traverse crypto & NAT simulator:** `cargo test -p rubylight-traverse wake_relay` — verifies:
   - Full wake flow through simulated Cone ↔ Cone and Cone ↔ Port-Restricted NATs (`sim.rs`) with zero network access.
   - **Security tests:** tampered Ed25519 signature rejected; wrong `device_id_hash` rejected; revoked `epoch` rejected; duplicate `nonce` rejected (`RejectedReplay`); clock skew `+121 s` / `-121 s` rejected; unauthenticated garbage UDP causes **0 bytes emitted** (zero amplification); `WakeStatus` wire length strictly `< WakeRequest` wire length; token bucket enforces 1 burst / 10 s and returns `RateLimited`.
2. **Core & Native API:** `cargo test -p butterpollo-core wake::relay`, `cargo test -p butterpollo-core config::settings30` (verifies updated `SETTINGS_30` count and lockstep `rust/web/src/lib/schema/extensions.ts` schema sync), `cargo test -p rubylight-native-api`, `cargo test -p butterpollo nvhttp::native_tests::wake_relay` (verifies `host_control` required for enroll, `launch` required for wake, non-LAN peer rejected on enroll, unpairing bumps epoch).
3. **Client contracts & Android tests:** `cargo test -p nightjar-core wake_relay`, `cargo test -p nightjar-core --test capabilities_contract`, `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.computers.WakeRelayClientTest`.
4. **Architecture & docs:** `python tools/arch_check.py`, `python tools/code_map.py check`, `cd rust/web && npm run check && node --test "*.test.mjs"`.

### Visual screenshot checks

- **Host Console (`screenshots.cjs`):** Network settings + `WakeRelayPanel` in enrolled, empty, and relay-offline states (Light & Dark, Desktop `1440×900` & Phone `390×844`).
- **Nightjar Desktop (`shot.rs`) & Android (`emulator-smoke.py`):** Offline PC card showing Wake progress (*"Contacting wake relay…"*, *"Wake signal sent…"*) and plain-English *"Relay offline"* sheet (Night, Day, High-Contrast, German).

### Serialized idle-gated hardware criteria

- **Idle gate:** Verify `http://127.0.0.1:47989/serverinfo` shows `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0` before any live test.
- **LAN companion relay verification:** Run isolated relay peer (`butterpollo.exe --config-dir C:\tests\wake-relay --wake-relay` on laptop `192.168.4.12` or isolated profile) enrolled against isolated target host profile; trigger signed `WakeRequest` over loopback/LAN Traverse socket, capture real 102-byte WoL magic packet on target subnet broadcast, verify `PacketSent` → `HostAwake` round-trip (`< 3 s` when target is already listening, `< 15 s` from S3 sleep when tested with user permission).
- **Zero streaming interference:** Run 90 s `rust/release/e2e.py` on isolated host (`--port 48126`) while `ListeningQuiet` relay mode is active; assert `present_to_sent p99` delta `<= +0.05 ms` and zero dropped frames.
- **Exit code 3 abort rule:** Exit code `3` from `e2e.py` or `check.ps1` indicates an AMD encoder hang — **abort immediately and investigate; never retry**.
- **GPU/OS note:** Wake Relay is encoder-agnostic (AMD AMF, NVIDIA NVENC, Win10, Linux, macOS); no NVIDIA hardware claim is made.
