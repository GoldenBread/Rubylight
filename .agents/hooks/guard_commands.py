#!/usr/bin/env python3
"""PreToolUse command & file guardrail for the Rubylight/Nightjar 7.0.0 roadmap.

Supports BOTH:
- Claude Code (`2.1.x+`) PreToolUse hooks via `.claude/settings.json`
  (snake_case `tool_name` in `Bash|PowerShell|Read|Write|Edit|MultiEdit|NotebookEdit`,
  `tool_input`, emitting `{}` on allow or `{"hookSpecificOutput": {"hookEventName": "PreToolUse",
  "permissionDecision": "deny", "permissionDecisionReason": "..."}}` on deny).
- Legacy Antigravity PreToolUse hooks via `.agents/hooks.json`
  (camelCase `toolCall`, emitting `{"decision": "allow"}` or `{"decision": "deny", "reason": "..."}`).

Guards shell and file tools against:
1. Unsafe mutations, guardrail tampering, force-pushes, tag edits, `cargo fmt --all`, unlocked `rust\build.ps1`.
2. Garbage pileup & lock bypasses: bare `cargo build/test/clippy/...` without shared target pool/locks,
   direct unlocked writes to `rl-target-pool`, and bare `gradlew` without Gradle lane + `--no-daemon`.
3. Windows Defender Firewall popups: wildcard `python -m http.server` / `vite --host 0.0.0.0` dev servers.
4. Unauthorized Wave 6 releases (`.agents/RELEASE_7_0_AUTHORIZED`) and post-hang GPU runs (`.agents/GPU_HANG_HALT`).
Fail-closed on malformed payloads.
"""
from __future__ import annotations

import json
import os
import pathlib
import re
import sys
from typing import Any, Mapping

RELEASE_SENTINEL = "RELEASE_7_0_AUTHORIZED"
GPU_HANG_SENTINEL = "GPU_HANG_HALT"
HW_DISABLE_ENV = "RUBYLIGHT_DISABLE_HW_GATE"

GIT_CMD = r"\bgit(?:\.exe)?\b"
CARGO_CMD = r"\bcargo(?:\.exe)?\b"
GH_CMD = r"\bgh(?:\.exe)?\b"

SHELL_TOOLS = frozenset({"run_command", "Bash", "PowerShell"})
READ_FILE_TOOLS = frozenset({"read_file", "Read"})
WRITE_FILE_TOOLS = frozenset({"write_file", "edit_file", "Write", "Edit", "MultiEdit", "NotebookEdit"})

# Protected sensitive paths (denied for ANY read/write/edit/command).
PROGRAMDATA_BP_RE = re.compile(
    r"(?:programdata|%programdata%|\$env:programdata)[\\/]+butterpollo(?![A-Za-z0-9_-])",
    re.IGNORECASE,
)
KEYSTORE_PROPS_RE = re.compile(r"(?:^|[\s\"'`/\\])keystore\.properties(?:\b|$)", re.IGNORECASE)

