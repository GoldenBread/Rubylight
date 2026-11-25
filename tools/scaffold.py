"""Dry-run-capable boilerplate generators matching Rubylight and Nightjar repo conventions.

Generates LF-only files with `//!` module headers and <=800 LOC compliance (`tools/arch_check.py`):
  - control-message: `no_std` wire module in `client/protocol/core/src/<name>.rs` (checks `ids.rs`)
  - native-endpoint: additive v1 types, LF fixture, Axum handler, and nvhttp_parity test
  - setting:         host setting snippets (Config accessor, web schema, console row, i18n spec, note)
  - test-scenario:   focused scenario test module for reconnect, native, config, or interop suites

Usage:
  python tools/scaffold.py control-message NAME --id 0x55XX [--summary TEXT] [--dry-run]
  python tools/scaffold.py native-endpoint NAME [--method get|post] [--permission list|view|launch|host_control] [--dry-run]
  python tools/scaffold.py setting KEY --type bool|int|choice|text --default VAL --label TEXT --description TEXT [--dry-run]
  python tools/scaffold.py test-scenario NAME [--suite reconnect|native|config|interop] [--summary TEXT] [--dry-run]
"""

import argparse
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
SNAKE_RE = re.compile(r'^[a-z][a-z0-9_]*$')
HEX_U16_RE = re.compile(r'^0x[0-9a-fA-F]{1,4}$')


def validate_snake(name, label='name'):
    if not SNAKE_RE.fullmatch(name):
        raise ValueError(f'{label} must be snake_case ([a-z][a-z0-9_]*): {name!r}')
    return name


def to_pascal(snake):
    return ''.join(part.capitalize() for part in snake.split('_') if part)


def to_upper(snake):
    return snake.upper()


def parse_hex_u16(raw):
    text = raw.strip()
    if not HEX_U16_RE.fullmatch(text):
        raise ValueError(f'message id must be hex u16 like 0x5547: {raw!r}')
    value = int(text, 16)
    return f'0x{value:04x}', value


def existing_control_ids(root):
    """Return set of integer u16 control IDs already declared in client/protocol/core/src/ids.rs."""
    ids_path = root / 'client/protocol/core/src/ids.rs'
    if not ids_path.is_file():
        return set()
    text = ids_path.read_text(encoding='utf-8')
    found = set(int(m, 16) for m in re.findall(r'\b0x[0-9a-fA-F]{4}\b', text))
    # Also scan *_MESSAGE_TYPE constants inside client/protocol/core/src/*.rs
    core_dir = root / 'client/protocol/core/src'
    for p in core_dir.glob('*.rs'):
        for m in re.findall(r'MESSAGE_TYPE\s*:\s*u16\s*=\s*(0x[0-9a-fA-F]{4})', p.read_text(encoding='utf-8')):
            found.add(int(m, 16))
    return found


def write_plan(root, files, dry_run=False, force=False):
    """Write {rel_posix_path: content} with LF endings, refusing overwrites unless force."""
    conflicts = [rel for rel in files if (root / rel).exists() and not force]
    if conflicts:
        raise FileExistsError('refusing to overwrite existing file(s) without --force: ' + ', '.join(conflicts))
    if not dry_run:
        for rel, content in files.items():
            target = root / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content.replace('\r\n', '\n'), encoding='utf-8', newline='\n')
    return sorted(files.keys())


# ---- 1. Control message generator ----

