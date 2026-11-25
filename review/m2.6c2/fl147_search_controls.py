#!/usr/bin/env python3
"""Fourth look at #147: the planner search (fl147_plan_search.rs) at the
head and with plants, to show it finds what it is meant to find. Each
plant applied alone, the search appended to plan/tests.rs, run, and all
restored in a finally.

usage: python3 -I fl147_search_controls.py WORKTREE
"""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
HERE = pathlib.Path(__file__).resolve().parent
PLAN = ROOT / "crates/skyzfs-agent/src/plan.rs"
TESTS = ROOT / "crates/skyzfs-agent/src/plan/tests.rs"

PLANTS = [
    ("head", None, None),
    ("A2 Done releases only a whole hold",
     "Phase::Done { .. } if pool.hold != Held::No => Action::Release,",
     "Phase::Done { .. } if pool.hold == Held::All => Action::Release,"),
    ("A3 away releases only a whole hold",
     "seen.now_ms >= until && pool.hold != Held::No => Action::Release,",
     "seen.now_ms >= until && pool.hold == Held::All => Action::Release,"),
    ("A5 pending: a part read as held",
     "if pool.hold != Held::All || broken.is_some() || moved {",
     "if pool.hold == Held::No || broken.is_some() || moved {"),
    ("M1 unknown: a part read as held (Ask)",
     "None if pool.hold == Held::All && broken.is_none() && !moved => Action::Ask,",
     "None if pool.hold != Held::No && broken.is_none() && !moved => Action::Ask,"),
    ("M5 pending with a part: release",
     "        Reply::Pending => {\n            if pool.hold != Held::All || broken.is_some() || moved {",
     "        Reply::Pending => {\n            if pool.hold == Held::Part {\n                return Action::Release;\n            }\n            if pool.hold != Held::All || broken.is_some() || moved {"),
]

saved = {p: p.read_text() for p in (PLAN, TESTS)}
try:
    TESTS.write_text(saved[TESTS] + (HERE / "fl147_plan_search.rs").read_text())
    for label, old, new in PLANTS:
        PLAN.write_text(saved[PLAN])
        if old:
            assert saved[PLAN].count(old) == 1, label
            PLAN.write_text(saved[PLAN].replace(old, new))
        out = subprocess.run(
            ["timeout", "600", "cargo", "test", "-p", "skyzfs-agent", "--lib",
             "fl147_", "--", "--nocapture"],
            cwd=ROOT, capture_output=True, text=True)
        text = out.stdout + out.stderr
        summary = [l for l in text.splitlines() if l.startswith("FL147")]
        first = [l for l in text.splitlines() if ": no " in l and "part " in l][:1]
        print(f"== {label}: exit {out.returncode}")
        for l in summary + first:
            print("  " + l[:400])
finally:
    for p, t in saved.items():
        p.write_text(t)
st = subprocess.run(["git", "status", "--short"], cwd=ROOT, capture_output=True, text=True)
print("worktree after:", st.stdout.strip() or "clean")
