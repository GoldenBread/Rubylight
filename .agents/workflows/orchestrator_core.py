#!/usr/bin/env python3
"""Pure-Python core for the Rubylight/Nightjar 7.0.0 roadmap workflow orchestrator.

Provides:
- Fail-closed post-sprint prerequisite check (`arch_check.py`, `size_ratchet.json`, `dev.ps1 -Fast`).
- Resumable state persistence (`docs/roadmap-state.json`, atomic LF writes).
- Topological intra-wave scheduling & transitive dependency quarantine propagation.
- Concurrency capping (`Semaphore(min(max_parallel, 2))`), serialized `hw_lock` & `merge_lock`,
  and fault-isolated per-item execution inside `parallel()`.
- Deterministic git sub-branch handoffs across Phases 3-7 (`wip/roadmap/<id>-<slug>` so CI
  `on.push: [main, 'wip/**']` triggers cleanly; Phase 3 slices + Phase 5 l10n/docs/notes merge
  gates; Phase 6/7 run against dedicated checkouts of `{branch}`).
- Phase 8 strictly read-only Carbon (`gemini-4-carbon-high`) post-wave review with corrected
  `agy --model ... --print` flag order, background task await via `manage_task`, `git rev-parse HEAD`
  + `git status --porcelain -uall` verification, and up to 2 fix/re-verify/re-review cycles.
- Wave 6 fail-closed Release 7.0.0 gating with deterministic artifact assertions (`Cargo.toml`,
  `build.gradle` `versionName`/`versionCode > 404`, Play `<=500`-char changelog, filled `**Tested.**`),
  explicit maintainer authorization requirement (`authorize_publish=True`), and `try/finally`
  sentinel cleanup.
"""
from __future__ import annotations

import asyncio
import datetime
import json
import os
import pathlib
import re
import tempfile
from dataclasses import dataclass
from typing import Any, Awaitable, Callable, Iterable, Mapping

SCHEMA_VERSION = 1
RELEASE_TARGET = "7.0.0"
MIN_NEXT_ANDROID_VERSION_CODE = 406
INTEGRATION_BRANCH = "roadmap/redesign-and-waves-1-5"
CARBON_MODEL = "gemini-4-carbon-high"
MAX_CARBON_FIX_CYCLES = 2
MAX_CONCURRENCY_CAP = 2
PLAY_CHANGELOG_LIMIT = 500
REQUIRED_CARBON_WAVES = (0, 1, 2, 3, 4, 5)
GPU_HANG_SENTINEL = ".agents/GPU_HANG_HALT"
RELEASE_AUTH_SENTINEL = ".agents/RELEASE_7_0_AUTHORIZED"
WAVE_REVIEW_SCHEMA_REL = ".agents/schemas/wave_review.json"
STATE_REL = "docs/roadmap-state.json"

VALID_ITEM_STATUSES = frozenset({"pending", "in_progress", "merged", "quarantined", "failed"})
VALID_SEVERITIES = frozenset({"critical", "high", "medium"})
TASK_ID_RE = re.compile(r"task id:\s*(\S+)", re.IGNORECASE)

ITEM_CATALOG: tuple[dict[str, Any], ...] = (
    # Wave 0: Ground-Up 7.0 Console-Quality Complete Redesign
    {"id": "01", "slug": "ui-rubylight-studio", "wave": 0, "title": "Rubylight 7.0 Ground-Up Console-Quality Redesign (Console, /play, /guest, Couch Menu, Setup & Tray)", "design_doc": "docs/designs/01-ui-rubylight-studio.md", "depends_on": [], "hw_required": False, "visual_review": True},
    {"id": "02", "slug": "ui-nightjar-glass", "wave": 0, "title": "Nightjar 7.0 Ground-Up Console-Quality Redesign (Phone, Handheld, TV & Desktop)", "design_doc": "docs/designs/02-ui-nightjar-glass.md", "depends_on": [], "hw_required": False, "visual_review": True},
    # Wave 1: Core Stream & Capture Excellence
    {"id": "03", "slug": "launch-curtain", "wave": 1, "title": "Launch Curtain & Smart Focus", "design_doc": "docs/designs/03-launch-curtain.md", "depends_on": ["01", "02"], "hw_required": True, "visual_review": True},
    {"id": "04", "slug": "frame-warp-2", "wave": 1, "title": "Frame Warp 2.0", "design_doc": "docs/designs/04-frame-warp-2.md", "depends_on": [], "hw_required": True, "visual_review": False},
    {"id": "05", "slug": "stream-dvr", "wave": 1, "title": "Host-Side Stream DVR & Instant Replay", "design_doc": "docs/designs/05-stream-dvr.md", "depends_on": ["01", "02"], "hw_required": True, "visual_review": True},
    {"id": "06", "slug": "hdr-sdr-tonemap", "wave": 1, "title": "Real-Time HDR-to-SDR Tone Mapping", "design_doc": "docs/designs/06-hdr-sdr-tonemap.md", "depends_on": [], "hw_required": True, "visual_review": False},
    {"id": "21", "slug": "desktop-perf-and-nvenc", "wave": 1, "title": "Desktop Client Performance Overhaul (macOS, Linux, Windows) & Host NVIDIA NVENC Low-Latency Overhaul", "design_doc": "docs/designs/21-desktop-perf-and-nvenc.md", "depends_on": ["04", "06"], "hw_required": True, "visual_review": False},
    {"id": "22", "slug": "vibepollo-parity-core", "wave": 1, "title": "Vibepollo Protocol & Telemetry Parity Core (Launch Warnings, DXGI/PDH gpu_encoder_percent, FrameLimiterStatus & psmap)", "design_doc": "docs/designs/22-vibepollo-parity-core.md", "depends_on": [], "hw_required": False, "visual_review": False},
    {"id": "27", "slug": "security-and-privacy-hardening", "wave": 1, "title": "Host Network, WebRTC & API Security Hardening + Support-Bundle Privacy Sanitization", "design_doc": "docs/designs/27-security-and-privacy-hardening.md", "depends_on": [], "hw_required": False, "visual_review": False},
    {"id": "28", "slug": "host-capture-gpu-and-power", "wave": 1, "title": "Multi-GPU / Hybrid Laptop Routing, idd-push LUID Coherence & Locked-PC Cold Launch (#117)", "design_doc": "docs/designs/28-host-capture-gpu-and-power.md", "depends_on": ["03"], "hw_required": True, "visual_review": False},
    {"id": "29", "slug": "android-pacing-and-transport", "wave": 1, "title": "High-Refresh Transport Pacing, Android MediaCodec/ARR Pacing & WebRTC Transport Resilience (#144, #7, #119, #116)", "design_doc": "docs/designs/29-android-pacing-and-transport.md", "depends_on": ["04", "06", "27"], "hw_required": True, "visual_review": False},
    {"id": "07", "slug": "hw-verify", "wave": 1, "title": "Wave 1 Serialized Hardware Verification & Measurement", "design_doc": "docs/designs/07-hw-verify.md", "depends_on": ["03", "04", "05", "06", "21", "22", "27", "28", "29"], "hw_required": True, "visual_review": False},
    # Wave 2: Everyday Quality-of-Life, Multi-Device & Vibepollo/USB Interop
    {"id": "08", "slug": "quick-suspend", "wave": 2, "title": "Quick Suspend & Resume", "design_doc": "docs/designs/08-quick-suspend.md", "depends_on": ["03"], "hw_required": True, "visual_review": True},
    {"id": "09", "slug": "host-hub-sync", "wave": 2, "title": "Host Hub Sync (Profiles, Covers & Capabilities)", "design_doc": "docs/designs/09-host-hub-sync.md", "depends_on": ["01", "02"], "hw_required": False, "visual_review": True},
    {"id": "10", "slug": "couch-party-audio", "wave": 2, "title": "Couch Party Audio Routing & Spatial Mix", "design_doc": "docs/designs/10-couch-party-audio.md", "depends_on": ["02"], "hw_required": True, "visual_review": True},
    {"id": "11", "slug": "stream-doctor", "wave": 2, "title": "Stream Doctor 2.0 & Upscaler Guidance", "design_doc": "docs/designs/11-stream-doctor.md", "depends_on": ["02"], "hw_required": True, "visual_review": True},
    {"id": "12", "slug": "phone-safe-area", "wave": 2, "title": "Phone Safe Area & Reverse Landscape (#152)", "design_doc": "docs/designs/12-phone-safe-area.md", "depends_on": ["02"], "hw_required": False, "visual_review": True},
    {"id": "23", "slug": "vibepollo-dolby-rtsp", "wave": 2, "title": "Vibepollo Dolby Audio RTSP Negotiation & Unchunked Datagram Receive Parity", "design_doc": "docs/designs/23-vibepollo-dolby-rtsp.md", "depends_on": [], "hw_required": False, "visual_review": False},
    {"id": "24", "slug": "usb-forwarding-convenience", "wave": 2, "title": "USB Forwarding Convenience (Windows usbipd-win, Linux EACCES Fallback & Controller Arbitration)", "design_doc": "docs/designs/24-usb-forwarding-convenience.md", "depends_on": [], "hw_required": False, "visual_review": False},
    {"id": "25", "slug": "ps-speaker-pcm-and-bt", "wave": 2, "title": "PlayStation Controller Speaker (Bluetooth DualSense HID & Vibepollo 0x5602 / 0x5500_0009 Interop)", "design_doc": "docs/designs/25-ps-speaker-pcm-and-bt.md", "depends_on": ["22"], "hw_required": False, "visual_review": False},
    # Wave 3: Power Features, Steam Controller 2026 & Remote Reach
    {"id": "13", "slug": "client-input-deck", "wave": 3, "title": "Visual Console-Grade Controller Editor & Client Input Deck on First-Party Virtual Gamepad", "design_doc": "docs/designs/13-client-input-deck.md", "depends_on": ["09"], "hw_required": True, "visual_review": True},
    {"id": "14", "slug": "coherence-workstation", "wave": 3, "title": "Coherence Workstation Multi-Window Streaming", "design_doc": "docs/designs/14-coherence-workstation.md", "depends_on": ["03", "09"], "hw_required": True, "visual_review": True},
    {"id": "15", "slug": "away-wake-relay", "wave": 3, "title": "Away Wake Relay over Traverse Peer Mesh", "design_doc": "docs/designs/15-away-wake-relay.md", "depends_on": ["09"], "hw_required": False, "visual_review": True},
    {"id": "26", "slug": "steam-controller-2026", "wave": 3, "title": "Physical Steam Controller (2026) Direct HID Driver (Desktop & Android) + Host vhf_steam Polish", "design_doc": "docs/designs/26-steam-controller-2026.md", "depends_on": ["13", "24"], "hw_required": False, "visual_review": False},
    # Wave 4: 10/10 Polish & Resilience
    {"id": "16", "slug": "ui-final-polish", "wave": 4, "title": "UI Final Polish Pass (Console + Nightjar)", "design_doc": "docs/designs/16-ui-final-polish.md", "depends_on": ["01", "02", "03", "05", "08", "09", "10", "11", "12", "13", "14", "15", "22"], "hw_required": False, "visual_review": True},
    {"id": "17", "slug": "ux-craft-and-resilience", "wave": 4, "title": "UX Craft, Cold-Start, Installer, Errors & Crash-Restore Soak (#129)", "design_doc": "docs/designs/17-ux-craft-and-resilience.md", "depends_on": ["03", "07", "08", "12", "13", "24", "27", "28"], "hw_required": True, "visual_review": True},
    # Wave 5: Showcase, Sites & Docs 10/10
    {"id": "18", "slug": "showcase-demos-and-videos", "wave": 5, "title": "Showcase Demos, Tours & Videos (Console + Nightjar)", "design_doc": "docs/designs/18-showcase-demos-and-videos.md", "depends_on": ["16", "17"], "hw_required": False, "visual_review": True},
    {"id": "19", "slug": "websites-10-of-10", "wave": 5, "title": "Rubylight & Nightjar Websites 10/10", "design_doc": "docs/designs/19-websites-10-of-10.md", "depends_on": ["16", "18"], "hw_required": False, "visual_review": True},
    {"id": "20", "slug": "readmes-and-docs-10-of-10", "wave": 5, "title": "READMEs & Documentation 10/10", "design_doc": "docs/designs/20-readmes-and-docs-10-of-10.md", "depends_on": ["16", "17", "18", "19"], "hw_required": False, "visual_review": True},
)