# Protected guardrail / sentinel / baseline files (denied for write/edit tools and mutating shell commands).
PROTECTED_GUARD_PATH_RE = re.compile(
    r"(?:^|[\\/])(?:"
    r"\.agents[\\/](?:RELEASE_7_0_AUTHORIZED|GPU_HANG_HALT|hooks\.json|hooks[\\/][^\\/\s]+|tools[\\/]roadmap_hygiene\.py)"
    r"|\.claude[\\/](?:settings(?:\.local)?\.json|agents[\\/][^\\/\s]+)"
    r"|tools[\\/](?:size_ratchet\.json|arch_baselines\.json)"
    r")(?:\b|$)",
    re.IGNORECASE,
)
PROTECTED_SENTINEL_OR_BASELINE_CMD_RE = re.compile(
    r"(?:"
    r"\.agents[\\/]+(?:RELEASE_7_0_AUTHORIZED|GPU_HANG_HALT|hooks\.json|hooks[\\/]+(?:guard_commands|watch_gpu_hang)\.py|tools[\\/]+roadmap_hygiene\.py)"
    r"|\.claude[\\/]+(?:settings(?:\.local)?\.json|agents[\\/]+[^\\/\s;|&\"']+)"
    r"|tools[\\/]+(?:size_ratchet\.json|arch_baselines\.json)"
    r")",
    re.IGNORECASE,
)
READ_ONLY_SHELL_RE = re.compile(
    r"^\s*(?:python(?:\.exe)?\s+(?:-B\s+)?(?:-m\s+(?:unittest|py_compile)\b|\.agents[\\/]+tools[\\/]+roadmap_hygiene\.py\b)|git(?:\.exe)?\s+(?:-C\s+\S+\s+)?(?:status|diff|log|show|ls-files)\b|Test-Path\b|cat\b|head\b|tail\b|ls\b)",
    re.IGNORECASE,
)

# 1. Always hard-denied command patterns.
FORCE_PUSH_RE = re.compile(
    GIT_CMD
    + r"(?:\s+-\S+(?:\s+\S+)?)*\s+push\b[^\n;|&]*(?:--force\b|--force-with-lease\b|--force-if-includes\b|\s-[a-zA-Z]*f[a-zA-Z]*\b|\s\+[^\s;|&]+)",
    re.IGNORECASE,
)
TAG_MUTATE_RE = re.compile(
    GIT_CMD
    + r"(?:\s+-\S+(?:\s+\S+)?)*\s+tag\b[^\n;|&]*(?:--force\b|--delete\b|\s-[a-zA-Z]*[fd][a-zA-Z]*\b)",
    re.IGNORECASE,
)
REMOTE_TAG_DELETE_RE = re.compile(
    GIT_CMD
    + r"(?:\s+-\S+(?:\s+\S+)?)*\s+push\b[^\n;|&]*(?:(?:--delete|\s-d)\b[^\n;|&]*(?:refs/tags/|\btag\b|\bv\d)|\s:refs/tags/|\s:v\d)",
    re.IGNORECASE,
)
CARGO_FMT_ALL_RE = re.compile(
    CARGO_CMD + r"(?:\s+\+\S+)?\s+fmt\b[^\n;|&]*--all\b",
    re.IGNORECASE,
)
ARCH_WRITE_BASELINES_RE = re.compile(
    r"\barch_check\.py\b[^\n;|&]*--write-baselines\b",
    re.IGNORECASE,
)
RUST_BUILD_PS1_RE = re.compile(
    r"(?:^|[\s\"'`&;|/\\])(?:\.[\\/]+)?rust[\\/]+build\.ps1\b",
    re.IGNORECASE,
)
CD_RUST_BUILD_RE = re.compile(
    r"\b(?:cd|Set-Location|sl|pushd)\b[^\n;|&]*\brust\b[^\n]*(?:[;&|\n]|\$\().*?(?:^|[\s\"'`/\\])build\.ps1\b",
    re.IGNORECASE | re.DOTALL,
)
BUILD_LOCK_EXCLUSIVE_RE = re.compile(
    r"\btools[\\/]+build_lock\.py\b(?![^\n;|&]*--lane\b)[^\n;|&]*--\s+[^\n;|&]*rust[\\/]+build\.ps1\b",
    re.IGNORECASE,
)

