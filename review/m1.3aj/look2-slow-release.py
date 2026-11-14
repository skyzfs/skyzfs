#!/usr/bin/env python3
"""A right implementation with a release slower than the test's 300 ms step: run the (a) test 20 times."""
import os, subprocess
TREE = "/home/user/skyzfs-pr90"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
PROV = os.path.join(TREE, "crates/skynbd-api/src/provision.rs")
orig = open(PROV).read()
old = "        if let Some(s) = inner.serving.remove(name) {\n            for t in &s.tasks {"
assert orig.count(old) == 1
open(PROV, "w").write(orig.replace(old, "        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;\n" + old))
try:
    res = {}
    for i in range(10):
        p = subprocess.run(["cargo", "test", "-q", "-p", "skynbd-api", "--test", "origin_serve",
                            "a_repoint_that_converges_while_a_serve_is_under_way"], cwd=TREE, env=ENV, capture_output=True, text=True)
        out = p.stdout + p.stderr
        if "1 passed" in out:
            key = "pass"
        else:
            msg = [l for l in out.splitlines() if "panicked" in l or "assertion" in l or l.strip().startswith(("left:", "right:")) or "served" in l]
            key = "FAIL: " + " | ".join(msg)[:300]
        res[key] = res.get(key, 0) + 1
    for k, v in res.items():
        print(v, k)
finally:
    open(PROV, "w").write(orig)
print("restored:", open(PROV).read() == orig)
