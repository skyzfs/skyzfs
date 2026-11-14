#!/usr/bin/env python3
"""U3 (commands sent after the deadline) against the three-ways test alone, 10 runs."""
import os, subprocess
TREE = "/home/user/skyzfs-pr91"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
SERVE = os.path.join(TREE, "bins/skyzfsd/src/serve.rs")
orig = open(SERVE).read()
old = "        if left.is_zero() {\n"
assert orig.count(old) == 1
open(SERVE, "w").write(orig.replace(old, "        if left.is_zero() && false {\n"))
try:
    res = {}
    for i in range(10):
        p = subprocess.run(["timeout", "120", "cargo", "test", "-q", "-p", "skyzfsd", "--features", "test-hooks", "--lib", "--",
                            "serve::tests::a_start_given_up_ends_within_one_limit"], cwd=TREE, env=ENV, capture_output=True, text=True)
        out = p.stdout + p.stderr
        if "1 passed" in out:
            key = "passes"
        else:
            key = "fails: " + " | ".join(l.strip() for l in out.splitlines() if l.strip().startswith(("left:", "right:")) or "panicked" in l)[:400]
        res[key] = res.get(key, 0) + 1
    for k, v in res.items():
        print(v, k)
finally:
    open(SERVE, "w").write(orig)
print("restored:", open(SERVE).read() == orig)