# 1b. Autonomous hygiene, shared-build & firewall-prompt guards.
CARGO_COMPILE_RE = re.compile(
    CARGO_CMD + r"(?:\s+\+\S+)?\s+(?:build|check|test|clippy|run|bench|doc|nextest|rustc)\b",
    re.IGNORECASE,
)
CARGO_POOL_MANAGED_RE = re.compile(
    r"(?:\btools[\\/]+dev\.(?:ps1|py)\b|\brl-quick\.ps1\b|\bEnter-RlSlot\b|\brust[\\/]+build\.ps1\b)",
    re.IGNORECASE,
)
CARGO_TARGET_DIR_RE = re.compile(r"(?:--target-dir(?:\s+|=)\s*([^\s;|&\"']+)|\bCARGO_TARGET_DIR\s*=\s*[\"']?([^\s;|&\"']+))", re.IGNORECASE)
PRIVATE_TARGET_VAL_RE = re.compile(r"^(?:\.[\\/]+)?(?:rust[\\/]+service[\\/]+|client[\\/]+protocol[\\/]+)?target(?:[\\/]*)$", re.IGNORECASE)
DIRECT_POOL_WRITE_RE = re.compile(r"rl-target-pool[\\/]+(?:client-)?\d+", re.IGNORECASE)

GRADLEW_RE = re.compile(r"(?:^|[\s\"'`/\\])gradlew(?:\.bat)?\b", re.IGNORECASE)
GRADLE_LOCKED_RE = re.compile(
    r"\btools[\\/]+build_lock\.py\b[^\n;|&]*(?:--lane\s+gradle\b|--lock-directory\b)",
    re.IGNORECASE,
)
GRADLE_NO_DAEMON_RE = re.compile(r"--no-daemon\b", re.IGNORECASE)

WILDCARD_HTTP_SERVER_RE = re.compile(
    r"\bpython(?:\.exe)?\b[^\n;|&]*\s-m\s+http\.server\b(?![^\n;|&]*(?:-b|--bind)\s+(?:127\.0\.0\.1|localhost|::1)\b)",
    re.IGNORECASE,
)
WILDCARD_VITE_HOST_RE = re.compile(
    r"\b(?:vite|npm(?:\.cmd)?\s+run\s+(?:dev|preview))\b[^\n;|&]*--host(?:\s+0\.0\.0\.0\b|\s*=\s*0\.0\.0\.0\b|\s*$|\s+-)",
    re.IGNORECASE,
)

# 2. Release/publish patterns (denied unless RELEASE_7_0_AUTHORIZED exists and GPU_HANG_HALT does not).
GIT_PUSH_RE = re.compile(GIT_CMD + r"(?:\s+-\S+(?:\s+\S+)?)*\s+push\b([^\n;|&]*)", re.IGNORECASE)
SAFE_PUSH_REFSPEC_RE = re.compile(
    r"(?:^|\s)(?:(?:HEAD:)?(?:refs/heads/)?(?:wip/|roadmap/)[^\s;|&]+)(?:\s|$)",
    re.IGNORECASE,
)
PUSH_PROTECTED_REF_RE = re.compile(
    r"(?:\bmain\b|\bmaster\b|refs/heads/main\b|:main\b|\bgh-pages\b|refs/heads/gh-pages\b|:gh-pages\b|refs/tags/v\d|--tags\b|\btag\s+v\d|\bv\d+\.\d+)",
    re.IGNORECASE,
)
GH_RELEASE_RE = re.compile(GH_CMD + r"\s+release\b", re.IGNORECASE)
GH_API_RELEASES_RE = re.compile(GH_CMD + r"\s+api\b[^\n;|&]*/releases\b", re.IGNORECASE)
GH_WORKFLOW_PUBLISH_RE = re.compile(
    GH_CMD + r"\s+workflow\s+run\b[^\n;|&]*(?:(?:-f|-F|--field|--raw-field)\s*(?:publish\s*=\s*true|dry_run\s*=\s*false))\b",
    re.IGNORECASE,
)

# 3. Hardware / GPU test patterns (allowed by default when host is idle; denied if GPU_HANG_HALT exists or RUBYLIGHT_DISABLE_HW_GATE=1).
HW_CMD_RE = re.compile(
    r"(?:(?:^|[\s\"'`/\\])(?:e2e\.py|check\.ps1|(?:[A-Za-z0-9_-]*_)?soak\.(?:ps1|py)|focus_ab\.py|wave1_verify\.py)\b|\btools[\\/]+hw[\\/]+(?:run(?:\.py|\.ps1)?|laptop-host\.ps1|demo-stream\.ps1)\b|--ignored\b|--run-ignored\b)",
    re.IGNORECASE,
)


