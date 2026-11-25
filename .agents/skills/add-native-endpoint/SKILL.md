---
name: add-native-endpoint
description: Add an additive v1 Native API endpoint (/api/native/v1/...) across rubylight-native-api, butterpollo-session, butterpollo-web, tests, docs, and Nightjar.
---

# Add a Native API v1 Endpoint

Use this skill when adding a paired-client HTTPS endpoint under `/api/native/v1`. All changes within v1 MUST be strictly additive and land with Nightjar capability gating.

## Contract invariants (`docs/native-api.md`)

- **Auth & transport:** Served on the streaming HTTPS port (`HttpsPort`, typically 47984) using the client's paired TLS certificate (`caller(&h, &c)?`). Never use admin console auth, cookies, or API tokens.
- **Live permissions:** Check the caller's permissions and Host control flag on EVERY request (`require`, `require_host_control`, `require_view_or_launch` from `rust/host/stream/src/session_actions.rs`, re-exported by `rust/host/web/src/native_api.rs`); never cache permissions from launch.
- **Additive v1 wire rules:**
  - Response types ignore unknown fields; enums use `#[serde(rename_all = "snake_case")]` and `#[serde(other)] Unknown`.
  - Request types use `#[serde(deny_unknown_fields)]` so typos fail loudly; POST bodies cap at `MAX_BODY` (16 KiB) and accept `application/json`, `text/plain`, or empty (`{}`).
  - Responses always return `application/json; charset=utf-8` with `Cache-Control: no-store`.
- **Gate on capabilities, never host brand:** Announce a feature string in `rubylight_native_api::feature` and read it in Nightjar's `HostCapabilities`.

## Step-by-step checklist

1. **Scaffold wire types, fixture, handler, and parity test:**
   ```powershell
   python tools/scaffold.py native-endpoint <name> --method get --permission list --dry-run
   python tools/scaffold.py native-endpoint <name> --method get --permission list
   ```
2. **Wire types (`client/protocol/native-api/`):**
   - Implement types in `client/protocol/native-api/src/<name>.rs` and export `pub mod <name>;` in `client/protocol/native-api/src/lib.rs`.
   - Add a `feature::*` constant in `client/protocol/native-api/src/lib.rs` if the endpoint is conditionally advertised.
   - Keep fixture JSON in `client/protocol/native-api/tests/fixtures/<name>.json` with LF line endings (`.gitattributes` enforces byte comparison).
3. **Pure state projection (`rust/core/session/`):**
   - Put pure projection/validation logic in `rust/core/session/src/native.rs` (or a submodule under `rust/core/session/src/native/`) so it is unit-tested without HTTP or Windows APIs.
4. **Host route & handler (`rust/host/web/`):**
   - Implement the handler in `rust/host/web/src/native_api/<name>.rs` (wrap blocking I/O in `tokio::task::spawn_blocking`).
   - Register the module and route in `router` in `rust/host/web/src/native_api.rs`, and include the feature string in its `capabilities` handler (`/capabilities`) when enabled.
5. **Host parity tests & contract doc:**
   - Register `mod <name>;` and verify fixture match, `Cache-Control: no-store`, and `403 permission_required` in `tests/host/tests/nvhttp_parity/native_tests/<name>.rs`.
   - Document request, response, permissions, and errors in `docs/native-api.md`.
6. **Nightjar client (same change):**
   - Add the `Feature` variant in `client/protocol/policy/src/http/capabilities/types.rs`, set it from the feature string in `client/protocol/policy/src/http/capabilities.rs` (`with_native`), name it in `client/protocol/android/src/capabilities.rs` (`feature_key`) and Java `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java`.
   - Add HTTP client method in `client/protocol/client/src/http/native.rs`, then wire Desktop (`client/protocol/desktop/src/hosts/native.rs`) and Android (`client/app/src/main/java/com/limelight/nativeapi/`).

## Verification

```powershell
$env:TEMP="D:\t"; $env:TMP="D:\t"
python tools/build_lock.py --lane cargo -- cargo test --manifest-path client/protocol/Cargo.toml -p rubylight-native-api -p nightjar-policy -p nightjar-core --locked
python tools/build_lock.py --lane cargo -- cargo test -p butterpollo-session -p butterpollo-web --locked
python tools/build_lock.py --lane cargo -- cargo test -p butterpollo-host-tests --test nvhttp_parity --locked
python tools/arch_check.py
python tools/code_map.py check
git diff --check
```