class RoadmapError(RuntimeError):
    """Base error for fail-closed orchestrator halts."""


class CarbonReadOnlyViolationError(RoadmapError):
    """Raised if read-only Carbon review altered git working tree or HEAD state."""


class ReleaseGateError(RoadmapError):
    """Raised when Wave 6 fail-closed release checks do not pass."""


class GpuHangHaltError(RoadmapError):
    """Raised when `.agents/GPU_HANG_HALT` is detected."""


def utc_now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def item_branch_name(item_id: str, slug: str) -> str:
    """Use `wip/roadmap/...` so GitHub Actions `on.push: [main, 'wip/**']` runs automatically."""
    return f"wip/roadmap/{item_id}-{slug}"


def slice_branch_name(item_branch: str, stage_idx: int, slice_name: str) -> str:
    """Dash-delimited temporary ref name that never D/F-collides with `refs/heads/{item_branch}`."""
    safe_name = re.sub(r"[^a-zA-Z0-9_-]+", "-", slice_name).strip("-").lower()
    return f"{item_branch}-s{stage_idx}-{safe_name}"


def stage_merge_branch_name(item_branch: str, stage_idx: int) -> str:
    """Intermediate merged stage ref name (dash-delimited, zero D/F collision with `{item_branch}`)."""
    return f"{item_branch}-stage-{stage_idx}"


def phase5_branch_name(item_branch: str, kind: str) -> str:
    """Dash-delimited Phase 5 sub-branch that never D/F-collides with `refs/heads/{item_branch}`."""
    safe_kind = re.sub(r"[^a-zA-Z0-9_-]+", "-", kind).strip("-").lower()
    return f"{item_branch}-p5-{safe_kind}"


def verify_post_sprint_prereqs(repo_root: pathlib.Path) -> list[str]:
    """Fail-closed preflight asserting post-sprint guardrails exist before Wave 0."""
    missing: list[str] = []
    for rel in ("tools/arch_check.py", "tools/size_ratchet.json", "tools/dev.ps1"):
        if not (repo_root / rel).is_file():
            missing.append(f"Missing post-sprint file `{rel}` (rebase onto post-sprint `main` first)")
    dev_ps1 = repo_root / "tools/dev.ps1"
    if dev_ps1.is_file() and not re.search(r"\$Fast\b", dev_ps1.read_text(encoding="utf-8", errors="replace")):
        missing.append("`tools/dev.ps1` does not declare `[switch] $Fast` (rebase onto post-sprint `main` first)")
    return missing


def initial_state() -> dict[str, Any]:
    waves = {
        str(w): {
            "status": "pending",
            "base_sha": None,
            "head_sha": None,
            "carbon_review": {
                "approved": False,
                "review_file": f"docs/reviews/wave-{w}-carbon.md",
                "attempts": 0,
                "blocking_count": 0,
                "summary": "",
            },
        }
        for w in REQUIRED_CARBON_WAVES
    }
    waves["6"] = {
        "status": "pending",
        "base_sha": None,
        "head_sha": None,
        "carbon_review": {
            "approved": False,
            "review_file": "docs/reviews/release-7.0.0-carbon.md",
            "attempts": 0,
            "blocking_count": 0,
            "summary": "",
        },
    }
    items = []
    for entry in ITEM_CATALOG:
        items.append(
            {
                **entry,
                "depends_on": list(entry["depends_on"]),
                "status": "pending",
                "phase": "not_started",
                "branch": item_branch_name(entry["id"], entry["slug"]),
                "worktree": None,
                "commit_sha": None,
                "quarantine_reason": None,
                "hw_verified": False,
            }
        )
    return {
        "schema_version": SCHEMA_VERSION,
        "release_target": RELEASE_TARGET,
        "integration_branch": INTEGRATION_BRANCH,
        "updated_at": utc_now(),
        "waves": waves,
        "items": items,
        "release": {
            "version": RELEASE_TARGET,
            "rubylight_tag_pushed": False,
            "rubylight_hw_check_passed": False,
            "nightjar_mirrored": False,
            "nightjar_dry_run_passed": False,
            "nightjar_defender_clean": False,
            "nightjar_tag_pushed": False,
            "sites_published": False,
            "whole_release_carbon_approved": False,
            "maintainer_authorized": False,
        },
    }


def load_state(state_path: pathlib.Path) -> dict[str, Any]:
    if not state_path.exists():
        return initial_state()
    data = json.loads(state_path.read_text(encoding="utf-8"))
    if data.get("schema_version") != SCHEMA_VERSION:
        raise RoadmapError(f"Unsupported roadmap-state schema_version: {data.get('schema_version')}")
    return data


def save_state(state_path: pathlib.Path, state: dict[str, Any]) -> None:
    state["updated_at"] = utc_now()
    state_path.parent.mkdir(parents=True, exist_ok=True)
    tmp = state_path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8", newline="\n")
    os.replace(tmp, state_path)


def item_by_id(state: Mapping[str, Any], item_id: str) -> dict[str, Any]:
    for item in state["items"]:
        if item["id"] == item_id:
            return item
    raise KeyError(f"Unknown roadmap item id: {item_id}")


