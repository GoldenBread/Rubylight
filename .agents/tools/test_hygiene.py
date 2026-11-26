"""Unit tests for .agents/tools/roadmap_hygiene.py."""
from __future__ import annotations

import os
import pathlib
import sys
import tempfile
import time
import unittest

TOOLS_MOD_DIR = pathlib.Path(__file__).resolve().parent
if str(TOOLS_MOD_DIR) not in sys.path:
    sys.path.insert(0, str(TOOLS_MOD_DIR))

import roadmap_hygiene as hygiene  # noqa: E402


class RoadmapHygieneTests(unittest.TestCase):
    def test_decide_lane_routing_matrix(self) -> None:
        # Both PC and laptop free -> use both effectively!
        res = hygiene.decide_lane(
            {"build_lock_free": True, "priority_live": False, "cargo_total": 4, "cargo_free": 3, "gradle_free": True, "c_free_gb": 180.0, "d_free_gb": 45.0},
            {"available": True, "reason": "idle"},
        )
        self.assertEqual(res["recommended_lane"], "local_and_laptop")
        self.assertFalse(res["disk_warning"])

        # PC Cargo lanes saturated, laptop idle -> offload to laptop!
        res2 = hygiene.decide_lane(
            {"build_lock_free": True, "priority_live": False, "cargo_total": 4, "cargo_free": 1, "gradle_free": True, "c_free_gb": 180.0, "d_free_gb": 45.0},
            {"available": True, "reason": "idle"},
        )
        self.assertEqual(res2["recommended_lane"], "laptop")

        # PC exclusive BUILD-LOCK held, laptop idle -> offload to laptop!
        res3 = hygiene.decide_lane(
            {"build_lock_free": False, "priority_live": False, "cargo_total": 4, "cargo_free": 0, "gradle_free": True, "c_free_gb": 180.0, "d_free_gb": 45.0},
            {"available": True, "reason": "idle"},
        )
        self.assertEqual(res3["recommended_lane"], "laptop")

        # Laptop unavailable, PC has 1 Cargo slot -> stay local.
        res4 = hygiene.decide_lane(
            {"build_lock_free": True, "priority_live": False, "cargo_total": 4, "cargo_free": 1, "gradle_free": True, "c_free_gb": 30.0, "d_free_gb": 10.0},
            {"available": False, "reason": "busy"},
        )
        self.assertEqual(res4["recommended_lane"], "local")
        self.assertTrue(res4["disk_warning"])

        # Everything busy -> wait.
        res5 = hygiene.decide_lane(
            {"build_lock_free": True, "priority_live": False, "cargo_total": 4, "cargo_free": 0, "gradle_free": False, "c_free_gb": 180.0, "d_free_gb": 45.0},
            {"available": False, "reason": "busy"},
        )
        self.assertEqual(res5["recommended_lane"], "wait")

    def test_find_private_garbage_and_expired_logs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "target").mkdir()
            wt = root / ".claude" / "worktrees" / "w1-21"
            (wt / "client" / "app" / "build").mkdir(parents=True)
            (wt / "client" / "protocol" / "target").mkdir(parents=True)

            found = hygiene.find_private_garbage(root)
            rels = sorted(p.relative_to(root).as_posix() for p in found)
            self.assertEqual(
                rels,
                [
                    ".claude/worktrees/w1-21/client/app/build",
                    ".claude/worktrees/w1-21/client/protocol/target",
                    "target",
                ],
            )

            dev_check = root / "dev-check"
            old_run = dev_check / "laptop-wip-roadmap-03-curtain"
            new_run = dev_check / "laptop-wip-roadmap-04-warp"
            unrelated = dev_check / "other-branch"
            for d in (old_run, new_run, unrelated):
                d.mkdir(parents=True)
            now = time.time()
            old_ts = now - 5 * 86400
            os.utime(old_run, (old_ts, old_ts))
            os.utime(unrelated, (old_ts, old_ts))

            expired = hygiene.find_expired_logs_and_scratch(
                dev_check_root=dev_check,
                scratch_roots=(),
                max_age_days=3.0,
                now_ts=now,
            )
            self.assertEqual(expired, [old_run])

    def test_scan_diff_for_firewall_risks(self) -> None:
        bad_diff = "\n".join([
            "+++ b/tests/host/tests/new_probe_test.rs",
            "@@ -0,0 +1,5 @@",
            '+let sock = UdpSocket::bind("0.0.0.0:0").unwrap();',  # firewall-ok: synthetic test input
            '+let ok_sock = TcpListener::bind("127.0.0.1:0").unwrap();',
            '+let v6 = TcpListener::bind("[::]:47989").unwrap();',  # firewall-ok: synthetic test input
            '+let annotated = UdpSocket::bind("0.0.0.0:47998").unwrap(); // firewall-ok: explicit LAN integration fixture',
            "+++ b/rust/core/session/src/lib.rs",
            "@@ -10,0 +11,3 @@",
            "+#[cfg(test)]",
            "+mod tests {",
            "+    let s = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));",  # firewall-ok: synthetic test input
            "+++ b/rust/host/web/src/server.rs",
            "@@ -1,0 +2,1 @@",
            '+let prod = TcpListener::bind("0.0.0.0:47990")?;',  # firewall-ok: synthetic test input
        ])
        findings = hygiene.scan_diff_for_firewall_risks(bad_diff)
        self.assertEqual(len(findings), 3, msg=findings)
        self.assertEqual([(f["file"], f["line"]) for f in findings], [
            ("tests/host/tests/new_probe_test.rs", 1),
            ("tests/host/tests/new_probe_test.rs", 3),
            ("rust/core/session/src/lib.rs", 13),
        ])

        clean_diff = "\n".join([
            "+++ b/tests/host/tests/loopback_test.rs",
            "@@ -0,0 +1,2 @@",
            '+let s = TcpListener::bind("127.0.0.1:0").unwrap();',
            '+let v6 = UdpSocket::bind((Ipv6Addr::LOCALHOST, 0)).unwrap();',
        ])
        self.assertEqual(hygiene.scan_diff_for_firewall_risks(clean_diff), [])


if __name__ == "__main__":
    unittest.main()
