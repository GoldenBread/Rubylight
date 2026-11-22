#!/usr/bin/env python3
"""Antigravity PreToolUse command & file guardrail for the Rubylight/Nightjar 7.0.0 roadmap.

Guards `run_command`, `write_file`, `edit_file`, and `read_file` against unsafe
mutations, bypasses, unauthorized releases, and hardware runs after an AMD encoder hang.

Reads camelCase JSON on stdin (Antigravity PreToolUse contract) and prints
{"decision": "allow"} or {"decision": "deny", "reason": "..."} on stdout.
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
HW_GATE_ENV = "RUBYLIGHT_ALLOW_HW_GATE"

GIT_CMD = r"\bgit(?:\.exe)?\b"
CARGO_CMD = r"\bcargo(?:\.exe)?\b"
GH_CMD = r"\bgh(?:\.exe)?\b"

# Protected sensitive paths (denied for ANY read/write/edit/command).
PROGRAMDATA_BP_RE = re.compile(
    r"(?:programdata|%programdata%|\$env:programdata)[\\/]+butterpollo(?![A-Za-z0-9_-])",
    re.IGNORECASE,
)
KEYSTORE_PROPS_RE = re.compile(r"(?:^|[\s\"'`/\\])keystore\.properties(?:\b|$)", re.IGNORECASE)

# Protected guardrail / sentinel / baseline files (denied for write_file, edit_file, and mutating shell commands).
PROTECTED_GUARD_PATH_RE = re.compile(
    r"(?:^|[\\/])(?:\.agents[\\/](?:RELEASE_7_0_AUTHORIZED|GPU_HANG_HALT|hooks\.json|hooks[\\/][^\\/\s]+)|tools[\\/](?:size_ratchet\.json|arch_baselines\.json))(?:\b|$)",
    re.IGNORECASE,
)
PROTECTED_SENTINEL_OR_BASELINE_CMD_RE = re.compile(
    r"(?:\.agents[\\/]+(?:RELEASE_7_0_AUTHORIZED|GPU_HANG_HALT|hooks\.json|hooks[\\/]+(?:guard_commands|watch_gpu_hang)\.py)|tools[\\/]+(?:size_ratchet\.json|arch_baselines\.json))",
    re.IGNORECASE,
)
READ_ONLY_SHELL_RE = re.compile(
    r"^\s*(?:python(?:\.exe)?\s+-m\s+(?:unittest|py_compile)\b|git(?:\.exe)?\s+(?:-C\s+\S+\s+)?(?:status|diff|log|show|ls-files)\b|Test-Path\b)",
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

# 3. Hardware / GPU test patterns (denied unless RUBYLIGHT_ALLOW_HW_GATE=1 and GPU_HANG_HALT does not exist).
HW_CMD_RE = re.compile(
    r"(?:(?:^|[\s\"'`/\\])(?:e2e\.py|check\.ps1|(?:[A-Za-z0-9_-]*_)?soak\.(?:ps1|py)|focus_ab\.py|wave1_verify\.py)\b|\btools[\\/]+hw[\\/]+(?:run(?:\.py|\.ps1)?|laptop-host\.ps1|demo-stream\.ps1)\b|--ignored\b|--run-ignored\b)",
    re.IGNORECASE,
)


def _candidate_agents_dirs(
    agents_dir: pathlib.Path,
    cwd: str | None,
    workspace_paths: list[str] | None = None,
) -> list[pathlib.Path]:
    dirs = [agents_dir.resolve()]
    candidates: list[str] = []
    if cwd:
        candidates.append(cwd)
    if workspace_paths:
        candidates.extend(workspace_paths)
    for raw in candidates:
        cur = pathlib.Path(raw).resolve()
        for parent in (cur, *cur.parents):
            cand = parent / ".agents"
            if cand.is_dir() and cand not in dirs:
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
    """Guard `read_file`, `write_file`, and `edit_file`."""
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
    if tool_name in ("write_file", "edit_file") and (
        PROTECTED_GUARD_PATH_RE.search(path_value) or PROTECTED_GUARD_PATH_RE.search(norm)
    ):
        return {
            "decision": "deny",
            "reason": (
                f"Hard-denied ({tool_name}): modifying sentinels (.agents/RELEASE_7_0_AUTHORIZED, "
                ".agents/GPU_HANG_HALT), hook guards (.agents/hooks*), or architecture baselines "
                "(tools/size_ratchet.json, tools/arch_baselines.json) is forbidden."
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
        if PROTECTED_SENTINEL_OR_BASELINE_CMD_RE.search(segment) and not READ_ONLY_SHELL_RE.match(segment):
            return {
                "decision": "deny",
                "reason": "Hard-denied: shell commands may not create, edit, or delete sentinels, hook guards, or size/arch baseline files.",
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
                # Bare `git push`, `git push origin`, or `git push origin HEAD` is fail-closed unless targeting wip/ or roadmap/.
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

    # --- Tier 3: GPU / hardware test gate ---
    if any(HW_CMD_RE.search(seg) for seg in segments):
        if gpu_halted:
            return {
                "decision": "deny",
                "reason": f"Hard-denied (GPU/HW test): .agents/{GPU_HANG_SENTINEL} is active (exit code 3 encoder hang). Investigate, do not retry.",
            }
        if env_map.get(HW_GATE_ENV) != "1":
            return {
                "decision": "deny",
                "reason": f"Hard-denied (GPU/HW test): requires process environment {HW_GATE_ENV}=1 after host idle verification.",
            }

    return {"decision": "allow"}


def handle_payload(
    payload: dict[str, Any],
    *,
    agents_dir: pathlib.Path,
    env: Mapping[str, str] | None = None,
) -> dict[str, str]:
    tool_call = payload.get("toolCall")
    if not isinstance(tool_call, dict):
        return {"decision": "deny", "reason": "guard_commands: missing toolCall object in hook payload."}
    tool_name = tool_call.get("name")
    args = tool_call.get("args")
    if not isinstance(args, dict):
        return {"decision": "deny", "reason": "guard_commands: missing toolCall.args object."}

    if tool_name in ("read_file", "write_file", "edit_file"):
        path_val = args.get("Path") or args.get("path") or ""
        if not isinstance(path_val, str):
            return {"decision": "deny", "reason": f"guard_commands: {tool_name} Path must be a string."}
        return evaluate_file_tool(tool_name, path_val)

    if tool_name != "run_command":
        return {"decision": "allow"}

    command_line = args.get("CommandLine") or args.get("commandLine") or ""
    if not isinstance(command_line, str):
        return {"decision": "deny", "reason": "guard_commands: CommandLine must be a string."}
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
        print(json.dumps({"decision": "deny", "reason": f"guard_commands: invalid JSON payload ({exc})."}))
        return 0
    decision = handle_payload(payload, agents_dir=default_agents_dir)
    print(json.dumps(decision))
    return 0


if __name__ == "__main__":
    sys.exit(main())