def quarantine_item(state: dict[str, Any], item_id: str, reason: str) -> list[str]:
    """Mark `item_id` quarantined and transitively quarantine dependent pending/in_progress items."""
    changed: list[str] = []
    target = item_by_id(state, item_id)
    if target["status"] != "quarantined":
        target["status"] = "quarantined"
        target["quarantine_reason"] = reason
        changed.append(item_id)

    propagated = True
    while propagated:
        propagated = False
        bad_ids = {i["id"] for i in state["items"] if i["status"] in ("quarantined", "failed")}
        for item in state["items"]:
            if item["status"] in ("pending", "in_progress"):
                blocked_by = [dep for dep in item["depends_on"] if dep in bad_ids]
                if blocked_by:
                    item["status"] = "quarantined"
                    item["quarantine_reason"] = f"blocked by quarantined dependency: {', '.join(blocked_by)}"
                    changed.append(item["id"])
                    propagated = True
    return changed


def propagate_dependency_quarantines(state: dict[str, Any]) -> list[str]:
    changed: list[str] = []
    bad_ids = {i["id"] for i in state["items"] if i["status"] in ("quarantined", "failed")}
    for bad in sorted(bad_ids):
        for item in state["items"]:
            if item["status"] in ("pending", "in_progress") and bad in item["depends_on"]:
                changed.extend(quarantine_item(state, item["id"], f"blocked by quarantined dependency: {bad}"))
    return sorted(set(changed))


def next_ready_batch(state: Mapping[str, Any], wave: int) -> list[dict[str, Any]]:
    merged_ids = {i["id"] for i in state["items"] if i["status"] == "merged"}
    ready: list[dict[str, Any]] = []
    for item in state["items"]:
        if item["wave"] != wave:
            continue
        if item["status"] not in ("pending", "in_progress"):
            continue
        if all(dep in merged_ids for dep in item["depends_on"]):
            ready.append(item)
    return ready


def wave_unmerged_items(state: Mapping[str, Any], wave: int) -> list[dict[str, Any]]:
    return [i for i in state["items"] if i["wave"] == wave and i["status"] != "merged"]


def check_gpu_hang_halt(repo_root: pathlib.Path) -> None:
    halt = repo_root / GPU_HANG_SENTINEL
    if halt.exists():
        raise GpuHangHaltError(
            f"Halted: {GPU_HANG_SENTINEL} exists after an AMD encoder hang (exit code 3). "
            "Investigate GPU state and clear manually before resuming."
        )


def check_wave_entry_gate(state: Mapping[str, Any], wave: int, repo_root: pathlib.Path) -> list[str]:
    reasons: list[str] = []
    if (repo_root / GPU_HANG_SENTINEL).exists():
        reasons.append(f"{GPU_HANG_SENTINEL} sentinel is present")
    if wave > 0:
        for prev in range(0, min(wave, 6)):
            prev_unmerged = wave_unmerged_items(state, prev)
            if prev_unmerged:
                reasons.append(
                    f"Wave {prev} has unmerged items: {', '.join(i['id'] + '(' + i['status'] + ')' for i in prev_unmerged)}"
                )
            prev_wave = state["waves"].get(str(prev), {})
            review = prev_wave.get("carbon_review", {})
            if not review.get("approved") or review.get("blocking_count", 0) != 0:
                reasons.append(f"Wave {prev} Carbon review is not approved")
    return reasons


def validate_play_changelog(text: str) -> None:
    clean = text.replace("\r\n", "\n").rstrip("\n")
    if not clean.strip():
        raise ReleaseGateError("Google Play en-US changelog is empty")
    if len(clean) > PLAY_CHANGELOG_LIMIT:
        raise ReleaseGateError(
            f"Google Play en-US changelog is {len(clean)} chars (limit {PLAY_CHANGELOG_LIMIT})"
        )


def verify_release_artifacts(
    repo_root: pathlib.Path,
    *,
    min_version_code: int = MIN_NEXT_ANDROID_VERSION_CODE,
) -> list[str]:
    """Deterministically verify 7.0.0 version bumps, Play <=500 changelog, and filled Tested paragraph."""
    errors: list[str] = []
    cargo_path = repo_root / "Cargo.toml"
    if not cargo_path.is_file():
        errors.append("Missing root Cargo.toml")
    else:
        m_ver = re.search(r'(?m)^version\s*=\s*"([^"]+)"', cargo_path.read_text(encoding="utf-8"))
        if not m_ver or m_ver.group(1) != RELEASE_TARGET:
            errors.append(f"Cargo.toml workspace version must be {RELEASE_TARGET} (found {m_ver.group(1) if m_ver else 'none'})")

    gradle_path = repo_root / "client/app/build.gradle"
    version_code: int | None = None
    if not gradle_path.is_file():
        errors.append("Missing client/app/build.gradle")
    else:
        gtext = gradle_path.read_text(encoding="utf-8")
        m_vn = re.search(r'(?m)^\s*versionName\s+"([^"]+)"', gtext)
        m_vc = re.search(r"(?m)^\s*versionCode\s*=?\s*(\d+)", gtext)
        if not m_vn or m_vn.group(1) != RELEASE_TARGET:
            errors.append(f"client/app/build.gradle versionName must be {RELEASE_TARGET}")
        if not m_vc:
            errors.append("client/app/build.gradle missing versionCode")
        else:
            version_code = int(m_vc.group(1))
            if version_code < min_version_code:
                errors.append(
                    f"client/app/build.gradle versionCode ({version_code}) must be >= {min_version_code}"
                )

    if version_code is not None:
        play_path = repo_root / f"client/fastlane/metadata/android/en-US/changelogs/{version_code}.txt"
        if not play_path.is_file():
            errors.append(f"Missing Google Play changelog: {play_path.relative_to(repo_root).as_posix()}")
        else:
            try:
                validate_play_changelog(play_path.read_text(encoding="utf-8"))
            except ReleaseGateError as exc:
                errors.append(str(exc))

    whats_new = repo_root / "docs/reviews/nightjar-7.0.0-whats-new.md"
    if not whats_new.is_file():
        errors.append("Missing Nightjar release notes draft: docs/reviews/nightjar-7.0.0-whats-new.md")
    else:
        wtext = whats_new.read_text(encoding="utf-8")
        m_tested = re.search(r"(?m)^\*\*Tested\.\*\*[ \t]*(.+)$", wtext)
        if not m_tested or len(m_tested.group(1).strip()) < 10:
            errors.append("docs/reviews/nightjar-7.0.0-whats-new.md has an unfilled `**Tested.**` paragraph")

    return errors


def evaluate_release_gate(
    state: Mapping[str, Any],
    repo_root: pathlib.Path,
    *,
    rubylight_remote_tags: Iterable[str] = (),
    nightjar_remote_tags: Iterable[str] = (),
    require_whole_release_carbon: bool = False,
    check_release_artifacts: bool = False,
    authorize_publish: bool = True,
) -> list[str]:
    """Fail-closed Wave 6 release 7.0.0 preflight check. Returns blocking errors (empty == GO)."""
    errors: list[str] = []

    if (repo_root / GPU_HANG_SENTINEL).exists():
        errors.append(f"{GPU_HANG_SENTINEL} exists (active AMD encoder hang halt)")

    non_merged = [i for i in state["items"] if i["status"] != "merged"]
    if non_merged:
        summary = ", ".join(f"{i['id']}({i['status']})" for i in non_merged)
        errors.append(f"All {len(ITEM_CATALOG)} roadmap items must be merged before 7.0.0; remaining: {summary}")

    for w in REQUIRED_CARBON_WAVES:
        wdata = state["waves"].get(str(w), {})
        review = wdata.get("carbon_review", {})
        if not review.get("approved") or review.get("blocking_count", 0) != 0:
            errors.append(f"Wave {w} Carbon review not approved (blocking={review.get('blocking_count')})")
        review_file = review.get("review_file")
        if not review_file or not (repo_root / review_file).is_file():
            errors.append(f"Wave {w} Carbon review artifact missing: {review_file}")

    target_tag = f"v{RELEASE_TARGET}"
    target_ref = f"refs/tags/{target_tag}"
    for repo_name, tags in (("Rubylight", rubylight_remote_tags), ("Nightjar", nightjar_remote_tags)):
        tag_set = {t.strip() for t in tags}
        if target_tag in tag_set or target_ref in tag_set:
            errors.append(f"{repo_name} already has tag {target_tag}; never move or overwrite a published tag")

    if check_release_artifacts:
        errors.extend(verify_release_artifacts(repo_root))

    if require_whole_release_carbon:
        rel_review = state["waves"].get("6", {}).get("carbon_review", {})
        if not rel_review.get("approved") or rel_review.get("blocking_count", 0) != 0:
            errors.append("Whole-release 7.0.0 Carbon review is not approved")
        rel_file = rel_review.get("review_file")
        if not rel_file or not (repo_root / rel_file).is_file():
            errors.append(f"Whole-release Carbon review artifact missing: {rel_file}")
        if not authorize_publish:
            errors.append(
                "Maintainer confirmation required (AGENTS.md): re-run Wave 6 with Args={'authorize_publish': True} "
                "after reviewing docs/reviews/release-7.0.0-carbon.md"
            )

    return errors


