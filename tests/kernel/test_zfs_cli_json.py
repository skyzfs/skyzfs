#!/usr/bin/env python3
"""Unit tests for zfs_cli_json.py: records as #147's fixture.

    python3 tests/kernel/test_zfs_cli_json.py
"""
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import zfs_cli_json  # noqa: E402


def record(out, name, cmd, rc, stdout=b"", stderr=b""):
    d = os.path.join(out, name)
    os.makedirs(d)
    for fn, data in (("cmd", cmd), ("rc", rc), ("stdout", stdout), ("stderr", stderr)):
        with open(os.path.join(d, fn), "wb") as f:
            f.write(data)


class Records(unittest.TestCase):
    def test_the_fixtures_shape(self):
        with tempfile.TemporaryDirectory() as out:
            record(out, "rig-get", b"zpool get -H skzq\n", b"0\n", b"guid\t1\n")
            record(out, "none-get", b"zpool get -H skznone\n", b"1\n", b"", b"no such pool available.\n")
            self.assertEqual(zfs_cli_json.records(out), {
                "none-get": {"cmd": "zpool get -H skznone\n", "rc": 1, "stdout": "",
                             "stderr": "no such pool available.\n"},
                "rig-get": {"cmd": "zpool get -H skzq\n", "rc": 0, "stdout": "guid\t1\n", "stderr": ""},
            })

    def test_a_note_is_carried(self):
        # A catch that failed says so in its note: the fixture keeps it,
        # as the first fixture (bd37422) did, text and newline.
        with tempfile.TemporaryDirectory() as out:
            record(out, "mirror-resilver-status", b"zpool status skzqm\n", b"0\n", b"ONLINE\n")
            note = "not caught: no output of 100 matched /resilver in progress/; this is the last\n"
            with open(os.path.join(out, "mirror-resilver-status", "note"), "w") as f:
                f.write(note)
            record(out, "rig-get", b"zpool get -H skzq\n", b"0\n")
            got = zfs_cli_json.records(out)
            self.assertEqual(got["mirror-resilver-status"]["note"], note)
            self.assertNotIn("note", got["rig-get"])

    def test_scale_and_the_rest_left_out(self):
        with tempfile.TemporaryDirectory() as out:
            record(out, "scale-1000-members", b"zfs program\n", b"0\n", b"{}")
            record(out, ".writer-x", b"x\n", b"0\n")
            os.makedirs(os.path.join(out, "no-rc"))
            with open(os.path.join(out, "about.txt"), "w") as f:
                f.write("about\n")
            record(out, "version", b"zfs version\n", b"0\n", b"zfs-2.4.1\n")
            self.assertEqual(sorted(zfs_cli_json.records(out)), ["version"])

    def test_not_utf8_kept_in_hex(self):
        with tempfile.TemporaryDirectory() as out:
            record(out, "rig-get-odd", b"zfs get\n", b"0\n", b"a\xff\xfeb\n")
            r = zfs_cli_json.records(out)["rig-get-odd"]
            self.assertEqual((r["stdout"], r["stdout_hex"]), ("a��b\n", "61fffe620a"))
            self.assertNotIn("stderr_hex", r)

    def test_utf8_beyond_ascii_is_text(self):
        with tempfile.TemporaryDirectory() as out:
            record(out, "rig-set-utf8", b"zfs set\n", b"0\n", "naïve, 日本\n".encode())
            r = zfs_cli_json.records(out)["rig-set-utf8"]
            self.assertEqual(r["stdout"], "naïve, 日本\n")
            self.assertNotIn("stdout_hex", r)

    def test_a_missing_output_is_empty(self):
        with tempfile.TemporaryDirectory() as out:
            record(out, "r", b"x\n", b"124\n")
            os.remove(os.path.join(out, "r", "stderr"))
            self.assertEqual(zfs_cli_json.records(out)["r"], {"cmd": "x\n", "rc": 124, "stdout": "", "stderr": ""})


class CommandLine(unittest.TestCase):
    def test_the_command_line(self):
        with tempfile.TemporaryDirectory() as out:
            record(out, "version", b"zfs version\n", b"0\n", b"zfs-2.4.1\n")
            r = subprocess.run([sys.executable, os.path.join(HERE, "zfs_cli_json.py"), out, "the run"],
                               capture_output=True, text=True)
            self.assertEqual(r.returncode, 0)
            self.assertEqual(json.loads(r.stdout), {"records": {"version": {
                "cmd": "zfs version\n", "rc": 0, "stdout": "zfs-2.4.1\n", "stderr": ""}}, "source": "the run"})
        with tempfile.TemporaryDirectory() as out:
            r = subprocess.run([sys.executable, os.path.join(HERE, "zfs_cli_json.py"), out, "s"],
                               capture_output=True, text=True)
            self.assertEqual(r.returncode, 2)


if __name__ == "__main__":
    unittest.main()
