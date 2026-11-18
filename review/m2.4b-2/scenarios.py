#!/usr/bin/env python3
"""Run the review's scenarios against a checkout of the head.

usage: scenarios.py CHECKOUT OUT [FILTER]

Copies scenarios.rs into the checkout's crates/skyzfs-upload/tests/ as
review_scn.rs, runs it, writes the output to OUT, and removes the file.
Refuses a checkout with changes, unless ALLOW_DIRTY is set (plants.py
runs a scenario with a plant in place).
"""
import os
import shutil
import subprocess
import sys

here = os.path.dirname(os.path.abspath(__file__))
checkout, out = sys.argv[1], sys.argv[2]
flt = sys.argv[3:]
dirty = subprocess.run(["git", "status", "--short"], cwd=checkout, capture_output=True, text=True).stdout
if dirty.strip() and not os.environ.get("ALLOW_DIRTY"):
    sys.exit("the checkout has changes:\n" + dirty)
dest = os.path.join(checkout, "crates/skyzfs-upload/tests/review_scn.rs")
shutil.copy(os.path.join(here, "scenarios.rs"), dest)
env = dict(os.environ, CARGO_INCREMENTAL="0")
try:
    cmd = ["cargo", "test", "-p", "skyzfs-upload", "--test", "review_scn", "--", "--nocapture", "--test-threads=1"] + flt
    try:
        run = subprocess.run(cmd, cwd=checkout, env=env, capture_output=True, text=True, timeout=2400)
        lines = run.stderr.splitlines()
        panics = []
        for i, line in enumerate(lines):
            if "panicked at" in line:
                panics += lines[i:i + 6] + [""]
        text = (run.stdout + "\n--- panics\n" + "\n".join(panics)[:6000]
                + "\n--- stderr (its end)\n" + run.stderr[-3000:] + f"\n--- exit {run.returncode}\n")
    except subprocess.TimeoutExpired as e:
        text = (e.stdout or b"").decode(errors="replace") + "\n--- TIMEOUT\n"
    with open(out, "w") as f:
        f.write(text)
finally:
    os.remove(dest)
print(subprocess.run(["git", "status", "--short"], cwd=checkout, capture_output=True, text=True).stdout or "clean")
