"""Plant one change at a time in #85's rig, run test_daemon_refused.sh, restore."""
import os, subprocess, sys
W = "/home/user/skyzfs-pr85"
R = "tests/kernel/daemon-rig.sh"
PLANTS = [
    ("R1 the client's exit not checked", [('&& [ "$crc" = 0 ] ', '')]),
    ("R2 the next stop's exit not checked", [('&& [ "$stop" = 0 ] ', '')]),
    ("R3 no write before the flush", [('if [ "$kind" != write ] && ! lim 30', 'if [ "$kind" = read ] && ! lim 30')]),
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
        r = subprocess.run(["bash", "tests/kernel/test_daemon_refused.sh"], cwd=W, capture_output=True, text=True)
        fails = [l for l in r.stdout.splitlines() if l.startswith("FAIL")]
        print(f"{name}: rc {r.returncode}; " + ("; ".join(fails) if fails else "nothing fails"))
    finally:
        subprocess.run(["git", "checkout", "--", R], cwd=W)
print("worktree clean:", subprocess.run(["git", "status", "--short"], cwd=W, capture_output=True, text=True).stdout.strip() == "")
