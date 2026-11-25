#!/usr/bin/env python3
"""T1 of #138 (029a48e): build skyzfsd's library test binary for the head
and for each plant, each with a probe line in the connect test that
prints its tries and how late it connected; each plant alone, textual,
each site found exactly once, restored in a finally. Then run each
binary's control::tests::socket module once and record which tests fail.
Usage: t1_build.py TREE BINDIR OUT [ids]"""
import json, os, re, shutil, subprocess, sys, time

TREE, BINDIR, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
ONLY = set(sys.argv[4:])
CTL = 'bins/skyzfsd/src/control.rs'
TST = 'bins/skyzfsd/src/control/tests.rs'
CAP = 'const CONNECT_PAUSE: Duration = Duration::from_millis(50);\n'
GROW = '                pause = (pause * 2).min(CONNECT_PAUSE);\n'
FIRST = '    let mut pause = Duration::from_millis(1);\n'
LATE = '        let late = connected.saturating_duration_since(made);\n'
PROBE = LATE + '        eprintln!("T1PROBE tries={tries} late_us={}", late.as_micros());\n'
P = [
    ('head', 'no plant (probe only)', []),
    ('CE', 'the pause keeps doubling, no cap', [(CTL, GROW, '                pause = pause * 2;\n')]),
    ('CE5', 'the pause capped at 500 ms', [(CTL, CAP, CAP.replace('(50)', '(500)'))]),
    ('C400', 'the pause capped at 400 ms', [(CTL, CAP, CAP.replace('(50)', '(400)'))]),
    ('C200', 'the pause capped at 200 ms', [(CTL, CAP, CAP.replace('(50)', '(200)'))]),
    ('C100', 'the pause capped at 100 ms', [(CTL, CAP, CAP.replace('(50)', '(100)'))]),
    ('CF', 'the first pause 50 ms (no growth below the cap)', [(CTL, FIRST, '    let mut pause = Duration::from_millis(50);\n')]),
]
ENV = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0', RUST_BACKTRACE='0')
res = []
for pid, what, edits in P:
    if ONLY and pid not in ONLY:
        continue
    origs = {}
    entry = {'id': pid, 'what': what}
    try:
        bad = None
        for path, old, new in [(TST, LATE, PROBE)] + edits:
            full = os.path.join(TREE, path)
            origs.setdefault(full, open(full).read())
            text = open(full).read()
            if text.count(old) != 1:
                bad = f'NOT APPLIED (site found {text.count(old)} times in {path})'
                break
            open(full, 'w').write(text.replace(old, new))
        if bad:
            entry['verdict'] = bad
        else:
            p = subprocess.run(['cargo', 'test', '-p', 'skyzfsd', '--lib', '--no-run', '--message-format=json'],
                               cwd=TREE, capture_output=True, text=True, timeout=3600, env=ENV)
            exe = None
            for line in p.stdout.splitlines():
                try:
                    m = json.loads(line)
                except ValueError:
                    continue
                if m.get('reason') == 'compiler-artifact' and m.get('executable') and m['target']['name'] == 'skyzfsd' and m['profile']['test']:
                    exe = m['executable']
            if p.returncode != 0 or not exe:
                entry['verdict'] = 'NOT APPLIED (does not build)'
                open(os.path.join(BINDIR, f'{pid}.err'), 'w').write(p.stderr[-6000:])
            else:
                dst = os.path.join(BINDIR, f'skyzfsd-{pid}')
                shutil.copy2(exe, dst)
                entry['bin'] = os.path.basename(dst)
                t0 = time.time()
                q = subprocess.run([dst, 'control::tests::socket', '--nocapture'], cwd=os.path.join(TREE, 'bins/skyzfsd'),
                                   capture_output=True, text=True, timeout=600, env=ENV)
                out = q.stdout + q.stderr
                entry['socket_rc'] = q.returncode
                entry['socket_failed'] = sorted(set(re.findall(r'^    (control::tests::socket::\S+)$', out, re.M)))
                entry['socket_result'] = (re.findall(r'^test result: .*$', out, re.M) or ['?'])[-1]
                entry['probe'] = re.findall(r'T1PROBE .*', out)
                entry['secs'] = round(time.time() - t0)
    finally:
        for full, text in origs.items():
            open(full, 'w').write(text)
    res.append(entry)
    print(json.dumps(entry), flush=True)
    json.dump(res, open(OUT, 'w'), indent=1)
