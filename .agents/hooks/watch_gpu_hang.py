#!/usr/bin/env python3
"""Antigravity PostToolUse hook that detects AMD encoder hangs (exit code 3).

Watches all hardware / stream / soak runners (`e2e.py`, `check.ps1`, `tools/hw/run*`,
`tools/hw/laptop-host.ps1`, `soak.ps1`, `soak.py`, `focus_ab.py`, `wave1_verify.py`).
Checks direct PostToolUse error/exit fields AND falls back to reading `stepIdx` from
`payload['transcriptPath']` because normal non-zero command completions may record the
exit code only in the transcript step output.

When exit code 3 is detected, writes `.agents/GPU_HANG_HALT` in `default_agents_dir`
AND every `.agents` directory resolved from `workspacePaths` and `cwd`. Always prints
`{}` on stdout per Antigravity's PostToolUse contract.
"""
from __future__ import annotations

import datetime
import json
import os
import pathlib
import re
import sys
from typing import Any

GPU_HANG_SENTINEL = "GPU_HANG_HALT"
WATCHED_CMD_RE = re.compile(
    r"(?:(?:^|[\s\"'`/\\])(?:e2e\.py|check\.ps1|(?:[A-Za-z0-9_-]*_)?soak\.(?:ps1|py)|focus_ab\.py|wave1_verify\.py)\b|\btools[\\/]+hw[\\/]+(?:run(?:\.py|\.ps1)?|laptop-host\.ps1)\b)",
    re.IGNORECASE,
)
EXIT_CODE_3_RE = re.compile(
    r"(?:exit\s+(?:status|code)\s*[:=]?\s*3\b|exited\s+with\s+(?:status|code)\s*3\b|returncode\s*=\s*3\b|\brc\s*=\s*3\b)",
    re.IGNORECASE,
)


def _check_transcript_exit_3(payload: dict[str, Any]) -> bool:
    transcript_raw = payload.get("transcriptPath")
    if not isinstance(transcript_raw, str) or not transcript_raw:
        return False
    transcript_path = pathlib.Path(transcript_raw)
    if not transcript_path.is_file():
        return False
    target_step = payload.get("stepIdx")
    tool_id = (payload.get("toolCall") or {}).get("id")
    try:
        lines = transcript_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return False
    # Scan from the tail for matching stepIdx or toolCall id.
    for raw_line in reversed(lines[-200:]):
        if not raw_line.strip():
            continue
        try:
            entry = json.loads(raw_line)
        except Exception:
            continue
        if not isinstance(entry, dict):
            continue
        matched = False
        step_match = isinstance(target_step, int) and (
            entry.get("step_index") == target_step or entry.get("index") == target_step
        )
        if step_match:
            matched = True
        elif isinstance(tool_id, str) and tool_id and tool_id in raw_line:
            matched = True
        if not matched:
            continue
        for key in ("exitCode", "exit_code", "returnCode", "returncode"):
            if entry.get(key) in (3, "3"):
                return True
        for text_key in ("content", "error", "output", "result"):
            val = entry.get(text_key)
            if isinstance(val, str) and EXIT_CODE_3_RE.search(val):
                return True
        if step_match:
            return False
    return False


def _is_exit_code_3(payload: dict[str, Any]) -> bool:
    for key in ("exitCode", "exit_code", "returnCode", "returncode"):
        val = payload.get(key)
        if val == 3 or val == "3":
            return True
    tool_result = payload.get("toolResult")
    if isinstance(tool_result, dict):
        for key in ("exitCode", "exit_code", "returnCode", "returncode"):
            val = tool_result.get(key)
            if val == 3 or val == "3":
                return True
    for text_field in (
        payload.get("error"),
        payload.get("output"),
        tool_result.get("error") if isinstance(tool_result, dict) else None,
        tool_result.get("output") if isinstance(tool_result, dict) else None,
    ):
        if isinstance(text_field, str) and EXIT_CODE_3_RE.search(text_field):
            return True
    return _check_transcript_exit_3(payload)


def _resolve_target_agents_dirs(
    agents_dir: pathlib.Path,
    payload: dict[str, Any],
) -> list[pathlib.Path]:
    targets = [agents_dir.resolve()]
    roots: list[str] = []
    cwd = payload.get("cwd")
    if isinstance(cwd, str) and cwd:
        roots.append(cwd)
    ws = payload.get("workspacePaths")
    if isinstance(ws, list):
        roots.extend(p for p in ws if isinstance(p, str) and p)
    for raw in roots:
        cur = pathlib.Path(raw).resolve()
        for parent in (cur, *cur.parents):
            cand = parent / ".agents"
            if cand.is_dir():
                if cand not in targets:
                    targets.append(cand)
                break
    return targets


def process_post_tool(payload: dict[str, Any], *, agents_dir: pathlib.Path) -> bool:
    """Inspect a PostToolUse payload; return True if GPU_HANG_HALT was written."""
    tool_call = payload.get("toolCall")
    if not isinstance(tool_call, dict) or tool_call.get("name") != "run_command":
        return False
    args = tool_call.get("args")
    if not isinstance(args, dict):
        return False
    cmd = args.get("CommandLine") or args.get("commandLine") or ""
    if not isinstance(cmd, str) or not WATCHED_CMD_RE.search(cmd):
        return False
    if not _is_exit_code_3(payload):
        return False

    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    err = str(payload.get("error") or "Command exited with code 3").strip()
    body = (
        "AMD ENCODER HANG HALT (exit code 3)\n"
        f"timestamp_utc: {now}\n"
        f"command: {cmd.strip()}\n"
        f"error: {err}\n"
        "action_required: Investigate LiveKernelReports, GPU state (Device Manager Code 31), and host logs. "
        "Do NOT retry automatically. Remove this sentinel only after human clearance.\n"
    )
    for target_dir in _resolve_target_agents_dirs(agents_dir, payload):
        target_dir.mkdir(parents=True, exist_ok=True)
        (target_dir / GPU_HANG_SENTINEL).write_text(body, encoding="utf-8", newline="\n")
    return True


def main() -> int:
    default_agents_dir = pathlib.Path(
        os.environ.get("RUBYLIGHT_AGENTS_DIR") or pathlib.Path(__file__).resolve().parent.parent
    )
    raw = sys.stdin.read()
    try:
        payload = json.loads(raw)
        if isinstance(payload, dict):
            process_post_tool(payload, agents_dir=default_agents_dir)
    except Exception:
        pass
    print("{}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