def generate_control_message(name, msg_id_hex, summary='', launch_param=''):
    validate_snake(name)
    upper = to_upper(name)
    pascal = to_pascal(name)
    doc = summary.strip() or f'{pascal} control-stream message ({msg_id_hex}).'
    launch_const = (
        f'/// `/launch` and `/resume` query parameter requesting `{name}`.\n'
        f'pub const LAUNCH_PARAMETER: &str = "{launch_param}";\n'
        if launch_param
        else ''
    )
    source = f"""//! {doc}
//!
//! Frame, little-endian: version u8 ([`VERSION`]), flags u8, payload u32. Trailing bytes are
//! ignored so newer peers can append fields without bumping the version.

/// Control-stream message type (`client/protocol/core/src/ids.rs`).
pub const {upper}_MESSAGE_TYPE: u16 = {msg_id_hex};
{launch_const}/// The frame version this crate reads and writes.
pub const VERSION: u8 = 1;
/// Size of a v1 frame on the wire.
pub const FRAME_BYTES: usize = 6;

/// Wire payload for `{name}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct {pascal} {{
    pub flags: u8,
    pub value: u32,
}}

impl {pascal} {{
    /// Encodes the frame in little-endian wire order.
    pub fn encode(&self) -> [u8; FRAME_BYTES] {{
        let mut out = [0u8; FRAME_BYTES];
        out[0] = VERSION;
        out[1] = self.flags;
        out[2..6].copy_from_slice(&self.value.to_le_bytes());
        out
    }}

    /// Decodes a v1 frame, ignoring trailing bytes for forward compatibility.
    pub fn decode(data: &[u8]) -> Option<Self> {{
        if data.len() < FRAME_BYTES || data[0] != VERSION {{
            return None;
        }}
        let value = u32::from_le_bytes([data[2], data[3], data[4], data[5]]);
        Some(Self {{
            flags: data[1],
            value,
        }})
    }}
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn round_trip_and_trailing_bytes() {{
        let frame = {pascal} {{ flags: 1, value: 42 }};
        let encoded = frame.encode();
        assert_eq!({pascal}::decode(&encoded), Some(frame));
        let mut extended = encoded.to_vec();
        extended.extend_from_slice(&[0xaa, 0xbb]);
        assert_eq!({pascal}::decode(&extended), Some(frame));
    }}

    #[test]
    fn rejects_short_or_unknown_version() {{
        assert_eq!({pascal}::decode(&[VERSION, 0, 0]), None);
        let mut bad = {pascal} {{ flags: 0, value: 0 }}.encode();
        bad[0] = VERSION + 1;
        assert_eq!({pascal}::decode(&bad), None);
    }}
}}
"""
    files = {f'client/protocol/core/src/{name}.rs': source}
    checklist = [
        f'1. Export `pub mod {name};` in client/protocol/core/src/lib.rs',
        f'2. Reserve `({name}::{upper}_MESSAGE_TYPE, "{name.replace("_", " ")}")` in client/protocol/core/src/ids.rs',
        '3. Offer it in rust/core/media-policy/src/extensions.rs (Offered) & rust/host/stream/src/extensions.rs',
        '4. Dispatch it in a family handler under rust/host/stream/src/stream/control/ (chained in event_loop.rs; a stateful one gets its own worker module)',
        '5. Wire Nightjar in client/protocol/client/src/control/{messages.rs,events.rs,session.rs} + Android/Desktop',
        '6. Document in docs/control-messages.md and check tests/host/tests/reconnect_tests/extension_tests.rs',
    ]
    return files, checklist


# ---- 2. Native API v1 endpoint generator ----

PERM_EXPRS = {
    'list': ('LIST', 'require(&client, LIST, "List applications")?;', 'LIST'),
    'view': ('VIEW', 'require(&client, VIEW, "View streams")?;', 'VIEW_BIT'),
    'launch': ('LAUNCH', 'require(&client, LAUNCH, "Launch applications")?;', 'LAUNCH'),
    'host_control': ('LIST', 'require_host_control(&client)?;', 'LIST'),
}


