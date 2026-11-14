import subprocess, sys
TREE="/home/user/skyzfs-pr85"; RIG=TREE+"/tests/kernel/control-rig.sh"
P=[("G1: the fetch's word not read again at the end",
    "    later=$(rf_line refuse-fetch 2)\n    if [ \"$later\" != \"$said\" ]; then RF_SAID+=\"; by the program's end it said of the fetch: '${later:-nothing}'\"; said=$later; fi\n", ""),
   ("G2: the last line cut at 120 again", "| tail -n 1 | cut -c1-400)'\"", "| tail -n 1 | cut -c1-120)'\""),
   ("G3: the late word recorded but the verdict on the first", "said=$later; fi\n", "fi\n")]
for name, old, new in P:
    s=open(RIG).read()
    if s.count(old)!=1: print(name, "ANCHOR"); continue
    open(RIG,"w").write(s.replace(old,new))
    p=subprocess.run(["bash","tests/kernel/test_control_fetch.sh"],cwd=TREE,capture_output=True,text=True)
    fails=[l for l in p.stdout.splitlines() if l.startswith("FAIL")]
    print(f"{name}: rc {p.returncode}; " + ("; ".join(f[:110] for f in fails) if fails else "nothing fails"), flush=True)
    subprocess.run(["git","checkout","--","tests/kernel/control-rig.sh"],cwd=TREE)
print("clean:", subprocess.run(["git","status","--short"],cwd=TREE,capture_output=True,text=True).stdout.strip()=="")
