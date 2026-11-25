#!/usr/bin/env python3
"""Repeat runs of `a_connect_is_made_soon_after_the_backlog_has_room` from
the binaries t1_build.py made (each with the probe line), idle and beside
8 and 16 busy loops. Records each run's exit, tries and lateness.
Usage: t1_repeat.py BINDIR CRATE_DIR OUT HEAD_RUNS PLANT_RUNS [ids]"""
import json, os, re, subprocess, sys, time

BINDIR, CRATE, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
HEAD_RUNS, PLANT_RUNS = int(sys.argv[4]), int(sys.argv[5])
IDS = sys.argv[6:] or ['head', 'CE', 'CE5', 'C400', 'C200', 'C100', 'CF']
TEST = 'control::tests::socket::a_connect_is_made_soon_after_the_backlog_has_room'


def busy(n):
    return [subprocess.Popen(['bash', '-c', 'while :; do :; done']) for _ in range(n)]


def once(binary):
    try:
        p = subprocess.run([binary, TEST, '--exact', '--nocapture'], cwd=CRATE, capture_output=True,
                           text=True, timeout=60, env=dict(os.environ, RUST_BACKTRACE='0'))
        out, rc = p.stdout + p.stderr, p.returncode
    except subprocess.TimeoutExpired:
        return {'rc': 'timeout'}
    m = re.search(r'T1PROBE tries=(\d+) late_us=(\d+)', out)
    r = {'rc': rc}
    if m:
        r['tries'], r['late_ms'] = int(m.group(1)), round(int(m.group(2)) / 1000, 1)
    if rc != 0:
        why = re.search(r"panicked at .*?\n(.*)", out)
        r['why'] = why.group(1)[:160] if why else out[-300:]
    if 'running 1 test' not in out:
        r['rc'] = f'no test ran ({rc})'
    return r


res = []
for load in (0, 8, 16):
    loops = busy(load)
    try:
        time.sleep(2)
        for pid in IDS:
            binary = os.path.join(BINDIR, f'skyzfsd-{pid}')
            n = HEAD_RUNS if pid == 'head' else PLANT_RUNS
            la = open('/proc/loadavg').read().split()[0]
            runs = [once(binary) for _ in range(n)]
            tries = sorted(r['tries'] for r in runs if 'tries' in r)
            late = sorted(r['late_ms'] for r in runs if 'late_ms' in r)
            entry = {'id': pid, 'busy_loops': load, 'loadavg_before': la, 'runs': n,
                     'passed': sum(1 for r in runs if r['rc'] == 0),
                     'failed': sum(1 for r in runs if r['rc'] == 101),
                     'other': sum(1 for r in runs if r['rc'] not in (0, 101)),
                     'tries_min_max': [tries[0], tries[-1]] if tries else None,
                     'tries_hist': {t: tries.count(t) for t in sorted(set(tries))},
                     'late_ms_min_median_max': [late[0], late[len(late) // 2], late[-1]] if late else None,
                     'failure_reasons': sorted({('tries<20' if r.get('tries', 99) < 20 else '') + (' late>=200' if r.get('late_ms', 0) >= 200 else '') for r in runs if r['rc'] == 101}),
                     'first_failure': next((r.get('why') for r in runs if r['rc'] != 0), None)}
            res.append(entry)
            print(json.dumps(entry), flush=True)
            json.dump(res, open(OUT, 'w'), indent=1)
    finally:
        for p in loops:
            p.kill()
        for p in loops:
            p.wait()