def generate_native_endpoint(name, method='get', route_path='', permission='list', summary=''):
    validate_snake(name)
    method = method.lower()
    if method not in ('get', 'post'):
        raise ValueError(f'unsupported method: {method!r}')
    if permission not in PERM_EXPRS:
        raise ValueError(f'unsupported permission: {permission!r}')
    pascal = to_pascal(name)
    upper = to_upper(name)
    path = route_path or f'/{name.replace("_", "-")}'
    if not path.startswith('/'):
        path = '/' + path
    doc = summary.strip() or f'{method.upper()} {path}: native API v1 {name.replace("_", " ")} endpoint.'
    perm_import, perm_check, test_perm = PERM_EXPRS[permission]

    request_block = ''
    if method == 'post':
        request_block = f"""
/// Request body for `POST {path}`. Rejects unknown fields so typos fail loudly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct {pascal}Request {{
    pub enabled: bool,
}}
"""

    types_rs = f"""//! `{method.upper()} {path}`: {doc}
use serde::{{Deserialize, Serialize}};

/// Status reported by `{path}`. Unknown values from newer hosts decode as `Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum {pascal}State {{
    Ready,
    Unavailable,
    #[serde(other)]
    Unknown,
}}
{request_block}
/// Response body for `{method.upper()} {path}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct {pascal}Response {{
    pub revision: u64,
    pub state: {pascal}State,
}}
"""

    fixture_json = json.dumps({'revision': 1, 'state': 'ready'}, indent=2) + '\n'

    super_perm = 'require_host_control' if permission == 'host_control' else 'require'
    native_perm_use = '' if permission == 'host_control' else f'use butterpollo_session::native::{perm_import};\n'

    if method == 'get':
        handler_rs = f"""//! `{method.upper()} {path}` handler (`docs/native-api.md`).
use super::{{Reply, caller, json, {super_perm}}};
use axum::{{Extension, extract::State, http::StatusCode}};
{native_perm_use}use butterpollo_stream::{{state::Shared, tls::Connection}};
use rubylight_native_api::{name}::{{{pascal}Response, {pascal}State}};

pub async fn read(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {{
    let client = caller(&h, &c)?;
    {perm_check}
    let body = {pascal}Response {{
        revision: 1,
        state: {pascal}State::Ready,
    }};
    Ok(json(StatusCode::OK, &body))
}}
"""
    else:
        handler_rs = f"""//! `{method.upper()} {path}` handler (`docs/native-api.md`).
use super::{{Failure, Reply, caller, json, read_body, {super_perm}}};
use axum::{{Extension, extract::{{Request, State}}, http::StatusCode}};
{native_perm_use}use butterpollo_stream::{{state::Shared, tls::Connection}};
use rubylight_native_api::{{
    error::ErrorCode,
    {name}::{{{pascal}Request, {pascal}Response, {pascal}State}},
}};

pub async fn write(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {{
    let client = caller(&h, &c)?;
    {perm_check}
    let body = read_body(request).await?;
    let req: {pascal}Request = serde_json::from_slice(&body).map_err(|error| {{
        Failure::new(
            ErrorCode::InvalidRequest,
            format!("The body is not a valid {name} request: {{error}}."),
        )
    }})?;
    let response = {pascal}Response {{
        revision: 1,
        state: if req.enabled {{
            {pascal}State::Ready
        }} else {{
            {pascal}State::Unavailable
        }},
    }};
    Ok(json(StatusCode::OK, &response))
}}
"""

    if method == 'get':
        call_expr = f's.get("{path}").await'
        denied_expr = f's.get("{path}").await'
    else:
        call_expr = (
            f'send(&s.f, true, &s.connection, "POST", &format!("{{PREFIX}}{path}"), '
            f'Some("application/json"), &serde_json::json!({{"enabled": true}}).to_string()).await'
        )
        denied_expr = (
            f'send(&s.f, true, &s.connection, "POST", &format!("{{PREFIX}}{path}"), '
            f'Some("application/json"), &serde_json::json!({{"enabled": true}}).to_string()).await'
        )

    if permission == 'host_control':
        fixture_setup = f'    let s = Setup::with({test_perm});\n    s.host_control(true);'
        denied_setup = '    // Paired with List, but without Host control.\n    let s = Setup::with(LIST);'
        denied_code = 'host_control_required'
    else:
        fixture_setup = f'    let s = Setup::with({test_perm});'
        denied_setup = '    // Paired with input-only bit (no List/View/Launch).\n    let s = Setup::with(1);'
        denied_code = 'permission_required'

    test_rs = f"""//! `{method.upper()} {path}` parity and permission checks against the documented fixture.
use super::*;
use rubylight_native_api::{name}::{{{pascal}Response, {pascal}State}};

const FIXTURE: &str =
    include_str!("../../../../../client/protocol/native-api/tests/fixtures/{name}.json");

#[tokio::test]
async fn {name}_matches_documented_fixture() {{
{fixture_setup}
    let reply = {call_expr};
    assert_eq!(reply.status, StatusCode::OK, "{{}}", reply.text());
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
    let parsed: {pascal}Response = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(parsed.state, {pascal}State::Ready);
    let expected: Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(reply.json(), expected);
}}

#[tokio::test]
async fn {name}_enforces_caller_permissions() {{
{denied_setup}
    {denied_expr}
        .assert_error(403, "{denied_code}");
}}
"""

    files = {
        f'client/protocol/native-api/src/{name}.rs': types_rs,
        f'client/protocol/native-api/tests/fixtures/{name}.json': fixture_json,
        f'rust/host/web/src/native_api/{name}.rs': handler_rs,
        f'tests/host/tests/nvhttp_parity/native_tests/{name}.rs': test_rs,
    }
    checklist = [
        f'1. Export `pub mod {name};` and `feature::{upper}` in client/protocol/native-api/src/lib.rs',
        f'2. Register `mod {name};` and `.route(&path("{path}"), {method}({name}::{"read" if method == "get" else "write"}).fallback(unknown))` in rust/host/web/src/native_api.rs',
        f'3. Add `mod {name};` in tests/host/tests/nvhttp_parity/native_tests.rs (or parity module)',
        '4. Document the endpoint contract in docs/native-api.md',
        '5. Wire capability (policy/src/http/capabilities/types.rs Feature) + HTTP client (client/src/http/native.rs) in Nightjar, Android + Desktop',
    ]
    return files, checklist