def check_host_idle_serverinfo(xml_text: str) -> tuple[bool, str]:
    m_active = re.search(r"<RustHostSessionCount>\s*(\d+)\s*</RustHostSessionCount>", xml_text)
    m_pending = re.search(r"<RustHostPendingSessionCount>\s*(\d+)\s*</RustHostPendingSessionCount>", xml_text)
    if not m_active or not m_pending:
        return False, "Missing RustHostSessionCount or RustHostPendingSessionCount in /serverinfo"
    active, pending = int(m_active.group(1)), int(m_pending.group(1))
    if active != 0 or pending != 0:
        return False, f"Installed host busy (active={active}, pending={pending})"
    return True, "Installed host idle (active=0, pending=0)"


def build_carbon_review_command(
    repo_root: pathlib.Path,
    prompt_file: pathlib.Path,
    output_file: pathlib.Path,
    *,
    schema_rel: str = WAVE_REVIEW_SCHEMA_REL,
) -> str:
    """Build strictly read-only Carbon CLI command with `--model` before `--print $prompt`.

    Scopes `--add-dir` exclusively to `repo_root` and writes JSON output to an external
    temp file so backgrounded execution never loses stdout.
    """
    schema_path = repo_root / schema_rel
    return (
        f'$p = Get-Content -Raw "{prompt_file}"; '
        f'& agy --model {CARBON_MODEL} --add-dir "{repo_root}" '
        f'--dangerously-skip-permissions --output-format json --json-schema "{schema_path}" '
        f'--print $p | Out-File -FilePath "{output_file}" -Encoding utf8; '
        'exit $LASTEXITCODE'
    )


def parse_carbon_review(raw_stdout: str) -> dict[str, Any]:
    """Parse and validate Carbon JSON output (direct or wrapped in agy JSON envelope)."""
    text = raw_stdout.lstrip("\ufeff").strip()
    if not text:
        raise RoadmapError("Carbon CLI returned empty output")
    obj = json.loads(text)
    if isinstance(obj, dict) and "approved" not in obj:
        for envelope_key in ("structured_output", "result", "response", "text", "output"):
            inner = obj.get(envelope_key)
            if isinstance(inner, dict) and "approved" in inner:
                obj = inner
                break
            if isinstance(inner, str) and inner.strip():
                candidate = inner.strip()
                if candidate.startswith("```"):
                    candidate = re.sub(r"^```(?:json)?\s*|\s*```$", "", candidate, flags=re.S)
                parsed_inner = json.loads(candidate)
                if isinstance(parsed_inner, dict) and "approved" in parsed_inner:
                    obj = parsed_inner
                    break

    if not isinstance(obj, dict):
        raise RoadmapError("Carbon review output is not a JSON object")
    for req in ("approved", "wave", "summary", "blocking_findings", "non_blocking"):
        if req not in obj:
            raise RoadmapError(f"Carbon review missing required field: {req}")
    if not isinstance(obj["approved"], bool):
        raise RoadmapError("Carbon review `approved` must be boolean")
    if not isinstance(obj["blocking_findings"], list) or not isinstance(obj["non_blocking"], list):
        raise RoadmapError("Carbon review findings lists must be arrays")
    for finding in obj["blocking_findings"]:
        if not isinstance(finding, dict):
            raise RoadmapError("Carbon blocking finding must be an object")
        for fkey in ("item_id", "severity", "file", "issue", "required_fix"):
            if fkey not in finding or not isinstance(finding[fkey], str):
                raise RoadmapError(f"Carbon blocking finding missing string field: {fkey}")
        if finding["severity"] not in VALID_SEVERITIES:
            raise RoadmapError(f"Invalid blocking finding severity: {finding['severity']}")

    if obj["blocking_findings"]:
        obj["approved"] = False
    return obj


def format_carbon_review_markdown(review: Mapping[str, Any], *, attempt: int, git_range: str) -> str:
    verdict = "APPROVED" if review["approved"] else "CHANGES REQUIRED"
    lines = [
        f"# Carbon Review: {review['wave']}",
        "",
        f"- **Model:** `{CARBON_MODEL}` (strictly read-only)",
        f"- **Verdict:** **{verdict}**",
        f"- **Attempt:** {attempt}",
        f"- **Range:** `{git_range}`",
        f"- **Timestamp (UTC):** {utc_now()}",
        "",
        "## Summary",
        "",
        str(review["summary"]).strip(),
        "",
        f"## Blocking Findings ({len(review['blocking_findings'])})",
        "",
    ]
    if not review["blocking_findings"]:
        lines.append("None.")
    else:
        for idx, f in enumerate(review["blocking_findings"], 1):
            lines.extend(
                [
                    f"### {idx}. [{f['severity'].upper()}] Item `{f['item_id']}` — `{f['file']}`",
                    f"- **Issue:** {f['issue']}",
                    f"- **Required fix:** {f['required_fix']}",
                    "",
                ]
            )
    lines.extend(["", f"## Non-Blocking Notes ({len(review['non_blocking'])})", ""])
    if not review["non_blocking"]:
        lines.append("None.")
    else:
        for note in review["non_blocking"]:
            lines.append(f"- {note}")
    lines.append("")
    return "\n".join(lines)


def assert_git_snapshot_unchanged(
    head_before: str,
    status_before: str,
    head_after: str,
    status_after: str,
    *,
    context: str,
) -> None:
    if head_before.strip() != head_after.strip() or status_before != status_after:
        raise CarbonReadOnlyViolationError(
            f"{context}: repository state changed during strictly read-only Carbon review!\n"
            f"HEAD before={head_before.strip()} after={head_after.strip()}\n"
            f"Status before:\n{status_before}\nStatus after:\n{status_after}"
        )


def group_slices_by_stage(slices: list[dict[str, Any]]) -> list[list[dict[str, Any]]]:
    """Group Phase 3 slices by optional integer `stage` (default 1) for staged parallel execution."""
    buckets: dict[int, list[dict[str, Any]]] = {}
    for slc in slices:
        stage = int(slc.get("stage", 1))
        buckets.setdefault(stage, []).append(slc)
    return [buckets[k] for k in sorted(buckets)]


@dataclass
class WorkflowRuntime:
    repo_root: pathlib.Path
    agent: Callable[..., Any]
    parallel: Callable[[list[Any], Callable[[Any], Awaitable[Any]]], Awaitable[list[Any]]]
    pipeline: Callable[[list[Any], Callable[[Any], Awaitable[Any]]], Awaitable[list[Any]]]
    phase: Callable[[str], None]
    log: Callable[..., None]
    call_tool: Callable[..., Awaitable[str]]


async def run_command_and_await(
    rt: WorkflowRuntime,
    command_line: str,
    *,
    summary: str,
    poll_seconds: float = 5.0,
    max_wait_seconds: float = 1800.0,
) -> str:
    """Run a shell command and await completion via `manage_task` if backgrounded after 10s."""
    out = await rt.call_tool(
        "run_command",
        CommandLine=command_line,
        WaitMsBeforeAsync=10000,
        toolSummary=summary,
    )
    m_task = TASK_ID_RE.search(out)
    if not m_task:
        return out
    task_id = m_task.group(1)
    waited = 0.0
    while waited < max_wait_seconds:
        await asyncio.sleep(poll_seconds)
        waited += poll_seconds
        st = await rt.call_tool("manage_task", Action="status", TaskId=task_id)
        if re.search(r"(?:status|state)\s*[:=]\s*(?:completed|done|exited|failed|error)", st, re.IGNORECASE):
            return st
    raise RoadmapError(f"Background command `{summary}` (task {task_id}) timed out after {max_wait_seconds:g}s")


async def _git_head_and_status(rt: WorkflowRuntime) -> tuple[str, str]:
    head = await rt.call_tool(
        "run_command",
        CommandLine=f'git -C "{rt.repo_root}" rev-parse HEAD',
        WaitMsBeforeAsync=5000,
        toolSummary="Read git HEAD",
    )
    status = await rt.call_tool(
        "run_command",
        CommandLine=f'git -C "{rt.repo_root}" status --porcelain -uall',
        WaitMsBeforeAsync=5000,
        toolSummary="Read git status -uall",
    )
    return head.strip(), status


