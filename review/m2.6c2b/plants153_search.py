#!/usr/bin/env python3
"""Review of #153 at 65d72ee: the plants that survived the committed
tests (plants153.out), each applied alone to the scratch copy patched by
scratch153_patch.py, judged by the review's probes PA and PM
(scratch153.rs) with the GUID oracle. B1 is a control (killed by the
committed tests). A plant is KILLED here where PA or PM finds a wrong
run.

usage: python3 -I plants153_search.py WORKTREE [IDS]
"""
import importlib.util
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
HERE = pathlib.Path(__file__).resolve().parent
src = (HERE / "plants153.py").read_text()
# The plant table only (the module runs main() on import).
table = src[src.index("PLANTS = ["):src.index("\n\n\ndef run")]
ns = {}
exec("AG='crates/skyzfs-agent/src'\nPLAN=f'{AG}/plan.rs'\nENG=f'{AG}/engine.rs'\nZFS=f'{AG}/zfs.rs'\nCLI=f'{AG}/zfs/cli.rs'\nLUA=f'{AG}/zfs/cli/members.lua'\n" + table, ns)
PLANTS = {p[0]: p for p in ns["PLANTS"]}
ids = sys.argv[2].split(",") if len(sys.argv) > 2 else ["B1", "P2", "P3", "P5", "P6", "P7", "P8"]
for pid in ids:
    _, what, rel, old, new = PLANTS[pid]
    path = ROOT / rel
    text = path.read_text()
    if text.count(old) != 1:
        print(json.dumps({"id": pid, "result": "ERROR: site"}))
        continue
    try:
        path.write_text(text.replace(old, new))
        out = subprocess.run(
            ["cargo", "test", "-p", "skyzfs-agent", "--release", "--lib",
             "review_scratch::p", "--", "--nocapture", "--test-threads=1"],
            cwd=ROOT, capture_output=True, text=True, timeout=900)
        lines = (out.stdout + out.stderr).splitlines()
        pa = next((l for l in lines if "PA:" in l), "")
        pm = next((l.split("PM ", 1)[-1] for l in lines if "PM (" in l), "")
        pm_strict = next((l.strip() for l in lines if "strict (GUID oracle)" in l), "")
        wrong = [l.strip() for l in lines if "WRONG" in l][:2]
        print(json.dumps({"id": pid, "what": what, "PA": pa.split("PA:")[-1].strip(),
                          "PM": pm, "PM strict": pm_strict, "e.g.": wrong}), flush=True)
    finally:
        path.write_text(text)
