#!/usr/bin/env python3
"""Plants for #88 at 7d5bdb7: one at a time, each with the tests named, then restored."""
import os, subprocess, sys

TREE = "/home/user/skyzfs-pr88"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
ORIGIN = "crates/skynbd-nbd/src/origin.rs"
SERVER = "crates/skynbd-nbd/src/server.rs"
API = "crates/skynbd-api/src/lib.rs"

UNIT = ["cargo", "test", "-p", "skynbd-nbd", "--lib", "origin::tests"]
APIT = ["cargo", "test", "-p", "skynbd-api", "--test", "api", "origin"]
REPRO = ["cargo", "test", "-p", "skynbd-api", "--test", "origin_race_repro", "--", "--nocapture"]

LOCK = "    // Held from the row's reading to the swap (M1.3ai, module doc).\n    let _order = registry.swap_lock(&export_name).await;\n"
BRANCH = "        Some(r) => {\n            between().await;\n            swap(registry, b, r.commit).await\n        }\n"

PLANTS = [
    ("B0 none", [], [UNIT, APIT]),
    ("B1 reconcile takes no lock", [(ORIGIN, LOCK, "")], [UNIT, APIT]),
    ("B2a the lock taken after the row is resolved, before the seam", [(ORIGIN, LOCK, ""),
      (ORIGIN, BRANCH, "        Some(r) => {\n            let _order = registry.swap_lock(&export_name).await;\n            between().await;\n            swap(registry, b, r.commit).await\n        }\n")], [UNIT, APIT]),
    ("B2b the lock taken after the seam, just before the swap", [(ORIGIN, LOCK, ""),
      (ORIGIN, BRANCH, "        Some(r) => {\n            between().await;\n            let _order = registry.swap_lock(&export_name).await;\n            swap(registry, b, r.commit).await\n        }\n")], [UNIT, APIT]),
    ("B3 set_origin swaps to its own target with repoint", [(API,
      "                skynbd_nbd::origin::reconcile(&state.registry, &binding).await?;\n                let served = origin_served(&state, &vol, &h).await;\n                return Ok(Json(origin_info(",
      "                skynbd_nbd::origin::repoint(&state.registry, &binding, target.commit).await?;\n                let served = origin_served(&state, &vol, &h).await;\n                return Ok(Json(origin_info(")], [UNIT, APIT]),
    ("B4 swap_lock hands out a fresh lock each time", [(SERVER,
      "            Arc::clone(swaps.entry(name.to_string()).or_default())",
      "            { let _ = swaps.entry(name.to_string()).or_default(); Arc::new(tokio::sync::Mutex::new(())) }")], [UNIT, APIT]),
    ("B5 the review's reproducer: 100 ms before the swap, under the lock", [(ORIGIN, BRANCH,
      "        Some(r) => {\n            tokio::time::sleep(std::time::Duration::from_millis(100)).await;\n            between().await;\n            swap(registry, b, r.commit).await\n        }\n")], [REPRO, APIT]),
    ("B6 the review's reproducer, 100 ms, with no lock (the base's shape)", [(ORIGIN, LOCK, ""), (ORIGIN, BRANCH,
      "        Some(r) => {\n            tokio::time::sleep(std::time::Duration::from_millis(100)).await;\n            between().await;\n            swap(registry, b, r.commit).await\n        }\n")], [REPRO]),
]


def run(cmd):
    p = subprocess.run(cmd, cwd=TREE, env=ENV, capture_output=True, text=True)
    out = p.stdout + p.stderr
    fails = sorted({l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.rstrip().endswith("FAILED")})
    race = [l[:200] for l in out.splitlines() if l.startswith("RACE")]
    res = [l.split(";")[0] for l in out.splitlines() if l.startswith("test result:")]
    panics = [l.strip()[:160] for l in out.splitlines() if "assert" in l and ("failed" in l or "panicked" in l)][:2]
    err = "" if p.returncode == 0 or fails else " | " + " / ".join(l for l in out.splitlines() if l.startswith("error"))[:400]
    return p.returncode, fails, res, race, panics, err


def main():
    for name, edits, tests in PLANTS:
        ok = True
        for path, old, new in edits:
            full = os.path.join(TREE, path)
            text = open(full).read()
            if text.count(old) != 1:
                print(f"{name}: ANCHOR NOT FOUND ONCE in {path}", flush=True)
                ok = False
                break
            open(full, "w").write(text.replace(old, new))
        if ok:
            for t in tests:
                rc, fails, res, race, panics, err = run(t)
                print(f"{name} [{' '.join(t[2:7])}]: rc {rc}; failed: {fails or 'nothing'}; {res} {race} {panics}{err}", flush=True)
        if edits:
            subprocess.run(["git", "checkout", "--"] + sorted({e[0] for e in edits}), cwd=TREE)
    st = subprocess.run(["git", "status", "--short"], cwd=TREE, capture_output=True, text=True).stdout
    print("worktree:", st.strip() or "clean")


if __name__ == "__main__":
    sys.exit(main())