async def run_item_lifecycle(
    rt: WorkflowRuntime,
    state: dict[str, Any],
    state_path: pathlib.Path,
    item: dict[str, Any],
    sem: asyncio.Semaphore,
    hw_lock: asyncio.Lock,
    merge_lock: asyncio.Lock,
    state_lock: asyncio.Lock,
    *,
    dry_run: bool = False,
) -> dict[str, Any]:
    """Execute Phases 0-7 for a single roadmap item with fault isolation and git branch handoffs."""
    item_id = item["id"]
    slug = item["slug"]
    design_doc = item["design_doc"]
    branch = item["branch"]

    async with sem:
        try:
            check_gpu_hang_halt(rt.repo_root)
            async with state_lock:
                item["status"] = "in_progress"
                item["phase"] = "phase_0_prep"
                save_state(state_path, state)

            if dry_run:
                rt.log(f"[dry-run] Item {item_id} ({slug}): Phases 0-7 simulated on {branch}")
                async with state_lock:
                    item["status"] = "merged"
                    item["phase"] = "phase_7_merged"
                    item["commit_sha"] = f"dryrun-{item_id}"
                    item["hw_verified"] = bool(item["hw_required"])
                    save_state(state_path, state)
                return {"id": item_id, "status": "merged"}

            # Phase 1.5: Spec Critic (read-only)
            async with state_lock:
                item["phase"] = "phase_1_5_spec_critic"
                save_state(state_path, state)
            critic = await rt.agent(
                prompt=(
                    f"Read `.agents/skills/rubylight-feature-spec/SKILL.md`, `docs/designs/00-protocol-reservations.md`, "
                    f"and `{design_doc}`. Critique item {item_id} ({slug}) against `AGENTS.md`, `client/AGENTS.md`, "
                    "`tools/arch_check.py`, and `tools/size_ratchet.json`. Do not edit files."
                ),
                schema={
                    "type": "object",
                    "required": ["approved", "issues"],
                    "properties": {"approved": {"type": "boolean"}, "issues": {"type": "array", "items": {"type": "string"}}},
                },
                role=f"Spec Critic {item_id}-{slug}",
                type_name="research",
                workspace="inherit",
            )
            if not critic.get("approved"):
                raise RoadmapError(f"Spec critic rejected {item_id}: {critic.get('issues')}")

            # Phase 2: Post-sprint code-map.md path verification BEFORE coding
            async with state_lock:
                item["phase"] = "phase_2_path_verify"
                save_state(state_path, state)
            plan = await rt.agent(
                prompt=(
                    f"Phase 2 dynamic post-sprint path & workflow verification for item {item_id} ({slug}) from `{design_doc}`. "
                    "Never trust pre-refactor monolithic file paths or line numbers: dynamically inspect the live post-sprint "
                    "`docs/code-map.md`, `client/docs/code-map.md`, `tools/dev.ps1` flags, `tools/arch_check.py`, and "
                    "`tools/size_ratchet.json` to resolve every `[verify post-sprint code-map.md]` / `[resolve post-sprint ...]` "
                    "anchor onto the exact post-sprint split submodules. Return staged disjoint implementer slices "
                    "(stage=1 for any shared protocol/registration slice, stage=2+ for independent parallel slices)."
                ),
                schema={
                    "type": "object",
                    "required": ["verified_paths", "slices"],
                    "properties": {
                        "verified_paths": {"type": "boolean"},
                        "slices": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["name", "files", "instructions"],
                                "properties": {
                                    "name": {"type": "string"},
                                    "stage": {"type": "integer"},
                                    "files": {"type": "array", "items": {"type": "string"}},
                                    "instructions": {"type": "string"},
                                },
                            },
                        },
                    },
                },
                role=f"Path Verifier {item_id}-{slug}",
                type_name="research",
                workspace="inherit",
            )
            if not plan.get("verified_paths") or not plan.get("slices"):
                raise RoadmapError(f"Phase 2 path verification failed for {item_id}")

            # Phase 3: Staged Disjoint Implementer Slices committing to dash-delimited sub-branches
            async with state_lock:
                item["phase"] = "phase_3_implement"
                save_state(state_path, state)
            slice_branches: list[str] = []
            temp_stage_branches: list[str] = []
            stages = group_slices_by_stage(plan["slices"])
            base_ref = INTEGRATION_BRANCH
            for stage_idx, stage_slices in enumerate(stages, 1):
                async def _run_slice(slc: dict[str, Any], _base: str = base_ref, _sidx: int = stage_idx) -> dict[str, Any]:
                    sub_branch = slice_branch_name(branch, _sidx, slc["name"])
                    try:
                        res = await rt.agent(
                            prompt=(
                                f"Create branch `{sub_branch}` from `{_base}` in this shared workspace and implement "
                                f"slice `{slc['name']}` for item {item_id} ({slug}) per `{design_doc}`. "
                                f"You own ONLY these files: {slc['files']}. Follow `.agents/skills/rubylight-verify/SKILL.md` "
                                "and `.agents/skills/nightjar-parity/SKILL.md`. Run inner-loop "
                                f"`tools\\dev.ps1 check -Fast -Base {_base}`, commit cleanly to `{sub_branch}`, "
                                f"and return the commit SHA. Instructions: {slc['instructions']}"
                            ),
                            schema={
                                "type": "object",
                                "required": ["commit_sha", "summary"],
                                "properties": {"commit_sha": {"type": "string"}, "summary": {"type": "string"}},
                            },
                            role=f"Impl {item_id}:{slc['name']}",
                            type_name="self",
                            workspace="share",
                        )
                        return {"slice": slc["name"], "branch": sub_branch, "ok": True, "commit_sha": res["commit_sha"]}
                    except Exception as exc:
                        return {"slice": slc["name"], "branch": sub_branch, "ok": False, "error": str(exc)}

                stage_res = await rt.parallel(stage_slices, _run_slice)
                failed_slices = [r for r in stage_res if not r["ok"]]
                if failed_slices:
                    raise RoadmapError(f"Slice failures for {item_id} stage {stage_idx}: {failed_slices}")
                stage_branches = [r["branch"] for r in stage_res]
                slice_branches.extend(stage_branches)
                if stage_idx < len(stages):
                    if len(stage_branches) == 1:
                        base_ref = stage_branches[0]
                    else:
                        stage_ref = stage_merge_branch_name(branch, stage_idx)
                        temp_stage_branches.append(stage_ref)
                        stage_merge = await rt.agent(
                            prompt=(
                                f"In this shared workspace, create/reset `{stage_ref}` from `{base_ref}` and cleanly "
                                f"merge all Stage {stage_idx} slice branches {stage_branches} onto `{stage_ref}` so "
                                f"Stage {stage_idx + 1} slices branch from their combined result. Return `commit_sha`."
                            ),
                            schema={"type": "object", "required": ["commit_sha"], "properties": {"commit_sha": {"type": "string"}}},
                            role=f"Stage {stage_idx} Merge {item_id}-{slug}",
                            type_name="self",
                            workspace="share",
                        )
                        if not stage_merge.get("commit_sha"):
                            raise RoadmapError(f"Stage {stage_idx} merge failed for {item_id}")
                        base_ref = stage_ref

            # Phase 4: Merge final-stage slice branches (plus base_ref) onto `{branch}` and run full verification
            async with state_lock:
                item["phase"] = "phase_4_verify"
                save_state(state_path, state)
            final_merge_refs = list(dict.fromkeys(([base_ref] if base_ref != INTEGRATION_BRANCH else []) + slice_branches))
            verify_out = await rt.agent(
                prompt=(
                    f"In this shared workspace, create/reset `{branch}` from `{INTEGRATION_BRANCH}`, merge ALL "
                    f"Phase 3 slice/stage branches {final_merge_refs} onto `{branch}`, and run full verification per "
                    f"`.agents/skills/rubylight-verify/SKILL.md`: `tools\\dev.ps1 check -Base {INTEGRATION_BRANCH}`, "
                    "`python tools/arch_check.py`, `python tools/code_map.py check`, `python tools/generated_docs.py --check`, "
                    "`python client/scripts/gen_design_tokens.py --check`, `git diff --check`. Fix any failures, "
                    f"commit to `{branch}`, and return the resulting `commit_sha`."
                ),
                schema={
                    "type": "object",
                    "required": ["passed", "commit_sha", "summary"],
                    "properties": {"passed": {"type": "boolean"}, "commit_sha": {"type": "string"}, "summary": {"type": "string"}},
                },
                role=f"Verify {item_id}-{slug}",
                type_name="self",
                workspace="share",
            )
            if not verify_out.get("passed"):
                raise RoadmapError(f"Phase 4 verification failed for {item_id}: {verify_out.get('summary')}")

            # Phase 5: 15-locale / docs / notes.d fan-out (`flash`) on dash-delimited sub-branches
            async with state_lock:
                item["phase"] = "phase_5_l10n_docs_notes"
                save_state(state_path, state)
            fanout_tasks = [
                (
                    "locales",
                    "Update all 15 Nightjar Android locales (`client/app/src/main/res/values*/strings.xml`), "
                    "add new Desktop translations ONLY in `client/protocol/desktop/src/strings_70/*.rs` (NEVER grow "
                    "grandfathered `strings/*.rs` past 1,750 LOC!), and update all 22 host locales (`rust/assets/locale/*.json`) "
                    f"for item {item_id} ({slug}).",
                ),
                (
                    "docs",
                    f"Update `docs/`, `client/docs/`, `docs/code-map.md`, `client/docs/code-map.md` and run "
                    f"`python tools/generated_docs.py` for item {item_id} ({slug}).",
                ),
                (
                    "notes",
                    f"Write release note fragments `rust/release/notes.d/{item_id}-{slug}.md` and/or "
                    f"`client/notes.d/{item_id}-{slug}.md` following their READMEs (no mod names, no Experimental, measured claims only).",
                ),
            ]

            async def _run_fanout(pair: tuple[str, str]) -> dict[str, Any]:
                kind, task_prompt = pair
                sub_branch = phase5_branch_name(branch, kind)
                try:
                    out = await rt.agent(
                        prompt=(
                            f"Create branch `{sub_branch}` from `{branch}` in this shared workspace. {task_prompt} "
                            f"Commit cleanly to `{sub_branch}` and return `commit_sha`."
                        ),
                        schema={"type": "object", "required": ["commit_sha"], "properties": {"commit_sha": {"type": "string"}}},
                        role=f"L10n/Docs {item_id}:{kind}",
                        type_name="self",
                        workspace="share",
                        model="flash",
                    )
                    return {"kind": kind, "branch": sub_branch, "ok": True, "commit_sha": out["commit_sha"]}
                except Exception as exc:
                    return {"kind": kind, "branch": sub_branch, "ok": False, "error": str(exc)}

            fanout_res = await rt.parallel(fanout_tasks, _run_fanout)
            if any(not r["ok"] for r in fanout_res):
                raise RoadmapError(f"Phase 5 fan-out failure for {item_id}: {fanout_res}")
            p5_branches = [r["branch"] for r in fanout_res]

            # Phase 5b: Merge Phase 5 sub-branches onto `{branch}` and re-verify
            p5_merge = await rt.agent(
                prompt=(
                    f"Check out `{branch}` in this shared workspace, merge Phase 5 branches {p5_branches}, "
                    f"and run full verification (`tools\\dev.ps1 check -Base {INTEGRATION_BRANCH}`, "
                    "`python tools/arch_check.py`, `python tools/code_map.py check`, `python tools/generated_docs.py --check`, "
                    "`python client/scripts/gen_design_tokens.py --check`, `git diff --check`). Commit fixes to `{branch}`."
                ),
                schema={"type": "object", "required": ["passed", "commit_sha"], "properties": {"passed": {"type": "boolean"}, "commit_sha": {"type": "string"}}},
                role=f"Phase 5b Verify {item_id}-{slug}",
                type_name="self",
                workspace="share",
            )
            if not p5_merge.get("passed"):
                raise RoadmapError(f"Phase 5b post-l10n verification failed for {item_id}")

            # Phase 6: 5-Lens Adversarial Review — ALL 5 lenses run on `{branch}` in isolated `share` workspaces (strictly read-only)
            async with state_lock:
                item["phase"] = "phase_6_adversarial_review"
                save_state(state_path, state)
            lenses = [
                ("protocol-security", "Audit PROTOCOL_VERSION preservation, HostCapabilities gating, fuzz/proptest coverage, raw config read bans, and no unsigned KMDF."),
                ("arch-ratchet", "Audit arch_check.py compliance (<=800 LOC, //! or /** headers, size_ratchet.json including strings_70), code-map.md accuracy, target_os='linux', and hot-thread blocking."),
                ("reconnect-restore", "Audit state machine transitions, reconnect/resume/roam matrix, crash-restore journaling, loud failures (no silent encoder fallback), and plain-English errors."),
                ("client-parity", "Audit Android+Desktop parity, fewer-settings rule, SettingsKeysTest migrations, 15 Nightjar + 22 host locales, zero mod names, zero Experimental, and dynamic input glyphs."),
                ("visual-ux", f"Follow `.agents/skills/visual-ux-review/SKILL.md`: verify the ground-up 7.0 console-quality design bar (PS5/tvOS/SteamOS OLED/flagship hardware polish), render screenshots on `{branch}` across every affected user-facing surface (Console, /play, /guest, Couch Menu, Setup/Tray, Nightjar Phone/Handheld/TV/Desktop, Websites/READMEs), and inspect them with `read_file` across light/dark/OLED/HC, 360/768/1440 px, German/CJK stress, resolution scaling (#144/#146), blur stability (#61), and zero stream-time blur."),
            ]

            async def _run_lens(pair: tuple[str, str]) -> dict[str, Any]:
                lname, ldesc = pair
                try:
                    res = await rt.agent(
                        prompt=(
                            f"Adversarial review lens `{lname}` for item {item_id} ({slug}). "
                            f"Check out `{branch}` in this shared workspace and audit `git diff {INTEGRATION_BRANCH}..{branch}` "
                            f"strictly read-only — do NOT edit or commit any files. {ldesc}"
                        ),
                        schema={
                            "type": "object",
                            "required": ["approved", "blocking"],
                            "properties": {"approved": {"type": "boolean"}, "blocking": {"type": "array", "items": {"type": "string"}}},
                        },
                        role=f"Review {item_id}:{lname}",
                        type_name="self",
                        workspace="share",
                    )
                    return {"lens": lname, "ok": True, "review": res}
                except Exception as exc:
                    return {"lens": lname, "ok": False, "error": str(exc)}

            lens_res = await rt.parallel(lenses, _run_lens)
            blockers = [
                f"{r['lens']}: {r.get('error') or r['review']['blocking']}"
                for r in lens_res
                if (not r["ok"]) or (not r["review"].get("approved")) or r["review"].get("blocking")
            ]
            if blockers:
                fix_out = await rt.agent(
                    prompt=(
                        f"Check out `{branch}` in this shared workspace, fix ALL Phase 6 blocking findings for item "
                        f"{item_id} ({slug}), re-run full verification (`tools\\dev.ps1 check -Base {INTEGRATION_BRANCH}`, "
                        f"`arch_check.py`, `code_map.py check`, `generated_docs.py --check`), and commit to `{branch}`: {blockers}"
                    ),
                    schema={"type": "object", "required": ["fixed"], "properties": {"fixed": {"type": "boolean"}}},
                    role=f"Fix {item_id}-{slug}",
                    type_name="self",
                    workspace="share",
                )
                if not fix_out.get("fixed"):
                    raise RoadmapError(f"Unresolved Phase 6 findings for {item_id}: {blockers}")

            # Phase 7: Push `wip/roadmap/...` & watch CI + idle-gated serialized HW check + merge onto integration branch
            async with state_lock:
                item["phase"] = "phase_7_ci_hw_merge"
                save_state(state_path, state)
            if item["hw_required"]:
                async with hw_lock:
                    check_gpu_hang_halt(rt.repo_root)
                    hw_out = await rt.agent(
                        prompt=(
                            f"Check out `{branch}` in this shared workspace and run the serialized idle-gated HW check "
                            f"for item {item_id} ({slug}) per `{design_doc}` and `.agents/skills/rubylight-verify/SKILL.md`. "
                            "Verify `/serverinfo` has 0 active and 0 pending sessions before starting. Stop immediately on exit code 3."
                        ),
                        schema={"type": "object", "required": ["passed", "gpu_hang"], "properties": {"passed": {"type": "boolean"}, "gpu_hang": {"type": "boolean"}}},
                        role=f"HW Check {item_id}-{slug}",
                        type_name="self",
                        workspace="share",
                    )
                    check_gpu_hang_halt(rt.repo_root)
                    if hw_out.get("gpu_hang"):
                        raise GpuHangHaltError(f"AMD encoder hang during HW check of {item_id}")
                    if not hw_out.get("passed"):
                        raise RoadmapError(f"HW check failed for {item_id}")

            async with merge_lock:
                merge_out = await rt.agent(
                    prompt=(
                        f"1) Push `{branch}` to `origin` (`git push -u origin {branch}` — triggers `wip/**` CI workflows). "
                        f"2) Watch CI until green (`pwsh tools/dev.ps1 ci {branch}`). "
                        f"3) Check out `{INTEGRATION_BRANCH}`, cleanly merge `{branch}` with zero conflicts, verify `git diff --check`, "
                        f"and prune temporary slice branches ({slice_branches + temp_stage_branches + p5_branches}). Return the merged `commit_sha`."
                    ),
                    schema={"type": "object", "required": ["merged", "commit_sha"], "properties": {"merged": {"type": "boolean"}, "commit_sha": {"type": "string"}}},
                    role=f"CI & Merge {item_id}-{slug}",
                    type_name="self",
                    workspace="inherit",
                )
                if not merge_out.get("merged"):
                    raise RoadmapError(f"CI or merge failed for {item_id}")

            async with state_lock:
                item["status"] = "merged"
                item["phase"] = "phase_7_merged"
                item["commit_sha"] = merge_out.get("commit_sha")
                item["hw_verified"] = bool(item["hw_required"])
                save_state(state_path, state)
            return {"id": item_id, "status": "merged", "commit_sha": item["commit_sha"]}

        except GpuHangHaltError as exc:
            async with state_lock:
                quarantine_item(state, item_id, str(exc))
                save_state(state_path, state)
            return {"id": item_id, "status": "quarantined", "gpu_hang": True, "error": str(exc)}
        except Exception as exc:
            async with state_lock:
                quarantine_item(state, item_id, str(exc))
                save_state(state_path, state)
            rt.log(f"QUARANTINED item {item_id} ({slug}): {exc}")
            return {"id": item_id, "status": "quarantined", "gpu_hang": False, "error": str(exc)}


