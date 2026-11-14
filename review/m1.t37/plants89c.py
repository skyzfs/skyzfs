#!/usr/bin/env python3
"""Plants for #89 at 54b419f, each in its own copy of tests/, in parallel."""
import os, shutil, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
TREE = "/home/user/skyzfs-pr89"
BASE = sys.argv[1]
RIG = "tests/kernel/daemon-rig.sh"
FETCH = ["bash", "tests/kernel/test_daemon_fetch.sh"]
CHECK = ["python3", "tests/kernel/test_daemon_check.py"]
PLANTS = [
    ("Q0 none", [], [FETCH, CHECK]),
    ("Q1 start: a hang is the rig's error again", [(RIG,
        '        vol_detach "$SKYZFSD" "$dir"; return 1\n    fi\n    said=$(tail -n 1',
        '        vol_detach "$SKYZFSD" "$dir"; return 2\n    fi\n    said=$(tail -n 1')], [FETCH]),
    ("Q2 take-over: a hang is the rig's error again", [(RIG,
        '        rf9_cleanup; return 1\n    fi\n    said=$(tail -n 1',
        '        rf9_cleanup; return 2\n    fi\n    said=$(tail -n 1')], [FETCH]),
    ("Q3 start: the hook's device not checked", [(RIG,
        '        && [ -n "$n" ] && [ "${hn:-}" = "$n" ] \\\n', '        && [ -n "$n" ] \\\n')], [FETCH]),
    ("Q4 take-over: the hook's device not checked", [(RIG,
        '        && [ "${hn:-}" = "$n" ] && [[ $said == *"device $n is left waiting', '        && [[ $said == *"device $n is left waiting')], [FETCH]),
    ("Q5 the cleanup's serve removed", [(RIG,
        '    if [ -z "$SERVE_PID" ] && [ "$(ublk_state "$bdev")" = QUIESCED ]; then serve_start "$SKYZFSD" "$dir" > /dev/null 2>&1; fi\n', '')], [FETCH]),
    ("Q6 take-over hang: the killed serve not waited for", [(RIG,
        '        if [ -n "$SERVE_PID" ]; then kill -9 "$SERVE_PID" 2>/dev/null; serve_wait 30 || RCT_WHY+="; it did not end in 30 s after SIGKILL"; fi\n        rf9_cleanup; return 1',
        '        [ -z "$SERVE_PID" ] || kill -9 "$SERVE_PID" 2>/dev/null\n        SERVE_PID=""; rf9_cleanup; return 1')], [FETCH]),
]
def one(plant):
    name, edits, tests = plant
    d = os.path.join(BASE, name.split()[0])
    shutil.rmtree(d, ignore_errors=True)
    shutil.copytree(os.path.join(TREE, "tests"), os.path.join(d, "tests"))
    for path, old, new in edits:
        full = os.path.join(d, path); text = open(full).read()
        if text.count(old) != 1:
            return f"{name}: ANCHOR found {text.count(old)} times"
        open(full, "w").write(text.replace(old, new))
    out = []
    for t in tests:
        p = subprocess.run(t, cwd=d, capture_output=True, text=True)
        o = p.stdout + p.stderr
        if t is CHECK:
            fails = [l[:150] for l in o.splitlines() if l.startswith(("FAIL:", "ERROR:"))]
            last = [l for l in o.splitlines() if l.startswith(("OK", "FAILED", "Ran "))]
        else:
            fails = [l[:150] for l in o.splitlines() if l.startswith("FAIL")]
            last = [l for l in o.splitlines() if l.startswith("all passed") or l.endswith("failed")][-1:]
            oks = sum(1 for l in o.splitlines() if l.startswith("ok"))
            last.append(f"{oks} ok")
        out.append(f"  [{' '.join(t[1:])}] rc {p.returncode}; {last}; {fails if fails else 'no FAIL line'}")
    return name + "\n" + "\n".join(out)
with ThreadPoolExecutor(max_workers=8) as ex:
    for r in ex.map(one, PLANTS):
        print(r, flush=True)
