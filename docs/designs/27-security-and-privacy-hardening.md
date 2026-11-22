# Item 27: Host Network, WebRTC & API Security Hardening + Support-Bundle Privacy Sanitization (Wave 1)

Status: engineering design (Phase 1). Written against `origin/main` `995f2540c8` (`6.3.0`) and [`00-protocol-reservations.md`](00-protocol-reservations.md).

## 1. Summary & User Experience

Rubylight's zero-account, local-first architecture is one of its greatest strengths over cloud-account streaming hosts. Item 27 closes seven concrete, code-verified security and privacy gaps across WebRTC signaling, reverse-proxy authentication boundaries, CSRF/pairing/probe rate limits, Windows UNC/NTLM exposure, and host/desktop diagnostic support bundles — with **zero wire-protocol changes**, **zero latency added to any streaming path**, and **zero new user-facing settings**.

### 1.1 Five Concrete Hardening Pillars

1. **WebRTC Signaling Hardening (`rust/host/src/webrtc/server.rs`):**
   - Enforce strict `Origin` validation on `webrtc_port` upgrades (matching `web.rs:453-459,554-560,662-668`, closing Cross-Site WebSocket Hijacking / CSWSH) and prefer TLS console signaling (`/api/webrtc/signal`) whenever available (restricting plain `ws://` `webrtc_port` to explicit opt-in or loopback-only).
   - Never transmit a permanent paired device `secret` over plaintext `ws://` (`webrtc/server.rs:1158-1161` requires `wss://` or an ephemeral single-use pairing handshake).
   - Omit host hardware MAC address from unauthenticated `auth-required` replies (`webrtc/server.rs:749-752`) unless the peer is an already-paired or same-origin LAN session, and add a 30 s per-peer tray-notification cooldown (`:1089-1092`).
2. **Reverse-Proxy-Aware Local Trust & Auth Rate Limiting (`rust/host/src/web.rs`):**
   - Route `SignIns::key` (`web.rs:85-89`), `fresh_password` setup (`web.rs:845-850`), and `/api/auth/login` through the existing `network::browser_on_this_pc(peer, local, headers)` guard (`rust/host/src/network.rs:56-96`) so requests forwarded through a local reverse proxy (`nginx`, `cloudflared`, `tailscale serve` carrying `Forwarded`/`X-Forwarded-*` or external `Host`/`Origin`) **never** inherit loopback trust or bypass sign-in brute-force rate limits.
   - Require `__Host-apollo_anon_csrf` OR strict `Origin` + `Sec-Fetch-Site: same-origin` on `POST /api/auth/login` and `POST /api/auth/refresh` (removing `/api/auth/refresh` from CSRF-exempt `public` handling at `web.rs:835,909`).
3. **Pairing, IPv6 `/64` Rate-Limit Parity & Bandwidth-Probe Guard (`nvhttp/pairing.rs`, `nvhttp/probe.rs`):**
   - Key `nvhttp/pairing.rs` rate limits and a 30 s per-peer pairing tray-popup throttle by IPv6 `/64` prefix (matching `web.rs:91` and `webrtc/clients.rs:86`) so an IPv6 neighbor cannot rotate host bits to bypass brute-force limits or spam Windows notifications.
   - Guard `/pyrowave-bandwidth-probe` (`rust/host/src/nvhttp/probe.rs:5-24`, which serves 32 MiB) with `launch` permission and the identical active-stream busy lock + rate limit already enforced on `/bandwidth-probe` (`probe.rs:50-78`).
4. **Windows UNC/SMB NTLM Protection & API-Token Least Privilege (`browse.rs`, `catalog.rs`, `web.rs`):**
   - Reject Windows UNC paths (`\\...`, `//...`, `\\?\UNC\...`) by default in `rust/core/src/browse.rs:48-97` and `rust/core/src/catalog.rs:65-97` so malformed or malicious console/API path probes can never trigger outbound SMB/NTLMv2 credential authentication.
   - Exclude high-privilege account/filesystem routes (`/api/password`, `/api/browse`) from default API-token scopes (`web.rs:210-296`).
