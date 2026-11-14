import subprocess, sys, shutil
T = sys.argv[1]
S = T + '/crates/skyzfs-ublk/src/status.rs'
M = T + '/tests/bench/skz_time.py'
P = {
 'S1 a pump key renamed in the source': [(S, '"pump_taken_other_max_ns",', '"pump_taken_others_max_ns",')],
 'S2 a pump key added in the source': [(S, '    "pump_taken_other_max_ns",\n];', '    "pump_taken_other_max_ns",\n    "pump_queue_ns",\n];')],
 'S3 a time_ array back in the source': [(S, 'pub const PUMP_KEYS', 'pub const CUT_KEYS: [&str; 1] = [\n    "time_cut_ns",\n];\n\npub const PUMP_KEYS')],
 'S4 the pump keys reordered in the source': [(S, '    "pump_submit_count",\n    "pump_submit_ns",', '    "pump_submit_ns",\n    "pump_submit_count",')],
 'M1 pump_* wins over time_pump_*': [(M, "            times.setdefault('time_' + k, reading[k])", "            times['time_' + k] = reading[k]")],
 'M2 every row has store times': [(M, "    return any(not k.startswith('time_pump_') for k in d)", "    return True")],
 'M3 the pump keys not read': [(M, "        if k in reading:\n            times.setdefault", "        if False:\n            times.setdefault")],
 'M4 the pump table skipped': [(M, "    if usable:\n        out += ['', \"The pump's times", "    if False:\n        out += ['', \"The pump's times")],
}
for name, edits in P.items():
    saved = {}
    for f, old, new in edits:
        saved.setdefault(f, open(f).read())
        t = open(f).read(); assert t.count(old) == 1, (name, old[:40], t.count(old))
        open(f, 'w').write(t.replace(old, new))
    try:
        p = subprocess.run([sys.executable, '-I', T + '/tests/bench/test_skz_time.py'], capture_output=True, text=True)
        fails = sorted({l.split('(')[0].split(': ')[1] for l in p.stderr.splitlines() if l.startswith(('FAIL:', 'ERROR:'))})
        print(f'{name}: rc {p.returncode}; {p.stderr.strip().splitlines()[-1]}; {fails}')
    finally:
        for f, t in saved.items(): open(f, 'w').write(t)
