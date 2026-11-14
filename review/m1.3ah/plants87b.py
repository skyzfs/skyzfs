#!/usr/bin/env python3
"""Gates and plants for #87 at a6239e6."""
import os, subprocess
TREE="/home/user/skyzfs-pr87"
ENV=dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
SERVE="bins/skyzfsd/src/serve.rs"; LIFE="crates/skyzfs-ublk/src/lifecycle.rs"
T=["cargo","test","-q","-p","skyzfsd","--features","test-hooks","--lib","serve::tests"]
GATES=[["cargo","fmt","--all","--check"],
       ["cargo","clippy","-q","-p","skyzfsd","--all-targets","--","-D","warnings"],
       ["cargo","clippy","-q","-p","skyzfsd","--features","test-hooks","--all-targets","--","-D","warnings"],
       ["cargo","test","-q","-p","skyzfsd","--lib"],
       ["cargo","test","-q","-p","skyzfsd","--features","test-hooks","--lib"]]
FN_START="    stop: impl FnOnce() -> Option<S>,\n) -> (Exit, Option<S>) {\n"
P=[
 ("R1: no look at the failure before the delete", [(SERVE,
   FN_START+"    if failed() {\n        let exit = failed_at_end(kernel, records, number, identity);\n        return (exit, stop());\n    }\n", FN_START)]),
 ("R2: no look at the failure after the stop", [(SERVE,
   "    let kept = stop();\n    if failed() {\n        return (failed_at_end(kernel, records, number, identity), kept);\n    }\n",
   "    let kept = stop();\n")]),
 ("R3: the stop before the delete (the old order)", [(SERVE,
   "    let deleted = give_up_start(kernel, records, number, took_over);\n    let kept = stop();\n",
   "    let kept = stop();\n    let deleted = give_up_start(kernel, records, number, took_over);\n")]),
 ("R4: the store dropped before the line", [(SERVE,
   "    let deleted = give_up_start(kernel, records, number, took_over);\n    let kept = stop();\n",
   "    let deleted = give_up_start(kernel, records, number, took_over);\n    drop(stop());\n    let kept = None;\n")]),
 ("R5: a failure first, but the queue never stopped", [(SERVE,
   "        let exit = failed_at_end(kernel, records, number, identity);\n        return (exit, stop());\n",
   "        let exit = failed_at_end(kernel, records, number, identity);\n        let _ = stop;\n        return (exit, None);\n")]),
 ("R6: delete_unstarted deletes before the note (Q8)", [(LIFE,
   "    number: u32,\n) -> Result<(), Undeleted> {\n    delete(control, records, number, Mutants::default())\n}\n\npub(crate) fn fail_serving_with",
   "    number: u32,\n) -> Result<(), Undeleted> {\n    delete(control, records, number, Mutants(Mutants::DELETE_BEFORE_NOTE))\n}\n\npub(crate) fn fail_serving_with")]),
 ("R7: run() does not stop the queue in the no-answer branch", [(SERVE,
   "                || {\n                    stopper.stop();\n                    for _ in 0..ANSWER_LOOKS {",
   "                || {\n                    for _ in 0..ANSWER_LOOKS {")]),
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
