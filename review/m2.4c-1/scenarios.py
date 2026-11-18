#!/usr/bin/env python3
"""Run the review's scenarios against a checkout of the head.

usage: scenarios.py CHECKOUT OUT [FILTER ...]

Copies scenarios.rs into the checkout's crates/skyzfs-upload/tests/ as
review_scn.rs, runs it, writes the output to OUT, and removes the file.
Refuses a checkout with changes, unless ALLOW_DIRTY is set (plants.py
runs a scenario with a plant in place). CARGO_TARGET_DIR is taken from
the environment.
"""
import os
import shutil
import signal
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
limit = int(os.environ.get("SCN_TIMEOUT", "1500"))
try:
    cmd = ["cargo", "test", "-p", "skyzfs-upload", "--test", "review_scn", "--", "--nocapture", "--test-threads=1"] + flt
    proc = subprocess.Popen(cmd, cwd=checkout, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            text=True, start_new_session=True)
    try:
        stdout, stderr = proc.communicate(timeout=limit)
        lines = stderr.splitlines()
        panics = []
        for i, line in enumerate(lines):
            if "panicked at" in line:
                panics += lines[i:i + 400] + [""]
        errors = [l for l in lines if l.startswith("error") or l.startswith("  -->")]
        text = (stdout + "\n--- panics\n" + "\n".join(panics)[:40000]
                + ("\n--- build errors\n" + "\n".join(errors)[:6000] if proc.returncode == 101 and "test result" not in stdout else "")
                + "\n--- stderr (its end)\n" + stderr[-3000:] + f"\n--- exit {proc.returncode}\n")
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        stdout, stderr = proc.communicate()
        text = stdout + "\n--- stderr (its end)\n" + stderr[-3000:] + f"\n--- TIMEOUT after {limit} s\n"
    with open(out, "w") as f:
        f.write(text)
finally:
    os.remove(dest)
print(subprocess.run(["git", "status", "--short"], cwd=checkout, capture_output=True, text=True).stdout or "clean")