5. **One-Click Privacy-Safe Anonymized Support Bundles — Host & Nightjar Desktop (`butterpollo-core::log_redact`, `maintenance.rs`, `report/redact.rs`, `logfile.rs`):**
   - Create a canonical `<= 400`-LOC `rust/core/src/log_redact.rs` used by `rust/host/src/maintenance.rs::bundle_with_dump()` to stream-redact WAN/LAN/peer IPs, MAC addresses, PINs/OTPs, bearer/session tokens, Windows user profile paths (`C:\Users\<name>`), client device names, `sunshine_name`, `bind_address`, and `apps.json` game titles (`[app-1]`, `[client-1]`) across `butterpollo.log`, `butterpollo.log.1`, `service.log`, `panic.txt`, and `diagnostics.json`.
   - Fold safe read-only environment facts (matching `rust/tests/collect_environment.ps1`) + `log_analysis` + Stream Doctor 2.0 summary into `diagnostics.json`, exclude raw up-to-512 MiB `.dmp` memory minidumps from default support bundles (offering a separate explicit unchecked opt-in **Include crash memory dump** export with a clear plain-English privacy warning), and add a 1-click **Copy GitHub issue summary** button in Console `/help`.
   - Extend shared Nightjar `client/protocol/client/src/report/redact.rs` (currently Android-only paths at line 68) to scrub Windows `C:\Users\<name>`, macOS `/Users/<name>`, and Linux `/home/<name>`, and upgrade Nightjar Desktop `Command::ReportProblem` (`logfile.rs`) to emit a compact redacted `nightjar-support.zip` (logs + GPU/decoder/fast-path capabilities + `.dmp` metadata, never raw heap dumps by default).

---

## 2. Protocol, Native API & Capability Contract

Per [`00-protocol-reservations.md`](00-protocol-reservations.md) §7, Item 27 requires **ZERO wire-protocol or Native API breaking changes**:

| Surface | Contract |
| --- | --- |
| `PROTOCOL_VERSION` | **Unchanged.** Stock Moonlight and older Nightjar clients pair and stream cleanly. |
| `rubylight_native_api::API_VERSION` | **Unchanged (`1`).** Zero new routes required. |
| Web Console `/api/maintenance/support-bundle` | Keeps existing route; adds strictly optional `?include_dump=1` query flag (default `false` — safe redacted bundle without raw `.dmp`). |
| Compatibility | Preserves LAN pairing, Internet Play via `rubylight-traverse`, Windows 10 & 11, AMD AMF, NVIDIA NVENC, and reverse-proxy setups (proxies simply authenticate normally instead of falsely bypassing loopback limits). |

---

## 3. Architecture & <=800-LOC Module Plan

All repo paths carry `[verify post-sprint code-map.md]` for Phase 2 reconciliation. Every new module stays strictly `<= 800` LOC with `//!` headers; grandfathered files (`web.rs`, `webrtc/server.rs`, `maintenance.rs`) stay at or below their `tools/size_ratchet.json` ceilings.

