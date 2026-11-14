#!/usr/bin/env python3
"""Narrow plant: a request received after a waiting flush runs at once when only flushes wait
and it need not wait for the threads (overlap order among the other requests is kept).
Runs skyzfs-local's tests, plain and with test-hooks, then restores."""
import os, subprocess
TREE = "/home/user/skyzfs-m2"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
F = os.path.join(TREE, "crates/skyzfs-local/src/inplace.rs")
orig = open(F).read()
old = "        if !self.deferred.is_empty() || self.must_wait(&request.op) {\n"
new = ("        let only_flushes_wait = self.deferred.iter().all(|r| matches!(r.op, Op::Flush))\n"
       "            && !matches!(request.op, Op::Flush);\n"
       "        if (!self.deferred.is_empty() && !only_flushes_wait) || self.must_wait(&request.op) {\n")
assert orig.count(old) == 1
open(F, "w").write(orig.replace(old, new))
try:
    for feats in ([], ["--features", "test-hooks"]):
        p = subprocess.run(["timeout", "900", "cargo", "test", "-p", "skyzfs-local", *feats], cwd=TREE, env=ENV, capture_output=True, text=True)
        out = p.stdout + p.stderr
        fails = sorted({l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.rstrip().endswith("FAILED")})
        res = [l.split(";")[0][13:] + ";" + l.split(";")[1] for l in out.splitlines() if l.startswith("test result:")]
        err = [l for l in out.splitlines() if l.startswith("error")][:3]
        print(feats or "plain", "rc", p.returncode, "failed:", fails or "nothing", res, err, flush=True)
finally:
    open(F, "w").write(orig)
print("restored:", open(F).read() == orig)
