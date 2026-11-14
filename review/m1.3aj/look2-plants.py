#!/usr/bin/env python3
"""Plants for #90 at 758ced6: one at a time in the worktree, then restored."""
import os, subprocess
TREE = "/home/user/skyzfs-pr90"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
PROV = "crates/skynbd-api/src/provision.rs"
LIB = "crates/skynbd-api/src/lib.rs"
LIBT = ["cargo", "test", "-p", "skynbd-api", "--lib"]
SERVE = ["cargo", "test", "-p", "skynbd-api", "--test", "origin_serve"]
APIT = ["cargo", "test", "-p", "skynbd-api", "--test", "api"]
ALL = [LIBT, SERVE, APIT]
SECOND = "    match &state.provisioner {\n        Some(prov) => Ok(prov.converge_origin(vol).await?),\n        None => Ok(None),\n    }\n"
LOCKED = "        let inner = self.inner.lock().await;\n        if !inner.serving.contains_key(name) {\n            return Ok(None);\n        }\n"
PLANTS = [
    ("S0 none", []),
    ("S1 no second look", [(LIB, SECOND, "    Ok(None)\n")]),
    ("S2 the second look takes no provisioner lock", [(PROV, LOCKED, ""), (PROV, "        .await?;\n        drop(inner);\n        Ok(Some(h))", "        .await?;\n        Ok(Some(h))")]),
    ("S3 the second look does not reconcile", [(PROV, "        drop(inner);\n        Ok(Some(h))\n    }", "        drop(inner);\n        Ok(Some(h))\n    }\n    #[allow(dead_code)]\n    async fn unused(&self) {}"),
        (PROV, "        skynbd_nbd::origin::reconcile(\n            &self.registry,\n            &VolumeBinding {\n                vol_id: h.vol_id,\n                vol_name: name.to_string(),", "        let _ = (\n            &self.registry,\n            &VolumeBinding {\n                vol_id: h.vol_id,\n                vol_name: name.to_string(),"),
        (PROV, "                term: h.term.clone(),\n            },\n        )\n        .await?;\n        drop(inner);", "                term: h.term.clone(),\n            },\n        );\n        drop(inner);")]),
    ("S4 the second look lets the lock go before it reconciles", [(PROV, "        let Some(h) = self.volumes.read().await.get(name).cloned() else {\n            return Ok(None);\n        };\n", "        let Some(h) = self.volumes.read().await.get(name).cloned() else {\n            return Ok(None);\n        };\n        drop(inner);\n"), (PROV, "        .await?;\n        drop(inner);\n        Ok(Some(h))", "        .await?;\n        Ok(Some(h))")]),
]

def run(cmd):
    p = subprocess.run(cmd, cwd=TREE, env=ENV, capture_output=True, text=True)
    out = p.stdout + p.stderr
    fails = sorted({l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.rstrip().endswith("FAILED")})
    res = [l.split(";")[0][13:] for l in out.splitlines() if l.startswith("test result:")]
    err = "" if p.returncode == 0 or fails else " | " + " / ".join(l for l in out.splitlines() if l.startswith("error"))[:400]
    return p.returncode, fails, res, err

for name, edits in PLANTS:
    saved = {}
    ok = True
    for path, old, new in edits:
        full = os.path.join(TREE, path); text = open(full).read()
        saved.setdefault(full, text)
        if text.count(old) != 1:
            print(f"{name}: ANCHOR found {text.count(old)} times in {path}: {old[:60]!r}", flush=True); ok = False; break
        open(full, "w").write(text.replace(old, new))
    try:
        if ok:
            for t in ALL:
                rc, fails, res, err = run(t)
                print(f"{name} [{' '.join(t[2:6])}]: rc {rc}; failed: {fails or 'nothing'}; {res}{err}", flush=True)
    finally:
        for full, text in saved.items():
            open(full, "w").write(text)
r = subprocess.run(["git", "status", "--short"], cwd=TREE, capture_output=True, text=True).stdout
print("worktree:", r.strip() or "clean")