# ---- 3. Host setting stub generator ----

# Valid `CategoryId` values from `rust/web/src/lib/settings-types.ts`, mapped to their schema file.
CATEGORY_TO_SCHEMA_FILE = {
    'general': 'basics.ts',
    'network': 'basics.ts',
    'audio': 'basics.ts',
    'input': 'basics.ts',
    'commands': 'basics.ts',
    'advanced': 'basics.ts',
    'video': 'video.ts',
    'encoders': 'video.ts',
    'hdr': 'video.ts',
    'display': 'display.ts',
    'frame-limiting': 'display.ts',
    'library': 'library.ts',
}


def generate_setting_stub(
    key, kind, default, label, description, category='video', group='General',
    choices=(), min_val=None, max_val=None, extension=False,
):
    validate_snake(key, 'setting key')
    if category not in CATEGORY_TO_SCHEMA_FILE:
        raise ValueError(
            f'invalid CategoryId {category!r}; expected one of: {", ".join(CATEGORY_TO_SCHEMA_FILE)}'
        )
    schema_file = 'extensions.ts' if extension else CATEGORY_TO_SCHEMA_FILE[category]
    if kind == 'bool':
        if default.lower() not in ('true', 'false'):
            raise ValueError(f'bool setting default must be true or false, got {default!r}')
        rust_ret = 'bool'
        rust_body = f'self.boolean("{key}", {default.lower()})'
        ts_default = default.lower()
        ts_control = "{ kind: 'toggle' }"
        console_ctrl = 'Bool'
    elif kind == 'int':
        int_def = int(default)
        if min_val is not None and int_def < min_val:
            raise ValueError(f'default {int_def} < min {min_val}')
        if max_val is not None and int_def > max_val:
            raise ValueError(f'default {int_def} > max {max_val}')
        rust_ret = 'i64'
        if min_val is not None and max_val is not None:
            rust_body = f'self.integer("{key}", {int_def}).clamp({min_val}, {max_val})'
        else:
            rust_body = f'self.integer("{key}", {int_def})'
        ts_default = str(int_def)
        parts = ["kind: 'number'"]
        if min_val is not None:
            parts.append(f'min: {min_val}')
        if max_val is not None:
            parts.append(f'max: {max_val}')
        ts_control = '{ ' + ', '.join(parts) + ' }'
        console_ctrl = 'Number'
    elif kind == 'choice':
        if not choices:
            raise ValueError('--choices is required for --type choice')
        if default not in choices:
            raise ValueError(f'choice default {default!r} must be one of {list(choices)}')
        rust_ret = '&str'
        rust_body = f'self.get("{key}", "{default}")'
        ts_default = json.dumps(default)
        opts = ', '.join(f"{{ value: {json.dumps(c)}, label: {json.dumps(c.replace('_', ' ').title())} }}" for c in choices)
        ts_control = f"{{ kind: 'select', options: [{opts}] }}"
        ch_list = ', '.join(json.dumps(c) for c in choices)
        console_ctrl = f'Choice(&[{ch_list}])'
    elif kind == 'text':
        rust_ret = '&str'
        rust_body = f'self.get("{key}", {json.dumps(default)})'
        ts_default = json.dumps(default)
        ts_control = "{ kind: 'text' }"
        console_ctrl = 'Text'
    else:
        raise ValueError(f'unsupported setting type: {kind!r}')

    rust_snippet = f"""    /// {label}: {description}
    pub fn {key}(&self) -> {rust_ret} {{
        {rust_body}
    }}"""

    ts_snippet = f"""  {{
    key: {json.dumps(key)},
    label: {json.dumps(label)},
    description: {json.dumps(description)},
    category: {json.dumps(category)},
    group: {json.dumps(group)},
    control: {ts_control},
    default: {ts_default},
  }},"""

    console_snippet = f'        "{key}", {json.dumps(label)}, {console_ctrl};'
    i18n_spec = {
        'host': [
            {'key': f'config.{key}', 'en': label, 'translations': {}},
            {'key': f'config.{key}_desc', 'en': description, 'translations': {}},
        ]
    }
    doc_row = f'| `{key}` | `{default}` | {description} |'
    note_md = f'# {label}\n- **{label}.** {description}\n'

    rendered = (
        f'=== 1. Rust accessor (rust/core/config/src/config.rs) ===\n{rust_snippet}\n\n'
        f'=== 2. Web schema (rust/web/src/lib/schema/{schema_file}) ===\n{ts_snippet}\n\n'
        f'=== 3. No-JS console (rust/host/web/src/console/settings.rs) ===\n{console_snippet}\n\n'
        f'=== 4. Host i18n batch spec (tools/i18n.py apply) ===\n{json.dumps(i18n_spec, indent=2, ensure_ascii=False)}\n\n'
        f'=== 5. docs/configuration.md row ===\n{doc_row}\n\n'
        f'=== 6. Release note fragment (rust/release/notes.d/{key}.md) ===\n{note_md}'
    )
    return rendered, note_md