async def run_carbon_review_cycle(
    rt: WorkflowRuntime,
    state: dict[str, Any],
    state_path: pathlib.Path,
    *,
    wave_key: str,
    review_rel: str,
    git_range: str,
    audit_prompt: str,
    dry_run: bool = False,
) -> dict[str, Any]:
    """Run strictly read-only Carbon review with async task await and up to 2 fix/re-review cycles."""
    review_path = rt.repo_root / review_rel
    review_path.parent.mkdir(parents=True, exist_ok=True)
    wave_entry = state["waves"][wave_key]

    if dry_run:
        fake = {
            "approved": True,
            "wave": wave_key,
            "summary": f"[dry-run] Carbon review approved for {wave_key}.",
            "blocking_findings": [],
            "non_blocking": [],
        }
        review_path.write_text(format_carbon_review_markdown(fake, attempt=1, git_range=git_range), encoding="utf-8", newline="\n")
        wave_entry["carbon_review"].update(
            {"approved": True, "attempts": 1, "blocking_count": 0, "summary": fake["summary"]}
        )
        save_state(state_path, state)
        return fake

    max_attempts = MAX_CARBON_FIX_CYCLES + 1
    last_review: dict[str, Any] | None = None
    for attempt in range(1, max_attempts + 1):
        check_gpu_hang_halt(rt.repo_root)
        head_before, status_before = await _git_head_and_status(rt)

        # Keep prompt and raw JSON outside repo_root so working tree stays untouched.
        with tempfile.TemporaryDirectory(prefix=f"rl-carbon-{wave_key}-") as tmpdir:
            prompt_file = pathlib.Path(tmpdir) / "prompt.txt"
            output_file = pathlib.Path(tmpdir) / "carbon.json"
            prompt_file.write_text(audit_prompt + "\n", encoding="utf-8", newline="\n")
            cmd = build_carbon_review_command(rt.repo_root, prompt_file, output_file)
            await run_command_and_await(rt, cmd, summary=f"Carbon read-only review {wave_key} attempt {attempt}")
            if not output_file.is_file():
                raise RoadmapError(f"Carbon review {wave_key} attempt {attempt} produced no JSON output file")
            raw_out = output_file.read_text(encoding="utf-8")

        head_after, status_after = await _git_head_and_status(rt)
        assert_git_snapshot_unchanged(
            head_before,
            status_before,
            head_after,
            status_after,
            context=f"Carbon {wave_key} attempt {attempt}",
        )

        review = parse_carbon_review(raw_out)
        last_review = review
        review_path.write_text(
            format_carbon_review_markdown(review, attempt=attempt, git_range=git_range),
            encoding="utf-8",
            newline="\n",
        )
        wave_entry["carbon_review"].update(
            {
                "approved": bool(review["approved"]),
                "attempts": attempt,
                "blocking_count": len(review["blocking_findings"]),
                "summary": review["summary"],
            }
        )
        save_state(state_path, state)

        if review["approved"]:
            return review
        if attempt < max_attempts:
            rt.log(f"Carbon {wave_key} attempt {attempt} found {len(review['blocking_findings'])} blocking issue(s); fixing...")
            await rt.agent(
                prompt=(
                    f"Fix EVERY blocking finding from Carbon's `{wave_key}` review in `{review_rel}` on "
                    f"`{INTEGRATION_BRANCH}`, then re-run full verification (`tools\\dev.ps1 check`, "
                    "`python tools/arch_check.py`, `python tools/code_map.py check`, `git diff --check`). "
                    f"Findings: {json.dumps(review['blocking_findings'])}"
                ),
                schema={"type": "string"},
                role=f"Carbon Fix {wave_key} #{attempt}",
                type_name="self",
                workspace="inherit",
            )

    raise RoadmapError(
        f"Carbon review for `{wave_key}` failed after {max_attempts} attempt(s): "
        f"{len((last_review or {}).get('blocking_findings', []))} blocking finding(s) remain."
    )


