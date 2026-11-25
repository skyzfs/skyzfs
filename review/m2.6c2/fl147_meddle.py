#!/usr/bin/env python3
"""Fourth look at #147: plant A12 (ask_anew's own read of the hold taken
as held on a part) against a scratch variant of the existing test
`asking_anew_checks_the_snapshot_and_its_hold_before_the_count`: a fourth
meddle, the agent's tag released on `tank/db@t` only, between the round
and the ask anew. Run at the head and with A12; restored in a finally.

usage: python3 -I fl147_meddle.py WORKTREE
"""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
TESTS = ROOT / "crates/skyzfs-agent/src/engine/tests.rs"
ENG = ROOT / "crates/skyzfs-agent/src/engine.rs"

EDITS = [
    ("    /// `pool@t` replaced by a snapshot of the same name (another GUID).\n    Replaced,\n}",
     "    /// `pool@t` replaced by a snapshot of the same name (another GUID).\n    Replaced,\n    /// Fourth look at #147 (scratch): the tag released on `tank/db@t` only.\n    PartlyReleased,\n}"),
    ("    for meddle in [Meddle::None, Meddle::Released, Meddle::Replaced] {",
     "    for meddle in [Meddle::None, Meddle::Released, Meddle::Replaced, Meddle::PartlyReleased] {"),
    ("                    Meddle::Replaced if *name == format!(\"{POOL}{suffix}\") => snap.guid ^= 1,",
     "                    Meddle::Replaced if *name == format!(\"{POOL}{suffix}\") => snap.guid ^= 1,\n                    Meddle::PartlyReleased if *name == format!(\"{POOL}/db{suffix}\") => {\n                        snap.holds.clear();\n                    }"),
    ("            Meddle::Released | Meddle::Replaced => {",
     "            Meddle::Released | Meddle::Replaced | Meddle::PartlyReleased => {"),
]
A12 = ("let held = self.zfs.held(&self.pool, &name, HOLD_TAG)? == Held::All;",
       "let held = self.zfs.held(&self.pool, &name, HOLD_TAG)? != Held::No;")


def test(label):
    out = subprocess.run(
        ["timeout", "600", "cargo", "test", "-p", "skyzfs-agent", "--lib",
         "asking_anew_checks_the_snapshot_and_its_hold_before_the_count"],
        cwd=ROOT, capture_output=True, text=True)
    lines = [l for l in (out.stdout + out.stderr).splitlines()
             if "test result" in l or "panicked" in l or "FAILED" in l
             or l.strip().startswith("left") or l.strip().startswith("right")
             or "PartlyReleased" in l]
    print(f"== {label}: exit {out.returncode}")
    for l in lines[:12]:
        print("  " + l[:300])


saved = {p: p.read_text() for p in (TESTS, ENG)}
try:
    text = TESTS.read_text()
    for old, new in EDITS:
        assert text.count(old) == 1, old
        text = text.replace(old, new)
    TESTS.write_text(text)
    test("head, with the fourth meddle")
    e = ENG.read_text()
    assert e.count(A12[0]) == 1
    ENG.write_text(e.replace(A12[0], A12[1]))
    test("A12 planted, with the fourth meddle")
    TESTS.write_text(saved[TESTS])
    test("A12 planted, the test as committed")
finally:
    for p, t in saved.items():
        p.write_text(t)
st = subprocess.run(["git", "status", "--short"], cwd=ROOT, capture_output=True, text=True)
print("worktree after:", st.stdout.strip() or "clean")
