#!/usr/bin/env python3
"""Run scenarios_store.rs inside skyzfs-local's tests of a checkout of the head.

usage: scenarios_store.py CHECKOUT OUT

The file goes in as a test module beside cuts.rs, `mod review_m24b2;` is
appended to cuts.rs, the tests named review_ are run with the test hooks,
and both changes are taken out again. Refuses a checkout with changes
unless ALLOW_DIRTY=1 (the plants' runner plants first, then calls this).
"""
import os
import subprocess
import sys

here = os.path.dirname(os.path.abspath(__file__))
checkout, out = sys.argv[1], sys.argv[2]
if os.environ.get("ALLOW_DIRTY") != "1":
    dirty = subprocess.run(["git", "status", "--short"], cwd=checkout, capture_output=True, text=True).stdout
    if dirty.strip():
        sys.exit("the checkout has changes:\n" + dirty)
cuts = os.path.join(checkout, "crates/skyzfs-local/src/inplace/tests/cuts.rs")
moddir = os.path.join(checkout, "crates/skyzfs-local/src/inplace/tests/cuts")
dest = os.path.join(moddir, "review_m24b2.rs")
original = open(cuts).read()
env = dict(os.environ, CARGO_INCREMENTAL="0")
try:
    os.makedirs(moddir, exist_ok=True)
    with open(dest, "w") as f:
        f.write(open(os.path.join(here, "scenarios_store.rs")).read())
    with open(cuts, "w") as f:
        f.write(original + "\nmod review_m24b2;\n")
    cmd = ["cargo", "test", "-p", "skyzfs-local", "--features", "test-hooks", "--lib", "review_", "--", "--nocapture", "--test-threads=1"]
    try:
        run = subprocess.run(cmd, cwd=checkout, env=env, capture_output=True, text=True, timeout=1500)
        text = run.stdout + "\n--- stderr\n" + run.stderr[-5000:] + f"\n--- exit {run.returncode}\n"
    except subprocess.TimeoutExpired as e:
        text = (e.stdout or b"").decode(errors="replace") + "\n--- TIMEOUT\n"
    with open(out, "w") as f:
        f.write(text)
finally:
    with open(cuts, "w") as f:
        f.write(original)
    if os.path.exists(dest):
        os.remove(dest)
    if os.path.isdir(moddir) and not os.listdir(moddir):
        os.rmdir(moddir)