def _candidate_agents_dirs(
    agents_dir: pathlib.Path,
    cwd: str | None,
    workspace_paths: list[str] | None = None,
) -> list[pathlib.Path]:
    """Collect `.agents` dirs for the active worktree AND any enclosing git checkout (e.g. `.claude/worktrees/<name>`)."""
    dirs = [agents_dir.resolve()]
    candidates: list[str] = [str(agents_dir.resolve().parent)]
    if cwd:
        candidates.append(cwd)
    if workspace_paths:
        candidates.extend(workspace_paths)
    for raw in candidates:
        cur = pathlib.Path(raw).resolve()
        for parent in (cur, *cur.parents):
            cand = parent / ".agents"
            if cand.is_dir() and (parent / ".git").exists() and cand not in dirs:
                dirs.append(cand)
        for parent in (cur, *cur.parents):
            cand = parent / ".agents"
            if cand.is_dir():
                if cand not in dirs:
                    dirs.append(cand)
                break
    return dirs


def _has_sentinel(dirs: list[pathlib.Path], filename: str) -> bool:
    return any((d / filename).exists() for d in dirs)


def _split_segments(command_line: str) -> list[str]:
    """Split shell command across chains, pipes, subexpressions, and scriptblocks."""
    parts = re.split(r"&&|\|\||[;|\r\n]|\$\(|&\s*\{", command_line)
    return [seg.strip(" )}'") for seg in parts if seg.strip(" )}'")]


def evaluate_file_tool(tool_name: str, path_value: str) -> dict[str, str]:
    """Guard read/write/edit file tools across Claude Code and Antigravity."""
    norm = path_value.replace("\\", "/")
    if PROGRAMDATA_BP_RE.search(path_value) or PROGRAMDATA_BP_RE.search(norm):
        return {
            "decision": "deny",
            "reason": f"Hard-denied ({tool_name}): touching C:\\ProgramData\\Butterpollo is forbidden.",
        }
    if KEYSTORE_PROPS_RE.search(path_value) or KEYSTORE_PROPS_RE.search(norm):
        return {
            "decision": "deny",
            "reason": f"Hard-denied ({tool_name}): touching keystore.properties is forbidden.",
        }
    if tool_name in WRITE_FILE_TOOLS and (
        PROTECTED_GUARD_PATH_RE.search(path_value) or PROTECTED_GUARD_PATH_RE.search(norm)
    ):
        return {
            "decision": "deny",
            "reason": (
                f"Hard-denied ({tool_name}): modifying sentinels (.agents/RELEASE_7_0_AUTHORIZED, "
                ".agents/GPU_HANG_HALT), hook/agent/hygiene guards (.agents/hooks*, .agents/tools/roadmap_hygiene.py, "
                ".claude/settings*.json, .claude/agents/*), or architecture baselines (tools/size_ratchet.json, "
                "tools/arch_baselines.json) is forbidden."
            ),
        }
    return {"decision": "allow"}


