"""Unit tests for .agents/hooks/guard_commands.py and watch_gpu_hang.py."""
from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

HOOKS_DIR = pathlib.Path(__file__).resolve().parent
if str(HOOKS_DIR) not in sys.path:
    sys.path.insert(0, str(HOOKS_DIR))

import guard_commands  # noqa: E402
import watch_gpu_hang  # noqa: E402


def _pre_payload(cmd: str, tool_name: str = "run_command", cwd: str | None = None) -> dict:
    p = {
        "conversationId": "test-conv",
        "hookEventName": "PreToolUse",
        "toolCall": {
            "id": "toolu_01",
            "name": tool_name,
            "args": {"CommandLine": cmd} if tool_name == "run_command" else {"Path": cmd},
        },
    }
    if cwd:
        p["cwd"] = cwd
    return p


class GuardCommandsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.agents_dir = pathlib.Path(self.tmp.name) / ".agents"
        self.agents_dir.mkdir(parents=True)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def _eval(self, cmd: str, env: dict[str, str] | None = None, cwd: str | None = None) -> dict[str, str]:
        return guard_commands.evaluate_command(
            cmd,
            agents_dir=self.agents_dir,
            env={} if env is None else env,
            cwd=cwd,
        )

    def test_safe_commands_allowed(self) -> None:
        safe = [
            "git status --short",
            "git push origin wip/roadmap/03-launch-curtain",
            "git push -u origin roadmap/redesign-and-waves-1-5",
            "cargo fmt -- --check",
            "tools\\dev.ps1 check -Fast -Base origin/main",
            "python tools/arch_check.py",
            "python tools/build_lock.py -- pwsh rust/build.ps1",
            "python tools\\build_lock.py --timeout 1800 -- pwsh rust\\build.ps1 -Package",
            "python -m unittest discover -s .agents -p test_*.py",
            "python rust/release/test_gpu_hang.py",
            "gh workflow run release.yml -R RamazanKara/nightjar -f dry_run=true",
        ]
        for cmd in safe:
            with self.subTest(cmd=cmd):
                res = self._eval(cmd)
                self.assertEqual(res["decision"], "allow", msg=res)

    def test_always_hard_denied_including_bypass_vectors(self) -> None:
        (self.agents_dir / guard_commands.RELEASE_SENTINEL).write_text("ok\n", encoding="utf-8", newline="\n")
        env = {}
        denied = [
            "git push --force origin wip/foo",
            "git.exe push -f origin roadmap/foo",
            "git push --force-with-lease origin wip/foo",
            "git push origin +HEAD:refs/heads/wip/foo",
            "git tag -f v7.0.0",
            "git.exe tag --force v7.0.0",
            "git tag -d v7.0.0",
            "git tag --delete v7.0.0",
            "git push origin :refs/tags/v7.0.0",
            "git push origin --delete v7.0.0",
            "cargo fmt --all -- --check",
            "cargo.exe +1.98.1-x86_64-pc-windows-gnu fmt --all",
            r"Get-Content C:\ProgramData\Butterpollo\config\logs\butterpollo.log",
            "cat C:/ProgramData/Butterpollo/config/butterpollo.conf",
            "type client/keystore.properties",
            "git add client/keystore.properties",
            "python tools/arch_check.py --write-baselines",
            r"pwsh rust\build.ps1",
            r"pwsh C:\src\rl-roadmap-prep\rust\build.ps1",
            "pwsh $PWD/rust/build.ps1",
            r"cd rust; .\build.ps1",
            "python tools/build_lock.py --lane cargo -- pwsh rust/build.ps1",
            "echo ok | pwsh rust/build.ps1",
            "$(pwsh rust/build.ps1)",
            "& { pwsh rust/build.ps1 }",
            "New-Item -ItemType File .agents/RELEASE_7_0_AUTHORIZED",
            "Remove-Item .agents/GPU_HANG_HALT",
            "Set-Content tools/size_ratchet.json '{}'",
            "Set-Content tools/arch_baselines.json '{}'",
            "Set-Content .agents/hooks.json '{}'",
        ]
        for cmd in denied:
            with self.subTest(cmd=cmd):
                res = self._eval(cmd, env=env)
                self.assertEqual(res["decision"], "deny", msg=f"expected deny for {cmd}, got {res}")

    def test_file_tools_guarded(self) -> None:
        # Protected writes/edits must be hard-denied.
        blocked_writes = [
            r"C:\src\rl-roadmap-prep\.agents\RELEASE_7_0_AUTHORIZED",
            r"C:\src\rl-roadmap-prep\.agents\GPU_HANG_HALT",
            r"C:\src\rl-roadmap-prep\.agents\hooks.json",
            r"C:\src\rl-roadmap-prep\.agents\hooks\guard_commands.py",
            r"C:\src\rl-roadmap-prep\tools\size_ratchet.json",
            r"C:\src\rl-roadmap-prep\tools\arch_baselines.json",
            r"C:\src\rl-roadmap-prep\client\keystore.properties",
            r"C:\ProgramData\Butterpollo\config\butterpollo.conf",
        ]
        for tool in ("write_file", "edit_file"):
            for path in blocked_writes:
                with self.subTest(tool=tool, path=path):
                    res = guard_commands.handle_payload(_pre_payload(path, tool_name=tool), agents_dir=self.agents_dir)
                    self.assertEqual(res["decision"], "deny", msg=res)

        # read_file allowed on tools/size_ratchet.json and .agents/hooks.json, denied on ProgramData\Butterpollo & keystore.properties.
        self.assertEqual(
            guard_commands.handle_payload(
                _pre_payload(r"C:\src\rl-roadmap-prep\tools\size_ratchet.json", tool_name="read_file"),
                agents_dir=self.agents_dir,
            )["decision"],
            "allow",
        )
        for secret in (r"C:\ProgramData\Butterpollo\config\logs\butterpollo.log", r"C:\src\rl-roadmap-prep\client\keystore.properties"):
            res = guard_commands.handle_payload(_pre_payload(secret, tool_name="read_file"), agents_dir=self.agents_dir)
            self.assertEqual(res["decision"], "deny", msg=res)

    def test_programdata_butterpollo_release_not_false_positive(self) -> None:
        res = self._eval(r"Test-Path C:\ProgramData\ButterpolloRelease\elevated.ps1")
        self.assertEqual(res["decision"], "allow", msg=res)

    def test_release_commands_require_sentinel_and_no_gpu_halt(self) -> None:
        release_cmds = [
            "git push origin main",
            "git push",
            "git push origin HEAD",
            "git push origin HEAD:refs/heads/main",
            "git push origin abc123:refs/tags/v7.0.0",
            "git push origin --tags",
            "git push origin v7.0.0",
            "gh release create v7.0.0",
            "gh.exe release edit v7.0.0 --notes-file notes.md",
            "gh api repos/RamazanKara/Rubylight/releases -X POST",
            "gh workflow run rust-windows.yml -f publish=true",
            "gh workflow run release.yml -R RamazanKara/nightjar -f dry_run=false",
            "git push origin gh-pages",
            "git -C C:\\temp\\pages push -u origin gh-pages",
        ]
        for cmd in release_cmds:
            with self.subTest(phase="no_sentinel", cmd=cmd):
                self.assertEqual(self._eval(cmd)["decision"], "deny")

        (self.agents_dir / guard_commands.RELEASE_SENTINEL).write_text("authorized\n", encoding="utf-8", newline="\n")
        for cmd in release_cmds:
            with self.subTest(phase="authorized", cmd=cmd):
                res = self._eval(cmd)
                self.assertEqual(res["decision"], "allow", msg=res)

        (self.agents_dir / guard_commands.GPU_HANG_SENTINEL).write_text("halted\n", encoding="utf-8", newline="\n")
        for cmd in release_cmds:
            with self.subTest(phase="gpu_halted", cmd=cmd):
                self.assertEqual(self._eval(cmd)["decision"], "deny")

    def test_hw_commands_allowed_by_default_and_blocked_by_disable_or_gpu_halt(self) -> None:
        hw_cmds = [
            "python rust/release/e2e.py --codec hevc",
            r"pwsh tools\hw\run.ps1 -Batch desktop",
            "python tools/hw/run.py --batch desktop",
            r"pwsh tools\hw\laptop-host.ps1 -Out C:\temp\out",
            r"pwsh rust\release\check.ps1 v7.0.0",
            r"pwsh rust\release\soak.ps1",
            "python rust/release/soak.py",
            "python rust/tests/crash_restore_soak.py",
            "python rust/release/focus_ab.py quick",
            "python tools/hw/wave1_verify.py",
            "cargo test -p butterpollo-windows -- --ignored",
            "cargo nextest run --run-ignored only",
        ]
        # Allowed by default when neither RUBYLIGHT_DISABLE_HW_GATE=1 nor GPU_HANG_HALT is active.
        for cmd in hw_cmds:
            with self.subTest(phase="default_allow", cmd=cmd):
                self.assertEqual(self._eval(cmd, env={})["decision"], "allow")

        # Denied when explicitly opted out via RUBYLIGHT_DISABLE_HW_GATE=1.
        for cmd in hw_cmds:
            with self.subTest(phase="explicit_disable", cmd=cmd):
                self.assertEqual(self._eval(cmd, env={guard_commands.HW_DISABLE_ENV: "1"})["decision"], "deny")

        # Always denied when GPU_HANG_HALT sentinel exists.
        (self.agents_dir / guard_commands.GPU_HANG_SENTINEL).write_text("halted\n", encoding="utf-8", newline="\n")
        for cmd in hw_cmds:
            with self.subTest(phase="gpu_halted", cmd=cmd):
                self.assertEqual(self._eval(cmd, env={})["decision"], "deny")

        # Remote laptop build/test verification lane and wip branch push remain cleanly allowed.
        for remote_cmd in (
            r"pwsh tools\remote-check.ps1 -Host laptop -Branch wip/roadmap/03-launch-curtain",
            "git push -u origin wip/roadmap/03-launch-curtain",
        ):
            with self.subTest(phase="remote_lane", cmd=remote_cmd):
                self.assertEqual(self._eval(remote_cmd, env={})["decision"], "allow")

    def test_subprocess_cli_contract(self) -> None:
        env = dict(os.environ, RUBYLIGHT_AGENTS_DIR=str(self.agents_dir))
        env.pop(guard_commands.HW_DISABLE_ENV, None)
        proc = subprocess.run(
            [sys.executable, str(HOOKS_DIR / "guard_commands.py")],
            input=json.dumps(_pre_payload("git push origin main")),
            capture_output=True,
            text=True,
            env=env,
            check=True,
        )
        out = json.loads(proc.stdout)
        self.assertEqual(out["decision"], "deny")
        self.assertIn("RELEASE_7_0_AUTHORIZED", out["reason"])

        proc_bad = subprocess.run(
            [sys.executable, str(HOOKS_DIR / "guard_commands.py")],
            input="{not-json",
            capture_output=True,
            text=True,
            env=env,
            check=True,
        )
        self.assertEqual(json.loads(proc_bad.stdout)["decision"], "deny")


class WatchGpuHangTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)
        self.agents_dir = self.root / "main_repo" / ".agents"
        self.agents_dir.mkdir(parents=True)
        self.wt_agents_dir = self.root / "share_wt" / ".agents"
        self.wt_agents_dir.mkdir(parents=True)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_writes_sentinel_on_transcript_fallback_and_multi_workspace(self) -> None:
        transcript = self.root / "transcript.jsonl"
        # Matches real Antigravity transcript.jsonl schema: `step_index` + `content`.
        transcript.write_text(
            json.dumps({
                "step_index": 12,
                "source": "MODEL",
                "type": "GENERIC",
                "status": "DONE",
                "content": "Running soak...\nCommand exited with code 3.\n",
            }) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        payload = {
            "hookEventName": "PostToolUse",
            "stepIdx": 12,
            "transcriptPath": str(transcript),
            "workspacePaths": [str(self.root / "main_repo")],
            "cwd": str(self.root / "share_wt"),
            "toolCall": {
                "id": "toolu_12",
                "name": "run_command",
                "args": {"CommandLine": "python rust/tests/crash_restore_soak.py"},
            },
        }
        triggered = watch_gpu_hang.process_post_tool(payload, agents_dir=self.agents_dir)
        self.assertTrue(triggered)
        for target in (self.agents_dir, self.wt_agents_dir):
            halt = target / watch_gpu_hang.GPU_HANG_SENTINEL
            self.assertTrue(halt.is_file(), msg=f"missing halt in {target}")
            self.assertNotIn(b"\r\n", halt.read_bytes())

    def test_watched_runners_trigger_and_non_exit_3_ignored(self) -> None:
        for cmd in (
            "python rust/release/e2e.py --codec hevc",
            r"pwsh rust\release\check.ps1 v7.0.0",
            r"pwsh tools\hw\run.ps1 -Batch desktop",
            "python rust/release/soak.py",
            "python rust/release/focus_ab.py quick",
        ):
            payload = {
                "hookEventName": "PostToolUse",
                "toolCall": {"id": "t1", "name": "run_command", "args": {"CommandLine": cmd}},
                "exitCode": 3,
            }
            self.assertTrue(watch_gpu_hang.process_post_tool(payload, agents_dir=self.agents_dir), msg=cmd)

        other_exit = {
            "hookEventName": "PostToolUse",
            "toolCall": {"id": "t2", "name": "run_command", "args": {"CommandLine": "python rust/release/e2e.py"}},
            "error": "exit status 1",
        }
        self.assertFalse(watch_gpu_hang.process_post_tool(other_exit, agents_dir=self.agents_dir))

        # `tools\remote-check.ps1` exit code 3 is a timeout, NOT an AMD encoder hang.
        remote_timeout = {
            "hookEventName": "PostToolUse",
            "toolCall": {
                "id": "t3",
                "name": "run_command",
                "args": {"CommandLine": r"pwsh tools\remote-check.ps1 -Host laptop -Branch wip/roadmap/03-launch-curtain"},
            },
            "exitCode": 3,
        }
        self.assertFalse(watch_gpu_hang.process_post_tool(remote_timeout, agents_dir=self.agents_dir))


if __name__ == "__main__":
    unittest.main()
