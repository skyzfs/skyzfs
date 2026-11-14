#!/usr/bin/env python3
"""Gates and plants for #87 at 8387402."""
import os, subprocess
TREE="/home/user/skyzfs-pr87"
ENV=dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
SERVE="bins/skyzfsd/src/serve.rs"
T=["cargo","test","-p","skyzfsd","--features","test-hooks","--lib","serve::tests"]
GATES=[["cargo","fmt","--all","--check"],
       ["cargo","clippy","-q","-p","skyzfsd","-p","skyzfs-ublk-sys","--all-targets","--","-D","warnings"],
       ["cargo","clippy","-q","-p","skyzfsd","--features","test-hooks","--all-targets","--","-D","warnings"],
       ["cargo","clippy","-q","-p","skyzfs-ublk-sys","--features","control-test-program","--all-targets","--","-D","warnings"],
       ["cargo","test","-q","-p","skyzfsd","--lib"],
       ["cargo","test","-q","-p","skyzfsd","--features","test-hooks","--lib"],
       ["cargo","test","-q","-p","skyzfs-ublk-sys","--features","control-test-program"]]
P=[
 ("S1: commands sent after the deadline", [(SERVE,
   "        if left.is_zero() {\n", "        if left.is_zero() && false {\n")]),
 ("S2: the delete not bounded", [(SERVE,
   "        self.send(number, \"DEL_DEV_ASYNC\", move |k| k.delete(number))?\n", "        self.inner.delete(number)\n")]),
 ("S3: the client's hold not bounded", [(SERVE,
   "        let name = format!(\"the exclusive open of {}\", block_device(number));\n        self.send(number, &name, move |k| k.client_holds(number))\n            .ok()\n            .flatten()\n",
   "        self.inner.client_holds(number)\n")]),
 ("S4: each command waited for the whole limit, not until the deadline", [(SERVE,
   "        match answer.recv_timeout(left) {", "        match answer.recv_timeout(self.limit) {")]),
 ("S5: a new deadline for each command", [(SERVE,
   "        let left = self.deadline.saturating_duration_since(Instant::now());",
   "        let left = (Instant::now() + self.limit).saturating_duration_since(Instant::now());")]),
 ("S6: give_up_unanswered does not wrap the kernel", [(SERVE,
   "    let kernel = &mut Bounded::new(kernel, commands, limit);\n", "    let _ = (commands, limit);\n")]),
 ("S7: a delete with no answer reads as done", [(SERVE,
   "        self.send(number, \"DEL_DEV_ASYNC\", move |k| k.delete(number))?\n",
   "        self.send(number, \"DEL_DEV_ASYNC\", move |k| k.delete(number)).unwrap_or(Ok(()))\n")]),
]
def run(cmd):
    p=subprocess.run(cmd,cwd=TREE,env=ENV,capture_output=True,text=True); out=p.stdout+p.stderr
    fails=sorted({l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.rstrip().endswith("FAILED")})
    res=[l[13:50] for l in out.splitlines() if l.startswith("test result:")]
    errs=[l for l in out.splitlines() if l.startswith("error")][:2]
    return p.returncode, fails, res, errs
for g in GATES:
    rc,f,res,errs=run(g); print("gate", " ".join(g[1:]), "rc", rc, res, errs, flush=True)
for name, edits in P:
    ok=True
    for path,old,new in edits:
        full=os.path.join(TREE,path); s=open(full).read()
        if s.count(old)!=1: print(name,"ANCHOR NOT FOUND",path); ok=False; break
        open(full,"w").write(s.replace(old,new))
    if ok:
        rc,f,res,errs=run(T); print(f"{name}: rc {rc}; failed: {f or 'nothing'} {res} {errs}", flush=True)
    subprocess.run(["git","checkout","--"]+sorted({e[0] for e in edits}),cwd=TREE)
print("clean:", subprocess.run(["git","status","--short"],cwd=TREE,capture_output=True,text=True).stdout.strip()=="")
