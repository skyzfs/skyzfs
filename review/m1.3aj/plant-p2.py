#!/usr/bin/env python3
"""P2 for #90 at 8dfe6e1: set_origin converges through the handle it began with. Then restored."""
import os, subprocess
TREE = "/home/user/skyzfs-pr90"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
LIB = os.path.join(TREE, "crates/skynbd-api/src/lib.rs")
orig = open(LIB).read()
call = "let Some(current) = converge_origin(&state, &vol).await? else {"
assert orig.count(call) == 2, orig.count(call)
anchor = "fn origin_binding(vol: &str, h: &VolumeHandle) -> VolumeBinding {"
assert orig.count(anchor) == 1
extra = ("async fn converge_origin_with(state: &ApiState, vol: &str, h: &VolumeHandle)"
         " -> std::result::Result<Option<VolumeHandle>, ApiError> {\n"
         "    skynbd_nbd::origin::reconcile(&state.registry, &origin_binding(vol, h)).await?;\n"
         "    Ok((!h.term.ended()).then_some(h.clone()))\n}\n\n")
text = orig.replace(call, "let Some(current) = converge_origin_with(&state, &vol, &h).await? else {").replace(anchor, extra + anchor)
open(LIB, "w").write(text)
try:
    for t in (["--test", "origin_serve"], ["--test", "api"]):
        p = subprocess.run(["cargo", "test", "-p", "skynbd-api", *t], cwd=TREE, env=ENV, capture_output=True, text=True)
        out = p.stdout + p.stderr
        fails = sorted({l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.rstrip().endswith("FAILED")})
        res = [l.split(";")[0][13:] for l in out.splitlines() if l.startswith("test result:")]
        errs = [l for l in out.splitlines() if l.startswith("error")][:3]
        print(f"P2 {t[1]}: rc {p.returncode}; failed: {fails or 'nothing'}; {res} {errs if not fails and p.returncode else ''}", flush=True)
finally:
    open(LIB, "w").write(orig)
print("restored:", open(LIB).read() == orig)