# ---- 4. Test scenario generator ----

def generate_test_scenario(name, suite='reconnect', summary=''):
    validate_snake(name)
    doc = summary.strip() or f'Focused {suite} scenario tests for `{name}`.'
    if suite == 'reconnect':
        rel = f'tests/host/tests/reconnect_tests/{name}.rs'
        src = f"""//! {doc}
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn {name}_scenario() -> Result<()> {{
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}}
"""
        wire = f'Add `mod {name};` in tests/host/tests/reconnect_tests.rs'
    elif suite == 'native':
        rel = f'tests/host/tests/nvhttp_parity/native_tests/{name}.rs'
        src = f"""//! {doc}
use super::*;

#[tokio::test]
async fn {name}_scenario() {{
    let s = Setup::with(LIST);
    let reply = s.get("/capabilities").await;
    assert_eq!(reply.status, StatusCode::OK, "{{}}", reply.text());
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
}}
"""
        wire = f'Add `mod {name};` in tests/host/tests/nvhttp_parity/native_tests.rs'
    elif suite == 'config':
        rel = f'rust/core/config/src/config/tests/{name}.rs'
        src = f"""//! {doc}
use super::*;

#[test]
fn {name}_defaults() {{
    let config = Config::default();
    let _ = config;
}}
"""
        wire = f'Add `mod {name};` in rust/core/config/src/config/tests.rs'
    elif suite == 'interop':
        rel = f'tests/interop/tests/{name}.rs'
        src = f"""//! {doc}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn {name}_interop_scenario() {{
    // Wire host + nightjar-core in-process interop assertion here.
}}
"""
        wire = f'Run with `cargo test -p rubylight-interop --test {name} --locked`'
    else:
        raise ValueError(f'unsupported suite: {suite!r}')
    return {rel: src}, [wire]


