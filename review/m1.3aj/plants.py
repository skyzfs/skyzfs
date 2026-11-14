#!/usr/bin/env python3
"""Plants for #90 at 8dfe6e1: one at a time in the worktree, each with the tests named, then restored."""
import os, subprocess, sys
TREE = "/home/user/skyzfs-pr90"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
ORIGIN = "crates/skynbd-nbd/src/origin.rs"
PROV = "crates/skynbd-api/src/provision.rs"
LIB = "crates/skynbd-api/src/lib.rs"
NBD = ["cargo", "test", "-p", "skynbd-nbd", "--lib", "origin::tests"]
NBDI = ["cargo", "test", "-p", "skynbd-nbd", "--test", "origin"]
PROVT = ["cargo", "test", "-p", "skynbd-api", "--lib", "provision::tests"]
SERVE = ["cargo", "test", "-p", "skynbd-api", "--test", "origin_serve"]
APIT = ["cargo", "test", "-p", "skynbd-api", "--test", "api", "origin"]
ALL = [NBD, PROVT, SERVE, APIT]
TERMCHECK = "    if b.term.ended() {\n        return Ok(());\n    }\n"
LOCK = "    let _order = registry.swap_lock(&export_name).await;\n"
PLANTS = [
    ("R0 none", [], ALL + [NBDI]),
    ("R1 reconcile does not read the term", [(ORIGIN, TERMCHECK, "")], ALL),
    ("R2 reconcile reads the term before the lock", [(ORIGIN,
        "    // Held from the row's reading to the swap (M1.3ai, module doc).\n" + LOCK + "    // Read under the lock that `retire` ends it under (M1.3aj): a\n    // serving that has ended converges nothing, not even a removal,\n    // since the export may be a later serving's by now.\n" + TERMCHECK,
        TERMCHECK + LOCK)], ALL),
    ("R3 retire takes no lock", [(ORIGIN,
        "    let _order = registry.swap_lock(&export_name).await;\n    term.end();", "    term.end();")], ALL),
    ("R4 teardown does not wait for its tasks", [(PROV,
        "            for t in s.tasks {\n                // Cancelled, or ended on its own (a panic included): the\n                // task is gone either way.\n                let _ = t.await;\n            }\n", "")], ALL),
    ("R5 teardown does not retire", [(PROV,
        "            skynbd_nbd::origin::retire(&self.registry, name, &s.term).await;\n", "")], ALL),
    ("R6 converge_origin reports a serving that ended as current", [(LIB,
        "    Ok((!h.term.ended()).then_some(h))", "    Ok(Some(h))")], ALL),
    ("R7 retire ends the term but unregisters nothing", [(ORIGIN,
        "    term.end();\n    registry.unregister(&export_name).await;\n}", "    term.end();\n}")], ALL),
    ("R8 retire after the tasks' wait is moved before the abort", [(PROV,
        "        if let Some(s) = inner.serving.remove(name) {\n            for t in &s.tasks {",
        "        if let Some(s) = inner.serving.remove(name) {\n            skynbd_nbd::origin::retire(&self.registry, name, &s.term).await;\n            for t in &s.tasks {"),
     (PROV, "            }\n            skynbd_nbd::origin::retire(&self.registry, name, &s.term).await;\n        }\n        inner.dynamic.remove(name);",
            "            }\n        }\n        inner.dynamic.remove(name);")], ALL),
]

def run(cmd):
    p = subprocess.run(cmd, cwd=TREE, env=ENV, capture_output=True, text=True)
    out = p.stdout + p.stderr
    fails = sorted({l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.rstrip().endswith("FAILED")})
    res = [l.split(";")[0][13:] for l in out.splitlines() if l.startswith("test result:")]
    err = "" if p.returncode == 0 or fails else " | " + " / ".join(l for l in out.splitlines() if l.startswith("error"))[:300]
    return p.returncode, fails, res, err

for name, edits, tests in PLANTS:
    ok = True
    for path, old, new in edits:
        full = os.path.join(TREE, path); text = open(full).read()
        if text.count(old) != 1:
            print(f"{name}: ANCHOR found {text.count(old)} times in {path}", flush=True); ok = False; break
        open(full, "w").write(text.replace(old, new))
    if ok:
        for t in tests:
            rc, fails, res, err = run(t)
            print(f"{name} [{' '.join(t[2:6])}]: rc {rc}; failed: {fails or 'nothing'}; {res}{err}", flush=True)
    if edits:
        subprocess.run(["git", "checkout", "--"] + sorted({e[0] for e in edits}), cwd=TREE)
st = subprocess.run(["git", "status", "--short"], cwd=TREE, capture_output=True, text=True).stdout
print("worktree:", st.strip() or "clean")
