#!/usr/bin/env python3
"""Next look at #153 at 3d40045: plants that survived the committed tests
(plants153b.out), each applied alone to the scratch copy patched by
scratch153b_patch.py, judged by the review's searches SD1 and SL
(scratch153b.rs). Restored in a finally; 1800 s limit each.

usage: python3 -I plants153b_search.py WORKTREE IDS
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
HERE = pathlib.Path(__file__).resolve().parent
src = (HERE / "plants153b.py").read_text()
table = src[src.index("GIVE_UP_DESTROY = "):src.index("\n\n\ndef run")]
ns = {}
exec("AG='crates/skyzfs-agent/src'\nPLAN=f'{AG}/plan.rs'\nENG=f'{AG}/engine.rs'\nZFS=f'{AG}/zfs.rs'\nCLI=f'{AG}/zfs/cli.rs'\nREC=f'{AG}/record.rs'\n" + table, ns)
PLANTS = {p[0]: p for p in ns["PLANTS"]}
for pid in sys.argv[2].split(","):
    _, what, rel, old, new = PLANTS[pid]
    path = ROOT / rel
    text = path.read_text()
    if text.count(old) != 1:
        print(json.dumps({"id": pid, "result": "ERROR: site"}))
        continue
    try:
        path.write_text(text.replace(old, new))
        out = subprocess.run(
            ["cargo", "test", "-q", "-p", "skyzfs-agent", "--release", "--lib",
             "review_scratch::s", "--", "--nocapture", "--test-threads=1"],
            cwd=ROOT, capture_output=True, text=True, timeout=1800)
        lines = (out.stdout + out.stderr).splitlines()
        keep = [l[:260] for l in lines if l.startswith(("SD1", "SL ", "  judged wrong", "    most takes"))
                or (l.startswith("    ") and " x " in l[:12])]
        print(json.dumps({"id": pid, "what": what, "rc": out.returncode, "lines": keep}), flush=True)
    finally:
        path.write_text(text)
