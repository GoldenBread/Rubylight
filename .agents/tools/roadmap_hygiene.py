#!/usr/bin/env python3
r"""Autonomous resource, PC+laptop scheduling, garbage collection, and firewall-safety helper.

Commands:
  python .agents/tools/roadmap_hygiene.py status
  python .agents/tools/roadmap_hygiene.py pick-lane [--no-ssh]
  python .agents/tools/roadmap_hygiene.py clean [--apply] [--max-log-age-days 3] [--min-free-gb 50] [--remote-laptop]
  python .agents/tools/roadmap_hygiene.py check-firewall [--base origin/main]

Designed for long autonomous Claude Code runs across Desktop PC (`C:\src\rl-target-pool`,
4 Cargo lanes / 1 Gradle / 1 Web) and Laptop (`ramaz@192.168.4.12` via `tools\remote-check.ps1`)
so disk garbage never piles up, shared build slots are used effectively, and rebuilt test
binaries never trigger Windows Defender Firewall prompts.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import time
from typing import Any

REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOLS_DIR = REPO_ROOT / "tools"
LAPTOP_SSH = "ramaz@192.168.4.12"
LAPTOP_REPO = r"C:\src\Rubylight"
LAPTOP_WORK = r"C:\src\rl-check"
LAPTOP_LOCKS = r"C:\src\butterpollo-release"
LAPTOP_POOL = r"C:\src\rl-target-pool"
DEV_CHECK_ROOT = pathlib.Path(r"C:\src\butterpollo-release\unified\dev-check")
SCRATCH_TEMP_ROOTS = (pathlib.Path(r"D:\t"),)

# Private build-output directories that MUST NOT pile up inside checkouts or `.claude/worktrees/*`.
PRIVATE_GARBAGE_RELDIRS = (
    "target",
    "rust/service/target",
    "client/protocol/target",
    "client/app/build",
    "client/app/.cxx",
    "rust/web/dist",
    "site/_site",
)

# Files/paths where wildcard binds (`0.0.0.0`, `[::]`, `UNSPECIFIED`) in test/probe binaries
# trigger Windows Defender Firewall UAC popups on every Cargo rebuild.
TEST_OR_PROBE_PATH_RE = re.compile(
    r"(?:^|/)(?:tests?/|probes?/|scripts?/|tools/)|(?:_tests?|_test|Tests?|Test|probe)\.(?:rs|py|java|kt|mjs|cjs|js|ts)$",
    re.IGNORECASE,
)
WILDCARD_BIND_RE = re.compile(
    r"""(?x)
    \b(?:bind|listen|TcpListener::bind|UdpSocket::bind|ServerSocket|DatagramSocket)\s*\([^;\n]*
      (?:0\.0\.0\.0|\[::\]|Ipv4Addr::UNSPECIFIED|Ipv6Addr::UNSPECIFIED|INADDR_ANY|in6addr_any)
    | (?:SocketAddr(?:V4|V6)?::new|IpAddr::from)\s*\([^;\n]*(?:Ipv4Addr::UNSPECIFIED|Ipv6Addr::UNSPECIFIED|\[0,\s*0,\s*0,\s*0\])
    | "0\.0\.0\.0:\d+"
    | "\[::\]:\d+"
    | \bhttp\.server\b(?![^\n]*?(?:-b|--bind)\s+(?:127\.0\.0\.1|localhost|::1))
    | \bvite\b[^\n]*?--host(?:\s+0\.0\.0\.0|\s*$)
    """,
    re.IGNORECASE,
)
FIREWALL_OK_RE = re.compile(r"(?://|#)\s*firewall-ok\s*:", re.IGNORECASE)
TMP_BRANCH_RE = re.compile(r"^wip/roadmap/[0-9]{2}-.+-(?:s\d+-.+|stage-\d+|p5-.+)$")


def _load_build_lock():
    if str(TOOLS_DIR) not in sys.path:
        sys.path.insert(0, str(TOOLS_DIR))
    import build_lock  # type: ignore

    return build_lock


def _disk_free_gb(path: str | pathlib.Path) -> float | None:
    try:
        usage = shutil.disk_usage(path)
        return round(usage.free / (2**30), 1)
    except OSError:
        return None


def _windows_ssh_exe() -> str:
    sys_root = os.environ.get("SystemRoot", r"C:\Windows")
    cand = pathlib.Path(sys_root) / "System32" / "OpenSSH" / "ssh.exe"
    return str(cand) if cand.is_file() else "ssh"


def _local_lane_snapshot(repo_root: pathlib.Path = REPO_ROOT) -> dict[str, Any]:
    bl = _load_build_lock()
    lock_dir = bl.lock_directory()
    build_lock_holder = bl.read(lock_dir / "BUILD-LOCK")
    priority_text = bl.read(lock_dir / "PRIORITY")
    priority_live = bool(priority_text and bl.priority_alive(priority_text))
    cargo_lanes = bl.cargo_lanes(lock_dir)
    cargo_busy = sum(1 for lane in cargo_lanes if lane.busy())
    gradle_lanes = bl.gradle_lanes(lock_dir)
    gradle_busy = any(lane.busy() for lane in gradle_lanes)
    web_lanes = bl.web_lanes(lock_dir)
    web_busy = any(lane.busy() for lane in web_lanes)
    return {
        "build_lock_free": build_lock_holder is None,
        "build_lock_holder": build_lock_holder,
        "priority_live": priority_live,
        "cargo_total": len(cargo_lanes),
        "cargo_busy": cargo_busy,
        "cargo_free": max(0, len(cargo_lanes) - cargo_busy),
        "gradle_free": not gradle_busy,
        "web_free": not web_busy,
        "c_free_gb": _disk_free_gb("C:\\" if os.name == "nt" else str(repo_root)),
        "d_free_gb": _disk_free_gb("D:\\") if os.name == "nt" else None,
        "target_pool": str(bl.pool_directory(lock_dir)),
    }


def _encode_ps(script: str) -> str:
    """Encode a PowerShell script as UTF-16LE Base64 for quote-safe `-EncodedCommand` over SSH."""
    return base64.b64encode(script.encode("utf-16-le")).decode("ascii")


def _probe_laptop(ssh_target: str = LAPTOP_SSH, timeout_s: int = 6) -> dict[str, Any]:
    """Non-blocking probe of laptop availability (locks, active lanes, console session, free disk)."""
    ps = (
        f"$locks='{LAPTOP_LOCKS}'; $pool='{LAPTOP_POOL}'; $busy=@();"
        "foreach($l in 'BUILD-LOCK','LAPTOP-LOCK','PRIORITY'){"
        "  $p=Join-Path $locks $l; if(Test-Path -LiteralPath $p){"
        "    $t=Get-Content -LiteralPath $p -Raw -ErrorAction SilentlyContinue;"
        "    if($l -eq 'PRIORITY' -and $t -match 'pid=(\\d+)' -and -not (Get-Process -Id ([int]$Matches[1]) -ErrorAction SilentlyContinue)){continue};"
        "    $busy+=$l"
        "  }"
        "};"
        "foreach($d in (Join-Path $locks 'lanes'),$pool){"
        "  if(Test-Path -LiteralPath $d){foreach($f in Get-ChildItem -LiteralPath $d -Filter *.lock){"
        "    try{[IO.File]::Open($f.FullName,'Open','ReadWrite','None').Dispose()}catch{$busy+=$f.Name}"
        "  }}"
        "};"
        "foreach($t in Get-ScheduledTask -TaskName 'rl-gate-*','rl-remote-check-*' -ErrorAction SilentlyContinue){"
        "  if($t.State -eq 'Running'){$busy+=$t.TaskName}"
        "};"
        "$u=(Get-CimInstance Win32_ComputerSystem).UserName;"
        "$cFree=[math]::Round((Get-PSDrive C).Free/1GB,1);"
        "Write-Output (@{busy=@($busy);user=$u;c_free_gb=$cFree}|ConvertTo-Json -Compress)"
    )
    cmd = [
        _windows_ssh_exe(),
        "-o",
        "BatchMode=yes",
        "-o",
        f"ConnectTimeout={timeout_s}",
        ssh_target,
        f"powershell -NoProfile -NonInteractive -EncodedCommand {_encode_ps(ps)}",
    ]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout_s + 8)
    except Exception as exc:
        return {"checked": True, "reachable": False, "available": False, "reason": f"ssh error: {exc}"}
    if proc.returncode != 0:
        err = (proc.stderr or proc.stdout or "").strip().splitlines()[-1:]
        return {
            "checked": True,
            "reachable": False,
            "available": False,
            "reason": f"ssh rc={proc.returncode}: {err[0] if err else 'unreachable'}",
        }
    try:
        raw = [ln for ln in proc.stdout.splitlines() if ln.strip()][-1]
        info = json.loads(raw)
    except Exception as exc:
        return {"checked": True, "reachable": True, "available": False, "reason": f"parse error: {exc}"}
    busy = info.get("busy") or []
    if isinstance(busy, str):
        busy = [busy]
    user = str(info.get("user") or "")
    console_ok = user.lower().endswith("\\ramaz") or user.lower() == "ramaz"
    c_free = info.get("c_free_gb")
    if busy:
        return {
            "checked": True,
            "reachable": True,
            "available": False,
            "c_free_gb": c_free,
            "reason": f"busy: {', '.join(busy[:4])}",
        }
    if not console_ok:
        return {
            "checked": True,
            "reachable": True,
            "available": False,
            "c_free_gb": c_free,
            "reason": f"no interactive ramaz console session (user={user!r})",
        }
    if isinstance(c_free, (int, float)) and c_free < 20:
        return {
            "checked": True,
            "reachable": True,
            "available": False,
            "c_free_gb": c_free,
            "reason": f"low disk on laptop C: ({c_free} GB free)",
        }
    return {
        "checked": True,
        "reachable": True,
        "available": True,
        "c_free_gb": c_free,
        "reason": "idle (ramaz at console)",
    }


def decide_lane(local: dict[str, Any], laptop: dict[str, Any]) -> dict[str, Any]:
    """Pure decision policy for optimal PC + laptop resource sharing."""
    local_ok = bool(local.get("build_lock_free")) and not bool(local.get("priority_live"))
    cargo_free = int(local.get("cargo_free", 0))
    gradle_free = bool(local.get("gradle_free", True))
    laptop_ok = bool(laptop.get("available"))
    c_free = local.get("c_free_gb")
    d_free = local.get("d_free_gb")
    disk_warn = (isinstance(c_free, (int, float)) and c_free < 50) or (isinstance(d_free, (int, float)) and d_free < 15)

    if not local_ok:
        if laptop_ok:
            rec, reason = "laptop", "PC exclusive BUILD-LOCK/PRIORITY active; offload full check to laptop"
        else:
            rec, reason = "wait", "PC BUILD-LOCK/PRIORITY active and laptop unavailable"
    elif cargo_free >= 2 and gradle_free:
        rec = "local_and_laptop" if laptop_ok else "local"
        reason = (
            f"PC has {cargo_free}/{local.get('cargo_total')} free Cargo lanes + Gradle free"
            + ("; laptop also idle for offloading full checks (`tools\\remote-check.ps1`)" if laptop_ok else "")
        )
    elif laptop_ok:
        rec = "laptop"
        reason = f"PC lanes saturated (cargo_free={cargo_free}, gradle_free={gradle_free}); offload to laptop"
    elif cargo_free >= 1:
        rec = "local"
        reason = f"PC has {cargo_free} Cargo slot free (laptop unavailable: {laptop.get('reason', 'n/a')})"
    else:
        rec = "wait"
        reason = "All PC Cargo slots busy and laptop unavailable"

    return {
        "recommended_lane": rec,
        "reason": reason,
        "disk_warning": disk_warn,
        "local": local,
        "laptop": laptop,
    }


def find_private_garbage(repo_root: pathlib.Path) -> list[pathlib.Path]:
    """Find private per-worktree build caches inside `repo_root` and `.claude/worktrees/*`."""
    roots = [repo_root]
    wt_dir = repo_root / ".claude" / "worktrees"
    if wt_dir.is_dir():
        roots.extend(sorted(p for p in wt_dir.iterdir() if p.is_dir()))
    found: list[pathlib.Path] = []
    for base in roots:
        for rel in PRIVATE_GARBAGE_RELDIRS:
            cand = base / rel
            if cand.is_dir():
                found.append(cand)
    return found


def find_expired_logs_and_scratch(
    *,
    dev_check_root: pathlib.Path = DEV_CHECK_ROOT,
    scratch_roots: tuple[pathlib.Path, ...] = SCRATCH_TEMP_ROOTS,
    max_age_days: float = 3.0,
    now_ts: float | None = None,
) -> list[pathlib.Path]:
    cutoff = (time.time() if now_ts is None else now_ts) - max_age_days * 86400.0
    expired: list[pathlib.Path] = []
    if dev_check_root.is_dir():
        for entry in dev_check_root.iterdir():
            if "wip-roadmap" not in entry.name.lower() and "roadmap" not in entry.name.lower():
                continue
            try:
                if entry.stat().st_mtime < cutoff:
                    expired.append(entry)
            except OSError:
                continue
    for sroot in scratch_roots:
        if not sroot.is_dir():
            continue
        for entry in sroot.iterdir():
            if not (entry.name.startswith("rl-review-") or entry.name.startswith("review-") or entry.name.startswith("rl-carbon-")):
                continue
            try:
                if entry.stat().st_mtime < cutoff:
                    expired.append(entry)
            except OSError:
                continue
    return sorted(expired)


def scan_diff_for_firewall_risks(diff_text: str) -> list[dict[str, Any]]:
    """Scan unified diff text for newly added wildcard binds in tests/probes/scripts."""
    findings: list[dict[str, Any]] = []
    cur_file: str | None = None
    cur_line = 0
    in_cfg_test = False

    for raw in diff_text.splitlines():
        if raw.startswith("+++ b/"):
            cur_file = raw[6:].strip()
            cur_line = 0
            in_cfg_test = False
            continue
        if raw.startswith("@@ "):
            m = re.search(r"\+(\d+)", raw)
            cur_line = int(m.group(1)) - 1 if m else 0
            continue
        if raw.startswith("+") and not raw.startswith("+++"):
            cur_line += 1
            added = raw[1:]
            if cur_file is None:
                continue
            if "#[cfg(test)]" in added or "mod tests" in added:
                in_cfg_test = True
            if FIREWALL_OK_RE.search(added):
                continue
            is_test_or_probe = bool(TEST_OR_PROBE_PATH_RE.search(cur_file)) or in_cfg_test
            if is_test_or_probe and WILDCARD_BIND_RE.search(added):
                findings.append(
                    {
                        "file": cur_file,
                        "line": cur_line,
                        "text": added.strip()[:140],
                        "fix": "Bind to loopback (`127.0.0.1` / `[::1]` / `Ipv4Addr::LOCALHOST`) or use IP-helper tables; never wildcard `0.0.0.0`/`[::]` in tests or probes.",
                    }
                )
        elif not raw.startswith("-"):
            cur_line += 1
    return findings


def _run_remote_laptop_clean(*, apply: bool) -> dict[str, Any]:
    probe = _probe_laptop()
    if not probe.get("reachable"):
        return {"skipped": True, "reason": probe.get("reason", "unreachable")}
    if not probe.get("available"):
        return {"skipped": True, "reason": f"laptop busy/unsafe to clean: {probe.get('reason')}"}
    ps = (
        f"$work='{LAPTOP_WORK}'; $repo='{LAPTOP_REPO}'; $removed=@();"
        "if(Test-Path -LiteralPath $work){"
        "  foreach($d in Get-ChildItem -LiteralPath $work -Directory -Filter 'wip-roadmap-*' -ErrorAction SilentlyContinue){"
        f"    if({'$true' if apply else '$false'}){{"
        "      git -C $repo worktree remove --force $d.FullName 2>$null | Out-Null;"
        "      if(Test-Path -LiteralPath $d.FullName){Remove-Item -Recurse -Force -LiteralPath $d.FullName -ErrorAction SilentlyContinue}"
        "    };"
        "    $removed+=$d.Name"
        "  }"
        "};"
        f"if({'$true' if apply else '$false'}){{"
        "  git -C $repo worktree prune 2>$null | Out-Null;"
        "  if((Get-PSDrive C).Free/1GB -lt 45){"
        f"    python '{LAPTOP_REPO}\\tools\\build_lock.py' --prune --min-free-gb 45 2>$null | Out-Null"
        "  }"
        "};"
        "Write-Output (@{apply=$"
        + ("true" if apply else "false")
        + ";removed=@($removed);c_free_gb=[math]::Round((Get-PSDrive C).Free/1GB,1)}|ConvertTo-Json -Compress)"
    )
    cmd = [
        _windows_ssh_exe(),
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=8",
        LAPTOP_SSH,
        f"powershell -NoProfile -NonInteractive -EncodedCommand {_encode_ps(ps)}",
    ]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=60)
        if proc.returncode != 0:
            return {"skipped": True, "reason": f"remote clean rc={proc.returncode}"}
        raw = [ln for ln in proc.stdout.splitlines() if ln.strip()][-1]
        return json.loads(raw)
    except Exception as exc:
        return {"skipped": True, "reason": f"remote clean error: {exc}"}


def cmd_status(repo_root: pathlib.Path) -> int:
    local = _local_lane_snapshot(repo_root)
    garbage = [str(p) for p in find_private_garbage(repo_root)]
    wt_out = subprocess.run(["git", "-C", str(repo_root), "worktree", "list", "--porcelain"], capture_output=True, text=True)
    wt_count = sum(1 for ln in wt_out.stdout.splitlines() if ln.startswith("worktree "))
    cc_wt_dir = repo_root / ".claude" / "worktrees"
    cc_wts = sorted(p.name for p in cc_wt_dir.iterdir() if p.is_dir()) if cc_wt_dir.is_dir() else []
    out = {
        "local": local,
        "git_worktree_count": wt_count,
        "claude_worktrees": cc_wts,
        "private_garbage_dirs": garbage,
    }
    print(json.dumps(out, indent=2))
    return 0


def cmd_pick_lane(repo_root: pathlib.Path, *, no_ssh: bool) -> int:
    local = _local_lane_snapshot(repo_root)
    laptop = {"checked": False, "reachable": False, "available": False, "reason": "skipped (--no-ssh)"} if no_ssh else _probe_laptop()
    print(json.dumps(decide_lane(local, laptop), indent=2))
    return 0


def cmd_clean(
    repo_root: pathlib.Path,
    *,
    apply: bool,
    max_log_age_days: float,
    min_free_gb: float,
    remote_laptop: bool,
) -> int:
    garbage = find_private_garbage(repo_root)
    expired = find_expired_logs_and_scratch(max_age_days=max_log_age_days)
    removed_garbage: list[str] = []
    removed_expired: list[str] = []
    pruned_branches: list[str] = []
    pool_pruned: list[str] = []

    for p in garbage:
        if apply:
            shutil.rmtree(p, ignore_errors=True)
        removed_garbage.append(str(p))

    for p in expired:
        if apply:
            if p.is_dir():
                shutil.rmtree(p, ignore_errors=True)
            else:
                p.unlink(missing_ok=True)
        removed_expired.append(str(p))

    # Find merged temporary slice branches (`wip/roadmap/NN-...-s1-*`, `*-stage-*`, `*-p5-*`).
    merged_proc = subprocess.run(
        ["git", "-C", str(repo_root), "branch", "--merged", "HEAD", "--format=%(refname:short)"],
        capture_output=True,
        text=True,
    )
    for br in merged_proc.stdout.splitlines():
        br = br.strip()
        if TMP_BRANCH_RE.match(br):
            if apply:
                subprocess.run(["git", "-C", str(repo_root), "branch", "-d", br], capture_output=True, text=True)
            pruned_branches.append(br)

    if apply:
        subprocess.run(["git", "-C", str(repo_root), "worktree", "prune"], capture_output=True, text=True)
        bl = _load_build_lock()
        pruned = bl.prune_pool(min_free_gb=min_free_gb)
        pool_pruned = [str(p) for p in pruned]

    remote_result = _run_remote_laptop_clean(apply=apply) if remote_laptop else None
    report = {
        "apply": apply,
        "private_garbage_dirs": removed_garbage,
        "expired_logs_and_scratch": removed_expired,
        "merged_temp_branches": pruned_branches,
        "pool_slots_pruned": pool_pruned,
        "c_free_gb": _disk_free_gb("C:\\" if os.name == "nt" else str(repo_root)),
        "d_free_gb": _disk_free_gb("D:\\") if os.name == "nt" else None,
        "remote_laptop": remote_result,
    }
    print(json.dumps(report, indent=2))
    return 0


def cmd_check_firewall(repo_root: pathlib.Path, *, base: str) -> int:
    proc = subprocess.run(
        ["git", "-C", str(repo_root), "diff", "-U0", base],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if proc.returncode != 0:
        print(f"check-firewall: git diff {base} failed: {proc.stderr.strip()}", file=sys.stderr)
        return 2
    findings = scan_diff_for_firewall_risks(proc.stdout)

    # Also scan untracked test/probe source files so brand-new tests are caught before staging.
    untracked = subprocess.run(
        ["git", "-C", str(repo_root), "ls-files", "--others", "--exclude-standard"],
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    for rel in untracked.stdout.splitlines():
        rel_posix = rel.replace("\\", "/")
        if not TEST_OR_PROBE_PATH_RE.search(rel_posix):
            continue
        p = repo_root / rel
        if not p.is_file():
            continue
        try:
            lines = p.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            continue
        synth = [f"+++ b/{rel_posix}", "@@ -0,0 +1,1 @@"] + [f"+{ln}" for ln in lines]
        findings.extend(scan_diff_for_firewall_risks("\n".join(synth)))

    if findings:
        print(json.dumps({"ok": False, "firewall_risks": findings}, indent=2))
        return 1
    print(json.dumps({"ok": True, "firewall_risks": []}, indent=2))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Rubylight 7.0.0 autonomous hygiene, lane picker & firewall guard.")
    parser.add_argument("--repo", default=str(REPO_ROOT), help="Repository/worktree root")
    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("status", help="Show local disk space, active build lanes, worktrees, and private cache garbage.")

    p_pick = sub.add_parser("pick-lane", help="Recommend PC local vs laptop remote-check lane from live utilization.")
    p_pick.add_argument("--no-ssh", action="store_true", help="Skip live SSH probe of laptop.")

    p_clean = sub.add_parser("clean", help="Clean private worktree caches, merged temp branches, expired logs & optional laptop worktrees.")
    p_clean.add_argument("--apply", action="store_true", help="Actually delete garbage (default is dry-run).")
    p_clean.add_argument("--max-log-age-days", type=float, default=3.0)
    p_clean.add_argument("--min-free-gb", type=float, default=50.0)
    p_clean.add_argument("--remote-laptop", action="store_true", help="Also prune finished wip-roadmap-* worktrees on laptop when idle.")

    p_fw = sub.add_parser("check-firewall", help="Fail closed if diff adds wildcard 0.0.0.0/[::] binds in tests or probes.")
    p_fw.add_argument("--base", default="origin/main", help="Git ref to diff against.")

    args = parser.parse_args(argv)
    repo_root = pathlib.Path(args.repo).resolve()

    if args.cmd == "status":
        return cmd_status(repo_root)
    if args.cmd == "pick-lane":
        return cmd_pick_lane(repo_root, no_ssh=args.no_ssh)
    if args.cmd == "clean":
        return cmd_clean(
            repo_root,
            apply=args.apply,
            max_log_age_days=args.max_log_age_days,
            min_free_gb=args.min_free_gb,
            remote_laptop=args.remote_laptop,
        )
    if args.cmd == "check-firewall":
        return cmd_check_firewall(repo_root, base=args.base)
    return 2


if __name__ == "__main__":
    sys.exit(main())