| Path | Status | Est. LOC | Responsibility |
| --- | --- | ---: | --- |
| `rust/core/src/log_redact.rs` | New | ~360 | `//!` Deterministic streaming redactor for host logs & `diagnostics.json` (IPv4/IPv6, MACs, tokens/PINs, `C:\Users\*`, client/app aliases) + comprehensive unit tests. |
| `rust/core/src/browse.rs`, `rust/core/src/catalog.rs` | Modify | existing | Reject Windows UNC paths (`\\`, `//`, `\\?\UNC\`) before filesystem metadata calls + unit tests. |
| `rust/host/src/web/security_hardening.rs` | New | ~260 | `//!` Proxy-aware `browser_on_this_pc` sign-in/setup key derivation, `Origin`+`Sec-Fetch-Site` CSRF guard for `/api/auth/{login,refresh}`, and least-privilege API token scope filter + negative tests. |
| `rust/host/src/web.rs` | Modify | `<= 1731` | Wire `web/security_hardening.rs` cleanly without growing `web.rs`. |
| `rust/host/src/webrtc/origin_guard.rs` | New | ~220 | `//!` WebRTC `Origin` validation, `wss://`-only permanent-secret guard, unauthenticated MAC redaction, and 30 s per-peer tray cooldown + negative tests. |
| `rust/host/src/webrtc/server.rs` | Modify | `<= 1322` | Delegate security checks to `webrtc/origin_guard.rs` (net shrink). |
| `rust/host/src/nvhttp/pairing.rs`, `rust/host/src/nvhttp/probe.rs` | Modify | existing | IPv6 `/64` rate-limit key + 30 s tray cooldown; guard `/pyrowave-bandwidth-probe` with `launch` permission + active-stream busy lock. |
| `rust/host/src/maintenance.rs` & `rust/host/src/maintenance/support_zip.rs` | Modify / New | `<= 650` / ~320 | Stream logs through `butterpollo_core::log_redact`, include safe `collect_environment` facts, default-exclude `.dmp` unless `include_dump=1`. |
| `rust/web/src/components/help/SupportBundlePanel.svelte` | Modify | `<= 250` | Add unchecked **Include crash memory dump (may contain sensitive memory)** opt-in + **Copy GitHub issue summary** button. |
| `client/protocol/client/src/report/redact.rs`, `client/protocol/desktop/src/logfile.rs` & `support_zip.rs` | Modify / New | existing / ~260 | Scrub Windows/macOS/Linux user paths across shared redactor + Desktop `nightjar-support.zip` export with redacted logs and GPU/decoder metadata. |

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

- **Reverse-Proxy Auth Boundary:** `peer.is_loopback() && !has_proxy_headers(headers) && local_host_header(headers)` -> `LocalDirect` (exempt from remote brute-force lockouts only where intentional); ANY proxy header (`Forwarded`, `X-Forwarded-For`, `X-Forwarded-Host`, `X-Forwarded-Proto`) -> `ProxiedRemote` (enforces full `SignIns` rate limiting, CSRF, and blocks `fresh_password` local shortcut).
- **WebRTC Signaling Boundary:** `ws://` plaintext -> forbids permanent device `secret` emission and cross-origin upgrades; `wss://` (or `/api/webrtc/signal`) + matching `Origin` -> full pairing/play/guest signaling.
- **Support Bundle Privacy Invariant:** Default export (`include_dump=false`) MUST contain `0` raw public/LAN IPs, `0` MACs, `0` user profile names, `0` raw game titles, `0` PIN/secret tokens, and `0` `.dmp` files — verified automatically by regression tests over synthetic leak fixtures.
- **Crash-Restore:** Item 27 mutates zero host OS display/audio/driver state.

---

## 5. Settings & 10/10 Craft Plan

- **Zero new settings** in Rubylight or Nightjar (all hardening is automatic by default; raw crash dump inclusion is an explicit per-export unchecked checkbox with a 1-sentence privacy note).
- **Plain-English refusals:** Blocked cross-origin WebRTC upgrades, UNC paths, rate-limited pairing bursts, and busy bandwidth probes return clear plain-English errors (zero stack traces).
- **Zero `"Experimental"` & zero mod names** in any new UI copy across all 22 host and 15 Nightjar locales.

---

## 6. Disjoint Implementer Slices (Phase 3 `parallel()`)

| Slice | Stage | Exclusive Owned Files (Zero Overlap) |
| --- | --- | --- |
| **27-A: Core Redactor & UNC Guard** | **Stage 1** | `rust/core/src/log_redact.rs` (new), `rust/core/src/lib.rs`, `rust/core/src/browse.rs`, `rust/core/src/catalog.rs` `[verify post-sprint code-map.md]` |
| **27-B: Host Web, Pairing, Probe & WebRTC Hardening** | **Stage 2** (parallel with 27-C, 27-D) | `rust/host/src/web/security_hardening.rs` (new), `rust/host/src/web.rs`, `rust/host/src/webrtc/origin_guard.rs` (new), `rust/host/src/webrtc/server.rs`, `rust/host/src/nvhttp/pairing.rs`, `rust/host/src/nvhttp/probe.rs` `[verify post-sprint code-map.md]` |
| **27-C: Host Anonymized Support Bundle & Console UI** | **Stage 2** (parallel with 27-B, 27-D) | `rust/host/src/maintenance/support_zip.rs` (new), `rust/host/src/maintenance.rs`, `rust/web/src/components/help/SupportBundlePanel.svelte`, `rust/assets/locale/*.json` (22 host locales) `[verify post-sprint code-map.md]` |
| **27-D: Nightjar Shared Path Redactor & Desktop Support ZIP** | **Stage 2** (parallel with 27-B, 27-C) | `client/protocol/client/src/report/redact.rs`, `client/protocol/desktop/src/logfile.rs`, `client/protocol/desktop/src/support_zip.rs` (new), `client/protocol/desktop/src/app/command.rs`, `client/protocol/desktop/src/strings_70/*.rs` `[verify post-sprint code-map.md]` |

---

## 7. Verification & Measurement Plan

1. **Automated Security Negative Tests (No GPU Required):**
   - `cargo test -p butterpollo-core --locked log_redact browse catalog` — asserts UNC rejection (`\\server\share`, `//10.0.0.1/c$`, `\\?\UNC\...`) and zero leaked IP/MAC/username/token/game-title substrings across synthetic log fixtures.
   - `cargo test -p butterpollo --locked security_hardening origin_guard pairing probe support_zip` — proves: (a) cross-origin WebRTC `ws://` upgrade returns `403`, plaintext `ws://` never emits permanent `secret`, unauthenticated `auth-required` omits MAC; (b) `127.0.0.1` + `X-Forwarded-For` enforces `SignIns` rate limit and rejects `fresh_password`; (c) `POST /api/auth/login` & `/api/auth/refresh` without valid CSRF/same-origin fail `403`; (d) IPv6 `/64` rotation shares one pairing rate-limit bucket; (e) `/pyrowave-bandwidth-probe` fails `403` without `launch` and `409/503` during an active stream; (f) default `butterpollo-support.zip` contains zero `.dmp` files and zero unredacted secrets/IPs.
   - `cargo test -p nightjar-core --locked report::redact` & `cargo test -p nightjar-desktop --locked support_zip` — proves `C:\Users\Alice`, `/Users/alice`, and `/home/alice` redaction and compact Desktop ZIP assembly.
2. **Guardrails:** `python tools/arch_check.py`, `python tools/code_map.py check`, `cargo fmt -- --check` (without `--all`). Zero hardware benchmark needed (non-stream-path security & diagnostics).
