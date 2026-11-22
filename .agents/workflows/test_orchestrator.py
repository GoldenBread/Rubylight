"""Unit tests for orchestrator_core.py and roadmap_orchestrator.py (no LLM calls)."""
from __future__ import annotations

import ast
import asyncio
import json
import pathlib
import sys
import tempfile
import unittest

WORKFLOWS_DIR = pathlib.Path(__file__).resolve().parent
REPO_ROOT = WORKFLOWS_DIR.parents[1]
if str(WORKFLOWS_DIR) not in sys.path:
    sys.path.insert(0, str(WORKFLOWS_DIR))

import orchestrator_core as core  # noqa: E402


async def _fake_parallel(items, fn):
    return list(await asyncio.gather(*(fn(x) for x in items)))


async def _fake_pipeline(items, fn):
    out = []
    for x in items:
        out.append(await fn(x))
    return out


class OrchestratorCoreTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)
        (self.root / ".agents" / "workflows").mkdir(parents=True)
        (self.root / ".agents" / "schemas").mkdir(parents=True)
        (self.root / "docs" / "reviews").mkdir(parents=True)
        (self.root / "tools").mkdir(parents=True)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def _seed_post_sprint_prereqs(self) -> None:
        (self.root / "tools" / "arch_check.py").write_text("# ok\n", encoding="utf-8", newline="\n")
        (self.root / "tools" / "size_ratchet.json").write_text('{"max_loc": 800, "files": {}}\n', encoding="utf-8", newline="\n")
        (self.root / "tools" / "dev.ps1").write_text("param([switch] $Fast)\n", encoding="utf-8", newline="\n")

    def _seed_valid_release_artifacts(self, version_code: int = 406) -> None:
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "7.0.0"\n', encoding="utf-8", newline="\n")
        gradle = self.root / "client" / "app" / "build.gradle"
        gradle.parent.mkdir(parents=True, exist_ok=True)
        gradle.write_text(f'    versionName "7.0.0"\n    versionCode = {version_code}\n', encoding="utf-8", newline="\n")
        play = self.root / "client" / "fastlane" / "metadata" / "android" / "en-US" / "changelogs" / f"{version_code}.txt"
        play.parent.mkdir(parents=True, exist_ok=True)
        play.write_text("• Rubylight & Nightjar 7.0.0 release.\n", encoding="utf-8", newline="\n")
        (self.root / "docs" / "reviews" / "nightjar-7.0.0-whats-new.md").write_text(
            "## What's new\n\n- Obsidian Cinema & Obsidian Studio 7.0.\n\n**Tested.** Verified on RX 7900 XT, Radeon 780M, and Pixel emulator.\n",
            encoding="utf-8",
            newline="\n",
        )

    def test_roadmap_orchestrator_top_level_await_syntax_valid(self) -> None:
        script_path = WORKFLOWS_DIR / "roadmap_orchestrator.py"
        src = script_path.read_text(encoding="utf-8")
        code = compile(src, str(script_path), "exec", flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT)
        self.assertIsNotNone(code)
        self.assertNotIn(b"\r\n", script_path.read_bytes())

    def test_post_sprint_prereqs_fail_closed(self) -> None:
        errs = core.verify_post_sprint_prereqs(self.root)
        self.assertEqual(len(errs), 3)
        self._seed_post_sprint_prereqs()
        self.assertEqual(core.verify_post_sprint_prereqs(self.root), [])

    def test_branch_names_trigger_wip_ci(self) -> None:
        st = core.initial_state()
        for item in st["items"]:
            self.assertTrue(item["branch"].startswith("wip/roadmap/"), msg=item["branch"])

    def test_carbon_cli_command_places_model_before_print(self) -> None:
        cmd = core.build_carbon_review_command(
            self.root,
            pathlib.Path(r"C:\temp\prompt.txt"),
            pathlib.Path(r"C:\temp\out.json"),
        )
        self.assertLess(cmd.index("--model"), cmd.index("--print"))
        self.assertNotIn("--add-dir \"C:\\src\"", cmd)
        self.assertIn(f'--add-dir "{self.root}"', cmd)

    def test_state_load_save_and_resume(self) -> None:
        state_path = self.root / core.STATE_REL
        st = core.load_state(state_path)
        self.assertEqual(len(st["items"]), 29)
        self.assertEqual(core.item_by_id(st, "21")["wave"], 1)
        self.assertEqual(core.item_by_id(st, "21")["depends_on"], ["04", "06"])
        self.assertEqual(core.item_by_id(st, "22")["wave"], 1)
        self.assertEqual(core.item_by_id(st, "23")["wave"], 2)
        self.assertEqual(core.item_by_id(st, "24")["wave"], 2)
        self.assertEqual(core.item_by_id(st, "25")["depends_on"], ["22"])
        self.assertEqual(core.item_by_id(st, "26")["depends_on"], ["13", "24"])
        self.assertEqual(core.item_by_id(st, "27")["wave"], 1)
        self.assertEqual(core.item_by_id(st, "28")["depends_on"], ["03"])
        self.assertEqual(core.item_by_id(st, "29")["depends_on"], ["04", "06", "27"])
        for dep in ("21", "22", "27", "28", "29"):
            self.assertIn(dep, core.item_by_id(st, "07")["depends_on"])
        for wf_file in ("orchestrator_core.py", "roadmap_orchestrator.py"):
            self.assertNotIn("flash_lite", (WORKFLOWS_DIR / wf_file).read_text(encoding="utf-8"))
        core.item_by_id(st, "01")["status"] = "merged"
        core.save_state(state_path, st)
        self.assertNotIn(b"\r\n", state_path.read_bytes())
        reloaded = core.load_state(state_path)
        self.assertEqual(core.item_by_id(reloaded, "01")["status"], "merged")
        self.assertEqual(core.item_by_id(reloaded, "02")["status"], "pending")

    def test_dependency_quarantine_and_staged_slices(self) -> None:
        st = core.initial_state()
        changed = core.quarantine_item(st, "09", "test failure")
        for expected in ("09", "13", "14", "15", "16", "17", "18", "19", "20"):
            self.assertIn(expected, changed)
        stages = core.group_slices_by_stage([
            {"name": "B", "stage": 2},
            {"name": "A", "stage": 1},
            {"name": "C", "stage": 2},
        ])
        self.assertEqual([[s["name"] for s in stg] for stg in stages], [["A"], ["B", "C"]])

    def test_wave_entry_gate_blocks_unmerged_or_unapproved_wave(self) -> None:
        st = core.initial_state()
        reasons = core.check_wave_entry_gate(st, 1, self.root)
        self.assertTrue(any("Wave 0 has unmerged items" in r for r in reasons))
        self.assertTrue(any("Wave 0 Carbon review" in r for r in reasons))

        core.item_by_id(st, "01")["status"] = "merged"
        core.item_by_id(st, "02")["status"] = "merged"
        st["waves"]["0"]["carbon_review"]["approved"] = True
        self.assertEqual(core.check_wave_entry_gate(st, 1, self.root), [])

    def test_release_gate_artifacts_and_maintainer_approval(self) -> None:
        st = core.initial_state()
        for item in st["items"]:
            item["status"] = "merged"
        for w in (*core.REQUIRED_CARBON_WAVES, 6):
            rev = st["waves"][str(w)]["carbon_review"]
            rev["approved"] = True
            rev["blocking_count"] = 0
            (self.root / rev["review_file"]).write_text("# ok\n", encoding="utf-8", newline="\n")

        # Missing release artifacts & maintainer confirmation both fail closed.
        errs = core.evaluate_release_gate(
            st,
            self.root,
            require_whole_release_carbon=True,
            check_release_artifacts=True,
            authorize_publish=False,
        )
        self.assertTrue(any("Cargo.toml" in e for e in errs))
        self.assertTrue(any("Maintainer confirmation required" in e for e in errs))

        self._seed_valid_release_artifacts(version_code=406)
        self.assertEqual(
            core.evaluate_release_gate(
                st,
                self.root,
                require_whole_release_carbon=True,
                check_release_artifacts=True,
                authorize_publish=True,
            ),
            [],
        )

        # Unbumped versionCode (405, already used by 6.3.0) fails closed.
        self._seed_valid_release_artifacts(version_code=405)
        errs_vc = core.verify_release_artifacts(self.root)
        self.assertTrue(any("versionCode (405)" in e for e in errs_vc))

        # Play changelog > 500 chars fails closed.
        self._seed_valid_release_artifacts(version_code=406)
        play = self.root / "client/fastlane/metadata/android/en-US/changelogs/406.txt"
        play.write_text("x" * 501 + "\n", encoding="utf-8", newline="\n")
        errs_len = core.verify_release_artifacts(self.root)
        self.assertTrue(any("501 chars" in e for e in errs_len))

        # Unfilled **Tested.** placeholder fails closed.
        play.write_text("• Ok.\n", encoding="utf-8", newline="\n")
        (self.root / "docs/reviews/nightjar-7.0.0-whats-new.md").write_text("## What's new\n\n- Hi\n\n**Tested.** \n", encoding="utf-8", newline="\n")
        errs_tested = core.verify_release_artifacts(self.root)
        self.assertTrue(any("unfilled `**Tested.**`" in e for e in errs_tested))

    def test_git_sub_branches_never_df_collide_and_multislice_stage_handoff(self) -> None:
        branch = core.item_branch_name("05", "stream-dvr")
        for ref in (
            core.slice_branch_name(branch, 1, "5A"),
            core.stage_merge_branch_name(branch, 1),
            core.phase5_branch_name(branch, "locales"),
        ):
            self.assertFalse(ref.startswith(branch + "/"), msg=f"Git D/F collision: {ref} vs {branch}")
            self.assertTrue(ref.startswith(branch + "-"), msg=ref)

        # Simulate a full non-dry-run item with 2-slice Stage 1 -> 1-slice Stage 2 and verify:
        # 1) Stage 1 merge branch is created and passed as Stage 2 base;
        # 2) All 5 Phase 6 lenses use type_name="self", workspace="share", and interpolated `{branch}`.
        st = core.initial_state()
        st_path = self.root / "docs" / "stage-test-state.json"
        core.save_state(st_path, st)
        recorded: list[dict] = []

        async def _mock_agent(**kwargs):
            recorded.append(kwargs)
            role = kwargs.get("role", "")
            if role.startswith("Spec Critic"):
                return {"approved": True, "issues": []}
            if role.startswith("Path Verifier"):
                return {
                    "verified_paths": True,
                    "slices": [
                        {"name": "A1", "stage": 1, "files": ["a.rs"], "instructions": "do A1"},
                        {"name": "A2", "stage": 1, "files": ["b.rs"], "instructions": "do A2"},
                        {"name": "B1", "stage": 2, "files": ["c.rs"], "instructions": "do B1"},
                    ],
                }
            if role.startswith(("Impl ", "Stage ", "L10n/Docs ")):
                return {"commit_sha": "deadbeef", "summary": "ok"}
            if role.startswith(("Verify ", "Phase 5b ")):
                return {"passed": True, "commit_sha": "cafebabe", "summary": "ok"}
            if role.startswith("Review "):
                return {"approved": True, "blocking": []}
            if role.startswith("CI & Merge "):
                return {"merged": True, "commit_sha": "12345678"}
            raise AssertionError(f"Unexpected role: {role}")

        async def _mock_tool(name, **kwargs):
            return ""

        rt = core.WorkflowRuntime(
            repo_root=self.root,
            agent=_mock_agent,
            parallel=_fake_parallel,
            pipeline=_fake_pipeline,
            phase=lambda _: None,
            log=lambda *a: None,
            call_tool=_mock_tool,
        )
        res = asyncio.run(
            core.run_item_lifecycle(
                rt,
                st,
                st_path,
                core.item_by_id(st, "01"),
                asyncio.Semaphore(2),
                asyncio.Lock(),
                asyncio.Lock(),
                asyncio.Lock(),
                dry_run=False,
            )
        )
        self.assertEqual(res["status"], "merged")
        stage_merges = [c for c in recorded if c["role"].startswith("Stage 1 Merge")]
        self.assertEqual(len(stage_merges), 1)
        b1_calls = [c for c in recorded if c["role"] == "Impl 01:B1"]
        self.assertEqual(len(b1_calls), 1)
        self.assertIn("wip/roadmap/01-ui-rubylight-studio-stage-1", b1_calls[0]["prompt"])
        review_calls = [c for c in recorded if c["role"].startswith("Review 01:")]
        self.assertEqual(len(review_calls), 5)
        for rc in review_calls:
            self.assertEqual(rc["type_name"], "self", msg=rc["role"])
            self.assertEqual(rc["workspace"], "share", msg=rc["role"])
            self.assertIn("wip/roadmap/01-ui-rubylight-studio", rc["prompt"])
            self.assertNotIn("{branch}", rc["prompt"])

    def test_carbon_parser_and_git_snapshot_assertion(self) -> None:
        wrapped = json.dumps({
            "response": json.dumps({
                "approved": True,
                "wave": "wave-1",
                "summary": "Has blocker.",
                "blocking_findings": [{
                    "item_id": "03",
                    "severity": "high",
                    "file": "rust/host/src/launch_focus.rs",
                    "issue": "Lock held across Win32 call",
                    "required_fix": "Release current_app mutex first",
                }],
                "non_blocking": [],
            })
        })
        self.assertFalse(core.parse_carbon_review(wrapped)["approved"])

        core.assert_git_snapshot_unchanged("abc", "", "abc", "", context="clean")
        with self.assertRaises(core.CarbonReadOnlyViolationError):
            core.assert_git_snapshot_unchanged("abc", "", "def", "", context="head moved")
        with self.assertRaises(core.CarbonReadOnlyViolationError):
            core.assert_git_snapshot_unchanged("abc", "", "abc", "?? evil.txt", context="untracked")

    def test_run_command_and_await_handles_background_task(self) -> None:
        calls: list[tuple[str, dict]] = []

        async def _fake_call_tool(name, **kwargs):
            calls.append((name, kwargs))
            if name == "run_command":
                return "Tool is running as a background task with task id: conv/task-42"
            return "Task conv/task-42 status: completed"

        async def _unused_agent(**kwargs):
            return ""

        rt = core.WorkflowRuntime(
            repo_root=self.root,
            agent=_unused_agent,
            parallel=_fake_parallel,
            pipeline=_fake_pipeline,
            phase=lambda _: None,
            log=lambda *a: None,
            call_tool=_fake_call_tool,
        )
        out = asyncio.run(core.run_command_and_await(rt, "echo hi", summary="test", poll_seconds=0.01))
        self.assertIn("completed", out)
        self.assertEqual([c[0] for c in calls], ["run_command", "manage_task"])

    def test_dry_run_and_wave_halts_on_quarantine(self) -> None:
        async def _unused_agent(**kwargs):
            raise RuntimeError("agent crash")

        async def _unused_tool(name, **kwargs):
            return ""

        rt = core.WorkflowRuntime(
            repo_root=self.root,
            agent=_unused_agent,
            parallel=_fake_parallel,
            pipeline=_fake_pipeline,
            phase=lambda _: None,
            log=lambda *a: None,
            call_tool=_unused_tool,
        )
        # Dry run across Waves 0-6 with authorize_publish=True succeeds.
        summary = asyncio.run(
            core.run_roadmap(rt, waves=[0, 1, 2, 3, 4, 5, 6], max_parallel=2, dry_run=True, authorize_publish=True)
        )
        self.assertEqual(len(summary["merged_items"]), 29)

        # Dry run without authorize_publish blocks at Wave 6 maintainer gate.
        (self.root / core.STATE_REL).unlink()
        with self.assertRaises(core.ReleaseGateError):
            asyncio.run(
                core.run_roadmap(rt, waves=[0, 1, 2, 3, 4, 5, 6], max_parallel=2, dry_run=True, authorize_publish=False)
            )

        # Non-dry-run with quarantined item in Wave 0 halts BEFORE Wave 0 Phase 8 / Wave 1!
        (self.root / core.STATE_REL).unlink()
        self._seed_post_sprint_prereqs()
        with self.assertRaises(core.RoadmapError) as ctx:
            asyncio.run(core.run_roadmap(rt, waves=[0, 1], max_parallel=2, dry_run=False))
        self.assertIn("Wave 0 halted before Phase 8", str(ctx.exception))

    def test_checked_in_roadmap_state_json_matches_catalog(self) -> None:
        checked_in = REPO_ROOT / core.STATE_REL
        self.assertTrue(checked_in.is_file(), f"Missing {checked_in}")
        st = core.load_state(checked_in)
        self.assertEqual([i["id"] for i in st["items"]], [i["id"] for i in core.ITEM_CATALOG])
        for item in st["items"]:
            self.assertTrue(item["branch"].startswith("wip/roadmap/"), msg=item["branch"])
        self.assertNotIn(b"\r\n", checked_in.read_bytes())


if __name__ == "__main__":
    unittest.main()
