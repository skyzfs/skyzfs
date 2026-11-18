"""plants2.py COPY: the plants of the two commits after 405b7ed (the seed's watcher,
the v17 columns, bound and boundsparse), one at a time in COPY (a copy of the head's
tests and workflows), each followed by the pull request's own tests of that part.
KILLED: a test failed. SURVIVED: none did. NOT APPLIED: the text was not found
exactly once. The file is restored after each plant (try/finally)."""
import subprocess, sys
copy = sys.argv[1]
SH, CR = 'tests/bench/topology-bench.sh', 'tests/kernel/cut-record.sh'
CK = 'tests/kernel/cut_daemon_check.py'
BENCH = {'copy_limit': ['bash', 'tests/bench/test_copy_limit.sh'],
         'embedded': ['python3', 'tests/bench/test_embedded_python.py']}
CUT = {'cut_check': ['python3', 'tests/kernel/test_cut_daemon_check.py'],
       'cut_record': ['bash', 'tests/kernel/test_cut_record.sh']}
def run(cmd):
    try:
        return subprocess.run(cmd, cwd=copy, capture_output=True, text=True, timeout=900).returncode
    except subprocess.TimeoutExpired:
        return 'TIMEOUT'
def tests(which):
    return {k: run(c) for k, c in which.items()}
base = tests({**BENCH, **CUT})
assert all(v == 0 for v in base.values()), base
P = [
 ('S1 the watcher is not started', SH, BENCH,
  '        ( SKZ_UPLOAD_WAIT_S=3600; skz_first_upload under ) &\n', '        :\n'),
 ('S2 the watcher starts without the sync', SH, BENCH,
  '        # The seed under the workload: watched, not waited for.\n        lim 120 zpool sync "$ZFS_POOL"\n',
  '        # The seed under the workload: watched, not waited for.\n'),
 ('S3 the teardown does not stop the watcher', SH, BENCH,
  '        kill "$SKZ_SEED_WATCH" 2>/dev/null; SKZ_SEED_WATCH=""\n', '        SKZ_SEED_WATCH=""\n'),
 ('S4 the watcher\'s line does not say "under the workload"', SH, BENCH,
  '[ "${1:-}" != under ] || how=" under the workload"', 'true'),
 ('S5 the watcher on every daemon row, uploads or not', SH, BENCH,
  '    elif [ "$1" = skyzfsd ] && [ "$SKZ_UPLOAD" = 1 ]; then', '    elif [ "$1" = skyzfsd ]; then'),
 ('S6 the watcher gives up after 60 s', SH, BENCH,
  '( SKZ_UPLOAD_WAIT_S=3600; skz_first_upload under ) &', '( SKZ_UPLOAD_WAIT_S=60; skz_first_upload under ) &'),
 ('V1 zero blocks: the end value, not the change', SH, BENCH,
  "{d('upload_zero_blocks')}", "{sa.get('upload_zero_blocks', 'n/a')}"),
 ('V2 holes: the start value', SH, BENCH,
  "{sa.get('cut_holes', 'n/a')} ({sa.get('cut_hole_chunks', 'n/a')})",
  "{sb.get('cut_holes', 'n/a')} ({sa.get('cut_hole_chunks', 'n/a')})"),
 ('V3 hole chunks: the change', SH, BENCH,
  "({sa.get('cut_hole_chunks', 'n/a')})", "({d('cut_hole_chunks')})"),
 ('K1 boundsparse: the copies not asked', CK, CUT,
  "            ok_cp = cb == '0'\n", "            ok_cp = True\n"),
 ('K2 boundsparse: the abandoned not asked', CK, CUT,
  "            ok_ab = ab is not None and ab.isdigit() and int(ab) == 0\n", "            ok_ab = True\n"),
 ('K3 boundsparse: INFO unless the set holds at the end', CK, CUT,
  '        if holes is None:\n', "        if holes != 'held':\n"),
 ('K4 boundsparse: a v17 daemon that took no holes (none) is asked nothing', CK, CUT,
  '        if holes is None:\n', "        if holes in (None, 'none'):\n"),
 ('K5 bound: the abandoned not asked', CK, CUT,
  "        ok_ab = ab is not None and ab.isdigit() and int(ab) >= 1\n", "        ok_ab = True\n"),
 ('K6 bound on a sparse volume (no --zero-fill)', CR, CUT,
  '        bound) CREATE_ARGS=(--zero-fill) ;;\n', ''),
 ('K7 boundsparse zero-filled too', CR, CUT,
  '        bound) CREATE_ARGS=(--zero-fill) ;;\n', '        bound|boundsparse) CREATE_ARGS=(--zero-fill) ;;\n'),
 ('K8 the create does not take the course\'s arguments', CR, CUT,
  '--run-dir "$RUN" "${CREATE_ARGS[@]}" "${place[@]}"', '--run-dir "$RUN" "${place[@]}"'),
]
for name, path, which, old, new in P:
    full = f'{copy}/{path}'
    text = open(full).read()
    if text.count(old) != 1:
        print(f'{name}: NOT APPLIED ({text.count(old)} matches)', flush=True)
        continue
    try:
        open(full, 'w').write(text.replace(old, new))
        r = tests(which)
    finally:
        open(full, 'w').write(text)
    died = [k for k, v in r.items() if v != 0]
    print(f"{name}: {'KILLED by ' + ', '.join(died) if died else 'SURVIVED'}", flush=True)