def evaluate_command(
    command_line: str,
    *,
    agents_dir: pathlib.Path,
    env: Mapping[str, str] | None = None,
    cwd: str | None = None,
    workspace_paths: list[str] | None = None,
) -> dict[str, str]:
    """Return {"decision": "allow"} or {"decision": "deny", "reason": "..."}."""
    env_map = os.environ if env is None else env
    dirs = _candidate_agents_dirs(agents_dir, cwd, workspace_paths)
    segments = _split_segments(command_line)
    has_env_target_dir = bool(env_map.get("CARGO_TARGET_DIR"))
    m_cmd_target = CARGO_TARGET_DIR_RE.search(command_line)
    cmd_target_val = (m_cmd_target.group(1) or m_cmd_target.group(2) or "") if m_cmd_target else ""
    has_external_target_dir = (bool(cmd_target_val) and not PRIVATE_TARGET_VAL_RE.match(cmd_target_val)) or has_env_target_dir
    has_cmd_pool_managed = bool(CARGO_POOL_MANAGED_RE.search(command_line))

    # --- Tier 1: Always hard-denied ---
    if PROGRAMDATA_BP_RE.search(command_line):
        return {
            "decision": "deny",
            "reason": "Hard-denied: touching C:\\ProgramData\\Butterpollo is forbidden; use an isolated profile.",
        }
    if KEYSTORE_PROPS_RE.search(command_line):
        return {
            "decision": "deny",
            "reason": "Hard-denied: touching keystore.properties is forbidden.",
        }
    if ARCH_WRITE_BASELINES_RE.search(command_line):
        return {
            "decision": "deny",
            "reason": "Hard-denied: `arch_check.py --write-baselines` is forbidden; stay under the 800-LOC ceiling and ratchets.",
        }
    if CD_RUST_BUILD_RE.search(command_line):
        return {
            "decision": "deny",
            "reason": "Hard-denied: `rust\\build.ps1` must run via exclusive `python tools/build_lock.py -- pwsh rust/build.ps1` (no `cd rust; .\\build.ps1`).",
        }

    for segment in segments:
        if PROTECTED_SENTINEL_OR_BASELINE_CMD_RE.search(segment) and (
            ">" in segment or not READ_ONLY_SHELL_RE.match(segment)
        ):
            return {
                "decision": "deny",
                "reason": "Hard-denied: shell commands may not create, edit, or delete sentinels, hook/agent/hygiene guards, or size/arch baseline files.",
            }
        if FORCE_PUSH_RE.search(segment):
            return {
                "decision": "deny",
                "reason": "Hard-denied: force pushing (git push --force / --force-with-lease / +ref) is forbidden.",
            }
        if TAG_MUTATE_RE.search(segment) or REMOTE_TAG_DELETE_RE.search(segment):
            return {
                "decision": "deny",
                "reason": "Hard-denied: moving or deleting tags (git tag -f / -d / deleting remote tags) is forbidden.",
            }
        if CARGO_FMT_ALL_RE.search(segment):
            return {
                "decision": "deny",
                "reason": "Hard-denied: run `cargo fmt -- --check` without `--all` (AGENTS.md rule).",
            }
        if RUST_BUILD_PS1_RE.search(segment) and not BUILD_LOCK_EXCLUSIVE_RE.search(segment):
            return {
                "decision": "deny",
                "reason": "Hard-denied: `rust\\build.ps1` must be wrapped with exclusive `python tools/build_lock.py -- pwsh rust/build.ps1` (no `--lane`).",
            }
        # Shared target pool & build-lock discipline (prevent private 10-25 GB worktree `target/` & unlocked pool corruption).
        if CARGO_COMPILE_RE.search(segment):
            pool_managed = bool(CARGO_POOL_MANAGED_RE.search(segment)) or has_cmd_pool_managed
            if DIRECT_POOL_WRITE_RE.search(command_line) and not pool_managed:
                return {
                    "decision": "deny",
                    "reason": (
                        "Hard-denied: never target `C:\\src\\rl-target-pool\\<slot>` directly without locking/claiming it. "
                        "Use `tools\\dev.ps1 check -Fast -Base <base>` or `pwsh C:\\src\\rl-quick.ps1 -Repo . -Crates <crate>`."
                    ),
                }
            if not pool_managed and not has_external_target_dir:
                return {
                    "decision": "deny",
                    "reason": (
                        "Hard-denied: `cargo` build/test/clippy without a shared target pool creates a private 10-25 GB `target/` "
                        "inside the worktree and bypasses `C:\\src\\rl-target-pool`. Use `tools\\dev.ps1 check -Fast -Base <base>` "
                        "(or `-Paths <file>`), `pwsh C:\\src\\rl-quick.ps1 -Repo . -Crates <crate>`, or "
                        "`python tools/build_lock.py --lane cargo -- ... --target-dir D:\\t\\<tmp>` (deleted in finally)."
                    ),
                }
        # Gradle lane & `--no-daemon` discipline (prevent leaked 4+ GB Gradle daemons & lock collisions).
        if GRADLEW_RE.search(segment) and not re.search(r"\btools[\\/]+dev\.(?:ps1|py)\b", segment, re.IGNORECASE):
            if not (GRADLE_LOCKED_RE.search(segment) and GRADLE_NO_DAEMON_RE.search(segment)):
                return {
                    "decision": "deny",
                    "reason": (
                        "Hard-denied: wrap direct `gradlew` runs in the shared Gradle lane with `--no-daemon`: "
                        "`python tools/build_lock.py --lane gradle --label <name> -- cmd /c client\\gradlew.bat -p client --no-daemon --max-workers=2 <task>` "
                        "(or use `tools\\dev.ps1 check`)."
                    ),
                }
        # Windows Defender Firewall popup prevention on ad-hoc local web servers.
        if WILDCARD_HTTP_SERVER_RE.search(segment):
            return {
                "decision": "deny",
                "reason": "Hard-denied: pass `--bind 127.0.0.1` to `python -m http.server` to prevent Windows Defender Firewall prompts.",
            }
        if WILDCARD_VITE_HOST_RE.search(segment):
            return {
                "decision": "deny",
                "reason": "Hard-denied: pass `--host 127.0.0.1` (never bare `--host` or `0.0.0.0`) to prevent Windows Defender Firewall prompts.",
            }

    gpu_halted = _has_sentinel(dirs, GPU_HANG_SENTINEL)
    release_authorized = _has_sentinel(dirs, RELEASE_SENTINEL)

    # --- Tier 2: Release/publish gate ---
    release_reason: str | None = None
    for segment in segments:
        m_push = GIT_PUSH_RE.search(segment)
        if m_push:
            tail = m_push.group(1) or ""
            if PUSH_PROTECTED_REF_RE.search(tail):
                release_reason = "pushing to main, gh-pages, or release tags"
            elif not SAFE_PUSH_REFSPEC_RE.search(tail):
                release_reason = "ambiguous or default `git push` without explicit `wip/*` or `roadmap/*` branch"
        elif GH_RELEASE_RE.search(segment):
            release_reason = "running `gh release`"
        elif GH_API_RELEASES_RE.search(segment):
            release_reason = "calling GitHub Releases API (`gh api .../releases`)"
        elif GH_WORKFLOW_PUBLISH_RE.search(segment):
            release_reason = "triggering publishing workflow (`publish=true` or `dry_run=false`)"
        if release_reason is not None:
            break

    if release_reason is not None:
        if gpu_halted:
            return {
                "decision": "deny",
                "reason": f"Hard-denied ({release_reason}): .agents/{GPU_HANG_SENTINEL} is present after an AMD encoder hang.",
            }
        if not release_authorized:
            return {
                "decision": "deny",
                "reason": f"Hard-denied ({release_reason}): requires .agents/{RELEASE_SENTINEL} sentinel from Wave 6 maintainer-approved release gate.",
            }

    # --- Tier 3: GPU / hardware test gate (allowed by default; opt-out via RUBYLIGHT_DISABLE_HW_GATE=1 or GPU_HANG_HALT) ---
    if any(HW_CMD_RE.search(seg) for seg in segments):
        if gpu_halted:
            return {
                "decision": "deny",
                "reason": f"Hard-denied (GPU/HW test): .agents/{GPU_HANG_SENTINEL} is active (exit code 3 encoder hang). Investigate, do not retry.",
            }
        if env_map.get(HW_DISABLE_ENV) == "1":
            return {
                "decision": "deny",
                "reason": f"Hard-denied (GPU/HW test): disabled by process environment {HW_DISABLE_ENV}=1.",
            }

    return {"decision": "allow"}


