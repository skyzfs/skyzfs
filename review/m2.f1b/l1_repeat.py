#!/usr/bin/env python3
"""Repeat runs of L1's test from the driver binaries l1_build.py made (the
head, and B1H planted), idle and beside 8 and 16 busy loops; and the head's
whole driver binary (its 25 tests in parallel) beside 16.
Usage: l1_repeat.py BINDIR CRATE_DIR OUT HEAD_RUNS B1H_RUNS WHOLE_RUNS"""
import json, os, re, subprocess, sys, time

BINDIR, CRATE, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
HEAD_RUNS, B1H_RUNS, WHOLE_RUNS = map(int, sys.argv[4:7])
NEW = 'head_swapped_before_the_first_cut_is_judged_by_the_group_the_engine_found'


def busy(n):
    return [subprocess.Popen(['bash', '-c', 'while :; do :; done']) for _ in range(n)]


def once(binary, args):
    try:
        p = subprocess.run([binary] + args, cwd=CRATE, capture_output=True, text=True, timeout=300,
                           env=dict(os.environ, RUST_BACKTRACE='0'))
    except subprocess.TimeoutExpired:
        return 'timeout', ''
    out = p.stdout + p.stderr
    m = re.search(r'^test result: \w+\. (\d+) passed; (\d+) failed', out, re.M)
    return p.returncode, (m.group(0) if m else out[-200:])


res = []
for load in (0, 8, 16):
    loops = busy(load)
    try:
        time.sleep(2)
        plan = [('head, L1 test', 'driver-head', [NEW, '--exact'], HEAD_RUNS),
                ('B1H, L1 test', 'driver-B1H', [NEW, '--exact'], B1H_RUNS)]
        if load == 16:
            plan.append(('head, whole driver binary', 'driver-head', [], WHOLE_RUNS))
        for label, b, args, n in plan:
            la = open('/proc/loadavg').read().split()[0]
            runs = [once(os.path.join(BINDIR, b), args) for _ in range(n)]
            rcs = [r for r, _ in runs]
            entry = {'what': label, 'busy_loops': load, 'loadavg_before': la, 'runs': n,
                     'passed': rcs.count(0), 'failed': rcs.count(101),
                     'other': sum(1 for r in rcs if r not in (0, 101)),
                     'results': sorted(set(t for _, t in runs))}
            res.append(entry)
            print(json.dumps(entry), flush=True)
            json.dump(res, open(OUT, 'w'), indent=1)
    finally:
        for p in loops:
            p.kill()
        for p in loops:
            p.wait()
