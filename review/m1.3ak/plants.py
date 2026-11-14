#!/usr/bin/env python3
"""Plants for #91 at 6bc687f: one at a time in the worktree, each under a time limit, then restored."""
import os, subprocess
TREE = "/home/user/skyzfs-pr91"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
SERVE = "bins/skyzfsd/src/serve.rs"
T = ["timeout", "240", "cargo", "test", "-p", "skyzfsd", "--features", "test-hooks", "--lib", "serve::tests"]
LEFT = "        let left = self.deadline.saturating_duration_since(Instant::now());\n"
FIRSTSEND = ("        thread_local! { static FIRST: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) }; }\n"
             "        let first = FIRST.with(|f| { if f.get().is_none() { f.set(Some(Instant::now())); } f.get().unwrap_or_else(Instant::now) });\n"
             "        let left = (first + self.limit).saturating_duration_since(Instant::now());\n")
PLANTS = [
    ("U0 none", []),
    ("U1a each command waits the whole limit (recv_timeout(limit)), the not-sent check kept", [(SERVE, "        match answer.recv_timeout(left) {", "        match answer.recv_timeout(self.limit) {")]),
    ("U1b a deadline per command (let left = self.limit)", [(SERVE, LEFT, "        let left = self.limit;\n")]),
    ("U2 a second deadline for the failure's way after the stop", [(SERVE, "    let kept = stop();\n    if failed() {\n", "    let kept = stop();\n    kernel.deadline = Instant::now() + limit;\n    if failed() {\n")]),
    ("U3 commands sent after the deadline", [(SERVE, "        if left.is_zero() {\n", "        if left.is_zero() && false {\n")]),
    ("U4 the deadline counted from the first command sent, not from new", [(SERVE, LEFT, FIRSTSEND)]),
    ("U5 the open not bounded", [(SERVE, "        self.send(number, &name, move |k| k.client_holds(number))\n            .ok()\n            .flatten()\n", "        let _ = name;\n        self.inner.client_holds(number)\n")]),
    ("U6 the info not bounded", [(SERVE, "        self.send(number, \"GET_DEV_INFO and GET_PARAMS\", move |k| {\n            k.info(number)\n        })?\n", "        self.inner.info(number)\n")]),
    ("U7 a delete with no answer reads as done", [(SERVE, "        self.send(number, \"DEL_DEV_ASYNC\", move |k| k.delete(number))?\n", "        self.send(number, \"DEL_DEV_ASYNC\", move |k| k.delete(number)).unwrap_or(Ok(()))\n")]),
    ("U8 the answer waited for with no limit", [(SERVE, "        match answer.recv_timeout(left) {", "        let _ = left;\n        match answer.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected) {")]),
]

def run(cmd):
    p = subprocess.run(cmd, cwd=TREE, env=ENV, capture_output=True, text=True)
    out = p.stdout + p.stderr
    fails = sorted({l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.rstrip().endswith("FAILED")})
    res = [l.split(";")[0][13:] + ";" + l.split(";")[1] for l in out.splitlines() if l.startswith("test result:")]
    why = [l.strip()[:220] for l in out.splitlines() if "panicked at" in l or ("took" in l and "limit" in l) or l.strip().startswith(("left:", "right:"))][:8]
    hung = [l for l in out.splitlines() if "has been running for over" in l][:3]
    err = [l for l in out.splitlines() if l.startswith("error")][:3]
    return p.returncode, fails, res, why, hung, err

for name, edits in PLANTS:
    saved = {}
    ok = True
    for path, old, new in edits:
        full = os.path.join(TREE, path); text = saved.get(full) or open(full).read()
        cur = open(full).read()
        saved.setdefault(full, text)
        if cur.count(old) != 1:
            print(f"{name}: ANCHOR found {cur.count(old)} times", flush=True); ok = False; break
        open(full, "w").write(cur.replace(old, new))
    try:
        if ok:
            rc, fails, res, why, hung, err = run(T)
            print(f"{name}: rc {rc}; failed: {fails or 'nothing'}; {res}", flush=True)
            for l in hung + why + err:
                print("    " + l, flush=True)
    finally:
        for full, text in saved.items():
            open(full, "w").write(text)
r = subprocess.run(["git", "status", "--short"], cwd=TREE, capture_output=True, text=True).stdout
print("worktree:", r.strip() or "clean")