# ---- CLI ----

def cmd_control_message(args):
    root = pathlib.Path(args.root).resolve()
    try:
        msg_id_hex, msg_id_int = parse_hex_u16(args.id)
        used = existing_control_ids(root)
        if msg_id_int in used and not args.force:
            raise ValueError(f'control message ID {msg_id_hex} is already registered in client/protocol/core/src/ids.rs')
        files, checklist = generate_control_message(args.name, msg_id_hex, args.summary, args.launch_param)
        written = write_plan(root, files, dry_run=args.dry_run, force=args.force)
    except (ValueError, FileExistsError) as exc:
        print(f'ERROR: {exc}', file=sys.stderr)
        return 1
    prefix = '[dry-run] would create' if args.dry_run else 'created'
    for rel in written:
        print(f'{prefix}: {rel}')
    print('\nNext steps:')
    for step in checklist:
        print(f'  {step}')
    return 0


def cmd_native_endpoint(args):
    root = pathlib.Path(args.root).resolve()
    try:
        files, checklist = generate_native_endpoint(
            args.name,
            method=args.method,
            route_path=args.path,
            permission=args.permission,
            summary=args.summary,
        )
        written = write_plan(root, files, dry_run=args.dry_run, force=args.force)
    except (ValueError, FileExistsError) as exc:
        print(f'ERROR: {exc}', file=sys.stderr)
        return 1
    prefix = '[dry-run] would create' if args.dry_run else 'created'
    for rel in written:
        print(f'{prefix}: {rel}')
    print('\nNext steps:')
    for step in checklist:
        print(f'  {step}')
    return 0


def cmd_setting(args):
    root = pathlib.Path(args.root).resolve()
    choices = tuple(c.strip() for c in args.choices.split(',') if c.strip()) if args.choices else ()
    try:
        rendered, note_md = generate_setting_stub(
            args.key,
            args.type,
            args.default,
            args.label,
            args.description,
            category=args.category,
            group=args.group,
            choices=choices,
            min_val=args.min,
            max_val=args.max,
            extension=args.extension,
        )
        files = {}
        if args.with_note:
            files[f'rust/release/notes.d/{args.key}.md'] = note_md
        if args.output:
            out_path = pathlib.Path(args.output)
            if not args.dry_run:
                out_path.parent.mkdir(parents=True, exist_ok=True)
                out_path.write_text(rendered.replace('\r\n', '\n'), encoding='utf-8', newline='\n')
        if files:
            written = write_plan(root, files, dry_run=args.dry_run, force=args.force)
            prefix = '[dry-run] would create' if args.dry_run else 'created'
            for rel in written:
                print(f'{prefix}: {rel}')
    except (ValueError, FileExistsError) as exc:
        print(f'ERROR: {exc}', file=sys.stderr)
        return 1
    if not args.output or args.dry_run:
        print(rendered)
    return 0