async def run_wave_6_release(
    rt: WorkflowRuntime,
    state: dict[str, Any],
    state_path: pathlib.Path,
    hw_lock: asyncio.Lock,
    *,
    dry_run: bool = False,
    authorize_publish: bool = False,
) -> dict[str, Any]:
    """Fail-closed Wave 6: Release Rubylight 7.0.0 and Nightjar 7.0.0 (requires explicit maintainer OK)."""
    rt.phase("Wave 6: Release 7.0.0 Preflight")
    rl_tags: list[str] = []
    nj_tags: list[str] = []
    prev_tag = "v6.3.0"
    if not dry_run:
        rl_out = await rt.call_tool(
            "run_command",
            CommandLine='git ls-remote --tags https://github.com/RamazanKara/Rubylight.git "refs/tags/v7.0.0"',
            WaitMsBeforeAsync=8000,
            toolSummary="Check Rubylight v7.0.0 tag absence",
        )
        nj_out = await rt.call_tool(
            "run_command",
            CommandLine='git ls-remote --tags https://github.com/RamazanKara/nightjar.git "refs/tags/v7.0.0"',
            WaitMsBeforeAsync=8000,
            toolSummary="Check Nightjar v7.0.0 tag absence",
        )
        rl_tags = [line.split()[-1] for line in rl_out.splitlines() if line.strip() and "refs/tags/" in line]
        nj_tags = [line.split()[-1] for line in nj_out.splitlines() if line.strip() and "refs/tags/" in line]
        desc_out = await rt.call_tool(
            "run_command",
            CommandLine=f'git -C "{rt.repo_root}" describe --tags --abbrev=0 --match "v[0-9]*.[0-9]*.[0-9]*" origin/main',
            WaitMsBeforeAsync=5000,
            toolSummary="Find previous release tag",
        )
        if desc_out.strip().startswith("v"):
            prev_tag = desc_out.strip().splitlines()[-1].strip()

    gate_errors = evaluate_release_gate(
        state,
        rt.repo_root,
        rubylight_remote_tags=rl_tags,
        nightjar_remote_tags=nj_tags,
        require_whole_release_carbon=False,
        check_release_artifacts=False,
    )
    if gate_errors:
        raise ReleaseGateError("Wave 6 Release 7.0.0 blocked:\n- " + "\n- ".join(gate_errors))

    if not dry_run:
        rt.phase("Wave 6: Version Bump & Release Notes")
        await rt.agent(
            prompt=(
                f"On integration branch `{INTEGRATION_BRANCH}`, prepare 7.0.0: "
                "1) Run `python rust/release/bump.py 7.0.0` (and `python rust/release/service_inputs.py --update` if service inputs changed). "
                "2) Bump `client/app/build.gradle` to `versionName \"7.0.0\"` and increment `versionCode` by 1 (strictly greater than the previous release's `versionCode`, at least 406). "
                "3) Assemble Nightjar notes via `python tools/nightjar_notes.py --out docs/reviews/nightjar-7.0.0-whats-new.md --changelog-en <file>` "
                "(Play changelog strictly <= 500 chars) and fill in the real `**Tested.**` paragraph. "
                "4) Run full checks (`tools\\dev.ps1 check`, `arch_check.py`, `code_map.py check`, `generated_docs.py --check`, "
                "`gen_design_tokens.py --check`, `site/build.py`, `client/site/check-links.mjs`)."
            ),
            schema={"type": "string"},
            role="Release 7.0.0 Prep",
            type_name="self",
            workspace="inherit",
        )

    rt.phase("Wave 6: Carbon Whole-Release Review")
    head_sha = "HEAD"
    if not dry_run:
        head_sha, _ = await _git_head_and_status(rt)
    state["waves"]["6"]["base_sha"] = prev_tag
    state["waves"]["6"]["head_sha"] = head_sha
    await run_carbon_review_cycle(
        rt,
        state,
        state_path,
        wave_key="6",
        review_rel="docs/reviews/release-7.0.0-carbon.md",
        git_range=f"{prev_tag}..{head_sha}",
        audit_prompt=(
            f"Perform a strictly read-only WHOLE-RELEASE review across `{prev_tag}..{head_sha}` for "
            "Rubylight 7.0.0 and Nightjar 7.0.0, including `rust/RELEASE_NOTES.md`, "
            "`docs/reviews/nightjar-7.0.0-whats-new.md`, the Google Play <=500 char changelog, all protocol/capability "
            "contracts, `AGENTS.md`, `client/AGENTS.md`, and `tools/arch_check.py`. Return strict JSON matching "
            "`.agents/schemas/wave_review.json`. Edit NO files."
        ),
        dry_run=dry_run,
    )
    state["release"]["whole_release_carbon_approved"] = True
    save_state(state_path, state)

    final_errors = evaluate_release_gate(
        state,
        rt.repo_root,
        rubylight_remote_tags=rl_tags,
        nightjar_remote_tags=nj_tags,
        require_whole_release_carbon=True,
        check_release_artifacts=not dry_run,
        authorize_publish=authorize_publish,
    )
    if final_errors:
        raise ReleaseGateError("Wave 6 Release authorization blocked:\n- " + "\n- ".join(final_errors))

    if dry_run:
        state["waves"]["6"]["status"] = "completed"
        save_state(state_path, state)
        return {"released": False, "dry_run": True}

    auth_path = rt.repo_root / RELEASE_AUTH_SENTINEL
    rt.phase("Wave 6: Maintainer-Authorized Release Execution")
    try:
        auth_path.parent.mkdir(parents=True, exist_ok=True)
        auth_path.write_text(
            f"authorized_utc={utc_now()}\nversion={RELEASE_TARGET}\nwhole_release_carbon=approved\nmaintainer_ok=true\n",
            encoding="utf-8",
            newline="\n",
        )
        state["release"]["maintainer_authorized"] = True
        save_state(state_path, state)

        await rt.agent(
            prompt=(
                "`.agents/RELEASE_7_0_AUTHORIZED` is active with explicit maintainer approval. Execute steps 1-3 safely "
                "(never force-push): "
                f"1) Clean-merge `{INTEGRATION_BRANCH}` into `main` and push `main`. "
                "2) Wait for 100% green `main` CI (`rust-windows.yml`, `nightjar-android.yml`, `nightjar-macos.yml`). "
                "3) Push `v7.0.0` by commit SHA on Rubylight (`git push origin <sha>:refs/tags/v7.0.0`) and wait for release workflow."
            ),
            schema={"type": "object", "required": ["rubylight_sha"], "properties": {"rubylight_sha": {"type": "string"}}},
            role="Publish Rubylight 7.0.0",
            type_name="self",
            workspace="inherit",
        )
        state["release"]["rubylight_tag_pushed"] = True
        save_state(state_path, state)

        async with hw_lock:
            check_gpu_hang_halt(rt.repo_root)
            await rt.agent(
                prompt=(
                    "Verify installed host is idle (`/serverinfo` session counts 0), then run "
                    "and `pwsh rust/release/check.ps1 v7.0.0` (after verifying installed-host idle "
            "`RustHostSessionCount == 0 && RustHostPendingSessionCount == 0` and zero `.agents/GPU_HANG_HALT`). "
                    "Abort immediately on exit code 3."
                ),
                schema={"type": "string"},
                role="Rubylight 7.0.0 Host Check",
                type_name="self",
                workspace="inherit",
            )
            check_gpu_hang_halt(rt.repo_root)
            state["release"]["rubylight_hw_check_passed"] = True
            save_state(state_path, state)

        await rt.agent(
            prompt=(
                "Complete Nightjar 7.0.0 & Websites publish (never force-push, preserve existing gh-pages history): "
                "1) Wait for `nightjar-mirror.yml` to mirror `client/` to `RamazanKara/nightjar`. "
                "2) Trigger `gh workflow run release.yml -R RamazanKara/nightjar -f dry_run=true`, download `windows-release` "
                "artifact, and run `client/scripts/defender-scan.ps1` (exit 2 blocks tag!). "
                "3) Push `v7.0.0` by mirrored commit SHA on `RamazanKara/nightjar`, wait for release workflow, and update "
                "release notes via `gh release edit v7.0.0 -R RamazanKara/nightjar --notes-file docs/reviews/nightjar-7.0.0-whats-new.md`. "
                "4) Publish both Rubylight (`site/build.py` `_site/`) and Nightjar (`client/site/`) `gh-pages` branches "
                "by cloning existing `gh-pages` history into temp dirs, updating files, committing, and pushing normally."
            ),
            schema={"type": "string"},
            role="Publish Nightjar 7.0.0 & Sites",
            type_name="self",
            workspace="inherit",
        )
        state["release"].update(
            {
                "nightjar_mirrored": True,
                "nightjar_dry_run_passed": True,
                "nightjar_defender_clean": True,
                "nightjar_tag_pushed": True,
                "sites_published": True,
            }
        )
        state["waves"]["6"]["status"] = "completed"
        save_state(state_path, state)
        return {"released": True, "version": RELEASE_TARGET}
    finally:
        if auth_path.exists():
            auth_path.unlink()