def is_claude_code_payload(payload: Mapping[str, Any]) -> bool:
    """Detect Claude Code hook payloads (`tool_name` / `tool_input` / `hook_event_name`)."""
    return "tool_name" in payload or "tool_input" in payload or "hook_event_name" in payload


def format_hook_response(decision: Mapping[str, str], *, claude_code: bool) -> dict[str, Any]:
    """Format decision for Claude Code (`hookSpecificOutput` / `{}`) or Antigravity (`decision`)."""
    if not claude_code:
        return dict(decision)
    if decision.get("decision") == "deny":
        return {
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": decision.get("reason", "Denied by guard_commands.py."),
            }
        }
    return {}


def handle_payload(
    payload: dict[str, Any],
    *,
    agents_dir: pathlib.Path,
    env: Mapping[str, str] | None = None,
) -> dict[str, str]:
    """Evaluate either a Claude Code or Antigravity PreToolUse payload and return normalized decision."""
    if is_claude_code_payload(payload):
        tool_name = payload.get("tool_name")
        args = payload.get("tool_input")
        if not isinstance(tool_name, str) or not tool_name:
            return {"decision": "deny", "reason": "guard_commands: missing tool_name in Claude Code hook payload."}
        if not isinstance(args, dict):
            return {"decision": "deny", "reason": "guard_commands: missing tool_input object in Claude Code hook payload."}
    else:
        tool_call = payload.get("toolCall")
        if not isinstance(tool_call, dict):
            return {"decision": "deny", "reason": "guard_commands: missing toolCall or tool_name in hook payload."}
        tool_name = tool_call.get("name")
        args = tool_call.get("args")
        if not isinstance(args, dict):
            return {"decision": "deny", "reason": "guard_commands: missing toolCall.args object."}

    if tool_name in READ_FILE_TOOLS or tool_name in WRITE_FILE_TOOLS:
        path_val = (
            args.get("file_path")
            or args.get("notebook_path")
            or args.get("Path")
            or args.get("path")
            or ""
        )
        if not isinstance(path_val, str):
            return {"decision": "deny", "reason": f"guard_commands: {tool_name} path must be a string."}
        return evaluate_file_tool(tool_name, path_val)

    if tool_name not in SHELL_TOOLS:
        return {"decision": "allow"}

    command_line = args.get("command") or args.get("CommandLine") or args.get("commandLine") or ""
    if not isinstance(command_line, str):
        return {"decision": "deny", "reason": "guard_commands: command must be a string."}
    cwd = payload.get("cwd") if isinstance(payload.get("cwd"), str) else None
    ws = payload.get("workspacePaths") if isinstance(payload.get("workspacePaths"), list) else None
    ws_clean = [w for w in (ws or []) if isinstance(w, str)]
    return evaluate_command(
        command_line,
        agents_dir=agents_dir,
        env=env,
        cwd=cwd,
        workspace_paths=ws_clean,
    )


def main() -> int:
    default_agents_dir = pathlib.Path(
        os.environ.get("RUBYLIGHT_AGENTS_DIR") or pathlib.Path(__file__).resolve().parent.parent
    )
    raw = sys.stdin.read()
    try:
        payload = json.loads(raw)
        if not isinstance(payload, dict):
            raise ValueError("payload must be a JSON object")
    except Exception as exc:
        reason = f"guard_commands: invalid JSON payload ({exc})."
        print(
            json.dumps(
                {
                    "decision": "deny",
                    "reason": reason,
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason,
                    },
                }
            )
        )
        return 0
    decision = handle_payload(payload, agents_dir=default_agents_dir)
    out = format_hook_response(decision, claude_code=is_claude_code_payload(payload))
    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