def cmd_test_scenario(args):
    root = pathlib.Path(args.root).resolve()
    try:
        files, checklist = generate_test_scenario(args.name, suite=args.suite, summary=args.summary)
        written = write_plan(root, files, dry_run=args.dry_run, force=args.force)
    except (ValueError, FileExistsError) as exc:
        print(f'ERROR: {exc}', file=sys.stderr)
        return 1
    prefix = '[dry-run] would create' if args.dry_run else 'created'
    for rel in written:
        print(f'{prefix}: {rel}')
    print('\nNext steps:')
    for step in checklist:
        print(f'  {step}')
    return 0


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--root', default=str(ROOT), help='repository root directory')
    sub = parser.add_subparsers(dest='command', required=True)

    p_cm = sub.add_parser('control-message', help='scaffold a no_std control message in rubylight-protocol')
    p_cm.add_argument('name', help='snake_case module name')
    p_cm.add_argument('--id', required=True, help='hex u16 message ID, e.g. 0x5547')
    p_cm.add_argument('--summary', default='', help='one-sentence module header summary')
    p_cm.add_argument('--launch-param', default='', help='optional /launch query parameter name')
    p_cm.add_argument('--dry-run', action='store_true')
    p_cm.add_argument('--force', action='store_true')
    p_cm.set_defaults(func=cmd_control_message)

    p_ne = sub.add_parser('native-endpoint', help='scaffold an additive v1 native API endpoint + fixture + parity test')
    p_ne.add_argument('name', help='snake_case endpoint module name')
    p_ne.add_argument('--method', choices=('get', 'post'), default='get')
    p_ne.add_argument('--path', default='', help='route path under /api/native/v1 (default /<name>)')
    p_ne.add_argument('--permission', choices=('list', 'view', 'launch', 'host_control'), default='list')
    p_ne.add_argument('--summary', default='', help='one-sentence description')
    p_ne.add_argument('--dry-run', action='store_true')
    p_ne.add_argument('--force', action='store_true')
    p_ne.set_defaults(func=cmd_native_endpoint)

    p_set = sub.add_parser('setting', help='generate host setting boilerplate snippets and optional note fragment')
    p_set.add_argument('key', help='snake_case setting key')
    p_set.add_argument('--type', choices=('bool', 'int', 'choice', 'text'), required=True)
    p_set.add_argument('--default', required=True, help='default value')
    p_set.add_argument('--label', required=True, help='UI label')
    p_set.add_argument('--description', required=True, help='UI description')
    p_set.add_argument('--category', default='video', choices=tuple(CATEGORY_TO_SCHEMA_FILE.keys()))
    p_set.add_argument('--extension', action='store_true', help='target rust/web/src/lib/schema/extensions.ts')
    p_set.add_argument('--group', default='General')
    p_set.add_argument('--choices', default='', help='comma-separated values for --type choice')
    p_set.add_argument('--min', type=int, default=None)
    p_set.add_argument('--max', type=int, default=None)
    p_set.add_argument('--with-note', action='store_true', help='also create rust/release/notes.d/<key>.md')
    p_set.add_argument('--output', default='', help='optional path to write rendered snippets')
    p_set.add_argument('--dry-run', action='store_true')
    p_set.add_argument('--force', action='store_true')
    p_set.set_defaults(func=cmd_setting)

    p_ts = sub.add_parser('test-scenario', help='scaffold a focused <=800-LOC Rust test scenario file')
    p_ts.add_argument('name', help='snake_case test module name')
    p_ts.add_argument('--suite', choices=('reconnect', 'native', 'config', 'interop'), default='reconnect')
    p_ts.add_argument('--summary', default='', help='one-sentence module header summary')
    p_ts.add_argument('--dry-run', action='store_true')
    p_ts.add_argument('--force', action='store_true')
    p_ts.set_defaults(func=cmd_test_scenario)

    return parser


def main(argv=None):
    if hasattr(sys.stdout, 'reconfigure'):
        sys.stdout.reconfigure(encoding='utf-8', errors='replace')
    if hasattr(sys.stderr, 'reconfigure'):
        sys.stderr.reconfigure(encoding='utf-8', errors='replace')
    args = build_parser().parse_args(argv)
    return args.func(args)


if __name__ == '__main__':
    sys.exit(main())
