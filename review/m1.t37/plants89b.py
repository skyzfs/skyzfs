#!/usr/bin/env python3
"""Plants for #89 at 7b2b083: each in its own copy of tests/, run in parallel, then compared."""
import os, shutil, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

TREE = "/home/user/skyzfs-pr89"
BASE = sys.argv[1]  # a scratch directory for the copies
RIG = "tests/kernel/daemon-rig.sh"
FETCH = ["bash", "tests/kernel/test_daemon_fetch.sh"]
CHECK = ["python3", "tests/kernel/test_daemon_check.py"]

START_OK = ('    [ "$rc" = "$EXIT_START_GIVEN_UP" ] && [[ $said == *"the kernel refused the first fetch of tag 0"* ]] \\\n'
            '        && [[ $said == *"which this serve added and never started, is deleted"* ]] && [[ $said == *"the volume is not failed; $RF9_TAIL" ]] \\\n'
            '        && [ -n "$free" ] && [ "${st:-}" != failed ] && [ "$back" = 0 ] && [ "$stop" = 0 ]\n')
TAKE_OK = ('    [ "$rc" = "$EXIT_START_GIVEN_UP" ] && [[ $said == *"the kernel refused the first fetch of tag 0"* ]] \\\n'
           '        && [[ $said == *"device $n is left waiting for the next server"* ]] && [[ $said == *"the volume is not failed; $RF9_TAIL" ]] \\\n'
           '        && [ "$st" = QUIESCED ] && [ "${vst:-}" != failed ] && [ "$held" = "still held" ] \\\n'
           '        && [ "$wrc" = 0 ] && [ "$pst" = ONLINE ] && [[ $perr == "vdev errors 0 "* ]]\n')

def drop(text, part):
    return (RIG, text, text.replace(part, ""))

PLANTS = [
    ("P0 none", [], [FETCH, CHECK]),
    ("P1 start: no check that the number is free", [drop(START_OK, ' && [ -n "$free" ]')], [FETCH]),
    ("P2 start: no check of the volume's status", [drop(START_OK, ' && [ "${st:-}" != failed ]')], [FETCH]),
    ("P3 start: no check of 'is deleted'", [(RIG, '[[ $said == *"which this serve added and never started, is deleted"* ]]', 'true')], [FETCH]),
    ("P4 start: no check of the next serve's read and stop", [drop(START_OK, ' && [ "$back" = 0 ] && [ "$stop" = 0 ]')], [FETCH]),
    ("P5 start: no check of the tail", [drop(START_OK, ' && [[ $said == *"the volume is not failed; $RF9_TAIL" ]]')], [FETCH]),
    ("P6 start: no check of exit 9", [drop(START_OK, '[ "$rc" = "$EXIT_START_GIVEN_UP" ] && ')], [FETCH]),
    ("P7 take: no check that the I/O was still held", [drop(TAKE_OK, ' && [ "$held" = "still held" ]')], [FETCH]),
    ("P8 take: no check of QUIESCED", [(RIG, '        && [ "$st" = QUIESCED ] && [ "${vst:-}" != failed ]', '        && true && [ "${vst:-}" != failed ]')], [FETCH]),
    ("P9 take: no check of the held I/O's exit", [(RIG, '        && [ "$wrc" = 0 ] && [ "$pst" = ONLINE ]', '        && true && [ "$pst" = ONLINE ]')], [FETCH]),
    ("P10 take: no check of vdev errors", [drop(TAKE_OK, ' && [[ $perr == "vdev errors 0 "* ]]')], [FETCH]),
    ("P11 take: no check of 'left waiting'", [(RIG, '[[ $said == *"device $n is left waiting for the next server"* ]]', 'true')], [FETCH]),
    ("P12 take: no check of the volume's status", [drop(TAKE_OK, ' && [ "${vst:-}" != failed ]')], [FETCH]),
    ("P13 cleanup: no serve for a QUIESCED device", [(RIG,
        '    if [ -z "$SERVE_PID" ] && [ "$(ublk_state "$bdev")" = QUIESCED ]; then serve_start "$SKYZFSD" "$dir" > /dev/null 2>&1; fi\n', '')], [FETCH]),
    ("P14 the kill loop does not read a 9 (both places)", [(RIG,
        'if [ "$SERVE_RC" = "$EXIT_BROKEN_RULE" ] || [ "$SERVE_RC" = "$EXIT_COMMIT_REFUSED" ] || [ "$SERVE_RC" = "$EXIT_START_GIVEN_UP" ]; then',
        'if [ "$SERVE_RC" = "$EXIT_BROKEN_RULE" ] || [ "$SERVE_RC" = "$EXIT_COMMIT_REFUSED" ]; then')], [CHECK]),
    ("P15 kt_refusal has no case for 9", [(RIG,
        '        "$EXIT_START_GIVEN_UP")\n            printf', '        "NO_SUCH_EXIT")\n            printf')], [CHECK]),
]

def one(plant):
    name, edits, tests = plant
    d = os.path.join(BASE, name.split()[0])
    shutil.rmtree(d, ignore_errors=True)
    shutil.copytree(os.path.join(TREE, "tests"), os.path.join(d, "tests"))
    for path, old, new in edits:
        full = os.path.join(d, path)
        text = open(full).read()
        n = text.count(old)
        if n < 1 or (n > 1 and not name.startswith("P14")):
            return f"{name}: ANCHOR found {n} times"
        open(full, "w").write(text.replace(old, new))
    out = []
    for t in tests:
        p = subprocess.run(t, cwd=d, capture_output=True, text=True)
        o = p.stdout + p.stderr
        fails = [l[:150] for l in o.splitlines() if l.startswith("FAIL")]
        if t is CHECK:
            fails = [l[:150] for l in o.splitlines() if l.startswith(("FAIL:", "ERROR:"))]
            last = [l for l in o.splitlines() if l.startswith(("OK", "FAILED", "Ran "))]
        else:
            last = [l for l in o.splitlines() if l.startswith(("all passed",)) or l.endswith("failed")][-1:]
        out.append(f"  [{' '.join(t[1:])}] rc {p.returncode}; {last}; {fails if fails else 'no FAIL line'}")
    return name + "\n" + "\n".join(out)

REDO = ('P3', 'P8', 'P9', 'P11')
with ThreadPoolExecutor(max_workers=8) as ex:
    for r in ex.map(one, [x for x in PLANTS if x[0].split()[0] in REDO]):
        print(r, flush=True)
