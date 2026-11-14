"""Plant one change at a time in #86's control rig, run test_control_fetch.sh, restore."""
import os, subprocess, sys
W = "/home/user/skyzfs-pr85"
R = "tests/kernel/control-rig.sh"
PLANTS = [
    ("F0 none (the test as it is)", []),
    ("F5 the verdict does not look at the sent line", [('if [ "$sent" = "sent on tag 0" ] && [[ $said == refused:* ]]; then', 'if [[ $said == refused:* ]]; then')]),
    ("F7 the late look not bounded (no break on an answer)", [('[ -n "$later" ] && break; sleep 0.1', 'sleep 0.1')]),
]
only = sys.argv[1:]
for name, edits in PLANTS:
    if only and name.split()[0] not in only:
        continue
    p = os.path.join(W, R)
    text = open(p).read(); new = text; bad = False
    for old, rep in edits:
        if new.count(old) != 1:
            print(f"{name}: anchor found {new.count(old)} times"); bad = True; break
        new = new.replace(old, rep)
    if bad:
        continue
    open(p, "w").write(new)
    try:
        r = subprocess.run(["bash", "tests/kernel/test_control_fetch.sh"], cwd=W, capture_output=True, text=True)
        fails = [l for l in r.stdout.splitlines() if l.startswith("FAIL")]
        oks = sum(1 for l in r.stdout.splitlines() if l.startswith("ok"))
        print(f"{name}: rc {r.returncode}; {oks} ok; " + ("; ".join(fails) if fails else "nothing fails"))
    finally:
        subprocess.run(["git", "checkout", "--", R], cwd=W)
print("worktree clean:", subprocess.run(["git", "status", "--short"], cwd=W, capture_output=True, text=True).stdout.strip() == "")