async def run_roadmap(
    rt: WorkflowRuntime,
    *,
    waves: list[int] | None = None,
    max_parallel: int = 2,
    dry_run: bool = False,
    authorize_publish: bool = False,
) -> dict[str, Any]:
    """Top-level resumable orchestrator across requested waves."""
    selected_waves = sorted(set(waves if waves is not None else [0, 1, 2, 3, 4, 5, 6]))
    effective_parallel = max(1, min(int(max_parallel), MAX_CONCURRENCY_CAP))

    if not dry_run:
        prereq_errors = verify_post_sprint_prereqs(rt.repo_root)
        if prereq_errors:
            raise RoadmapError("Post-sprint prerequisites missing:\n- " + "\n- ".join(prereq_errors))

    state_path = rt.repo_root / STATE_REL
    state = load_state(state_path)
    propagate_dependency_quarantines(state)
    save_state(state_path, state)

    sem = asyncio.Semaphore(effective_parallel)
    hw_lock = asyncio.Lock()
    merge_lock = asyncio.Lock()
    state_lock = asyncio.Lock()

    rt.log(
        f"Roadmap 7.0.0 orchestrator starting: waves={selected_waves}, "
        f"max_parallel={effective_parallel}, dry_run={dry_run}, authorize_publish={authorize_publish}"
    )

    for wave in selected_waves:
        check_gpu_hang_halt(rt.repo_root)
        blockers = check_wave_entry_gate(state, wave, rt.repo_root)
        if blockers:
            raise RoadmapError(f"Cannot enter Wave {wave}: {'; '.join(blockers)}")

        if wave == 6:
            await run_wave_6_release(
                rt,
                state,
                state_path,
                hw_lock,
                dry_run=dry_run,
                authorize_publish=authorize_publish,
            )
            continue

        rt.phase(f"Wave {wave}")
        wstate = state["waves"][str(wave)]
        wstate["status"] = "in_progress"
        if not wstate.get("base_sha"):
            if dry_run:
                wstate["base_sha"] = f"dryrun-wave-{wave}-base"
            else:
                base_sha, _ = await _git_head_and_status(rt)
                wstate["base_sha"] = base_sha
        save_state(state_path, state)

        while True:
            propagate_dependency_quarantines(state)
            batch = next_ready_batch(state, wave)
            if not batch:
                break
            results = await rt.parallel(
                batch,
                lambda it: run_item_lifecycle(
                    rt,
                    state,
                    state_path,
                    it,
                    sem,
                    hw_lock,
                    merge_lock,
                    state_lock,
                    dry_run=dry_run,
                ),
            )
            if any(r.get("gpu_hang") for r in results):
                raise GpuHangHaltError(f"Wave {wave} halted by AMD encoder hang (exit code 3)")

        # Fail-closed: do NOT advance to Phase 8 or mark Wave completed if any item in this wave failed/quarantined.
        unmerged = wave_unmerged_items(state, wave)
        if unmerged:
            wstate["status"] = "blocked"
            save_state(state_path, state)
            raise RoadmapError(
                f"Wave {wave} halted before Phase 8 Carbon review: unmerged/quarantined items: "
                + ", ".join(f"{i['id']}({i['status']}: {i.get('quarantine_reason')})" for i in unmerged)
            )

        if dry_run:
            wstate["head_sha"] = f"dryrun-wave-{wave}-head"
        else:
            head_sha, _ = await _git_head_and_status(rt)
            wstate["head_sha"] = head_sha
        save_state(state_path, state)

        git_range = f"{wstate['base_sha']}..{wstate['head_sha']}"
        rt.phase(f"Wave {wave}: Phase 8 Carbon Review")
        await run_carbon_review_cycle(
            rt,
            state,
            state_path,
            wave_key=str(wave),
            review_rel=f"docs/reviews/wave-{wave}-carbon.md",
            git_range=git_range,
            audit_prompt=(
                f"Perform a strictly read-only post-wave audit of Wave {wave} (`{git_range}` on "
                f"`{INTEGRATION_BRANCH}`) against `AGENTS.md`, `client/AGENTS.md`, `tools/arch_check.py`, "
                "`tools/size_ratchet.json`, and the Wave design specs under `docs/designs/`. Return strict JSON "
                "matching `.agents/schemas/wave_review.json`. Edit NO files."
            ),
            dry_run=dry_run,
        )
        wstate["status"] = "completed"
        save_state(state_path, state)

    return {
        "release_target": RELEASE_TARGET,
        "waves_requested": selected_waves,
        "merged_items": [i["id"] for i in state["items"] if i["status"] == "merged"],
        "quarantined_items": [i["id"] for i in state["items"] if i["status"] == "quarantined"],
    }
