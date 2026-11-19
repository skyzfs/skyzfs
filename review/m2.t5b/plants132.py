"""plants132.py TREE [FIRST [LAST]]: plants on #132's code, one at a time in TREE (a worktree at
27fe831). A plant of topology-bench.sh is followed by test_copy_limit.sh, test_embedded_python.py
and tests/lib/test_skz_store.sh (as root); of bracket.py by test_bracket.py and
test_embedded_python.py. KILLED: a test failed (TIMEOUT counts). SURVIVED: none did. NOT
APPLIED: the old text was not found exactly once. The file is restored after each plant
(try/finally), every test runs under a 600 s limit, and the tree is checked clean at the end.
Run with python3 -I."""
import subprocess
import sys

tree = sys.argv[1]
SH, BK = 'tests/bench/topology-bench.sh', 'tests/bench/bracket.py'
T_COPY = ('copy_limit', ['bash', 'tests/bench/test_copy_limit.sh'])
T_EMB = ('embedded', ['python3', 'tests/bench/test_embedded_python.py'])
T_STORE = ('skz_store', ['bash', 'tests/lib/test_skz_store.sh'])
T_BRK = ('bracket', ['python3', 'tests/bench/test_bracket.py'])
TESTS = {SH: [T_COPY, T_EMB, T_STORE], BK: [T_BRK, T_EMB]}


def run(cmd):
    try:
        return subprocess.run(cmd, cwd=tree, capture_output=True, text=True, timeout=600).returncode
    except subprocess.TimeoutExpired:
        return 'TIMEOUT'


WIN_RESUME = '    [ "$SKZ_UPLOAD_FROM_LOAD" != window ] || skz_minio_resume\n'
BEFORE = '    snapshot "$OUT/$tag.before"\n'
P = [
    # (1) the window mode in topology-bench.sh (first head)
    ('W1 =window let go at the first warm-up as well (the mark\'s guard dropped)', SH,
     '    [ "$SKZ_UPLOAD_FROM_LOAD" = window ] || skz_minio_resume\n', '    skz_minio_resume\n'),
    ('W2 =window let go before the window\'s first status read', SH,
     BEFORE, WIN_RESUME + BEFORE),  # and the later one removed below
    ('W3 =window never let go in run_workload (only at the teardown)', SH, WIN_RESUME, ''),
    ('W4 =window let go after the window\'s pgbench', SH,
     WIN_RESUME + '    log "$tag: measuring ${DURATION}s',
     '    log "$tag: measuring ${DURATION}s'),  # and put after pgbench below
    ('W5 the release time written for =1 too', SH,
     '    [ "$SKZ_UPLOAD_FROM_LOAD" != window ] || date +%s > "$SKZ_SEED_RELEASED"\n',
     '    date +%s > "$SKZ_SEED_RELEASED"\n'),
    ('W6 the release time kept when the unpause fails', SH, '        rm -f "$SKZ_SEED_RELEASED"\n', ''),
    ('W7 the release time written after the unpause', SH,
     '    [ "$SKZ_UPLOAD_FROM_LOAD" != window ] || date +%s > "$SKZ_SEED_RELEASED"\n    if skz_minio unpause; then\n',
     '    if skz_minio unpause; then\n        [ "$SKZ_UPLOAD_FROM_LOAD" != window ] || date +%s > "$SKZ_SEED_RELEASED"\n'),
    ('W8 the last row\'s release time not removed at the build', SH,
     '        rm -f "$SKZ_SEED_BEGAN" "$SKZ_SEED_RELEASED"\n', '        rm -f "$SKZ_SEED_BEGAN"\n'),
    ('W9 the watcher reads the release time without watch', SH,
     'if [ "${1:-}" = watch ] && [[ $rel =~ ^[0-9]+$ ]]', 'if [[ $rel =~ ^[0-9]+$ ]]'),
    ('W10 the lower end not floored at 0', SH, '$(( prev > rel ? prev - rel : 0 ))', '$(( prev - rel ))'),
    ('W11 the lower end always 0 (the read before ignored)', SH, '$(( prev > rel ? prev - rel : 0 ))', '0'),
    ('W12 the upper end from the read before', SH, 'to $(( now - rel )) s after MinIO', 'to $(( prev - rel )) s after MinIO'),
    ('W13 the release time not asked to be before the read that saw it', SH,
     ' && [ "$now" -ge "$rel" ]; then\n                how="$how; ', '; then\n                how="$how; '),
    ('W14 a release time that is not a number taken', SH, '[[ $rel =~ ^[0-9]+$ ]]', '[ -n "$rel" ]'),
    ('W15 =window refused (only 1 taken)', SH,
     '[ "$SKZ_UPLOAD_FROM_LOAD" = 1 ] || [ "$SKZ_UPLOAD_FROM_LOAD" = window ] \\',
     '[ "$SKZ_UPLOAD_FROM_LOAD" = 1 ] || [ "$SKZ_UPLOAD_FROM_LOAD" = windo ] \\'),
    ('W16 =window lets a wait over 0 through', SH,
     '    [ "$SKZ_UPLOAD_WAIT_S" = 0 ] || {', '    [ "$SKZ_UPLOAD_WAIT_S" = 0 ] || [ "$SKZ_UPLOAD_FROM_LOAD" = window ] || {'),
    ('W17 the record says 1 for =window', SH, '    echo "upload_from_load $SKZ_UPLOAD_FROM_LOAD" >> "$OUT/$TOPO.skzinfo"\n',
     '    echo "upload_from_load 1" >> "$OUT/$TOPO.skzinfo"\n'),
    ('W18 the row reads window as the workload mode', SH,
     "u['from_load'] = 'window' if info['upload_from_load'] == 'window' else 'workload'", "u['from_load'] = 'workload'"),
    ('W19 the log names the workload for =window', SH,
     'if [ "$SKZ_UPLOAD_FROM_LOAD" = window ]; then echo "the first window begins"',
     'if [ "$SKZ_UPLOAD_FROM_LOAD" = windo ]; then echo "the first window begins"'),
    # (2) one_server and its consumers (first head)
    ('O1 one_server reads pid_same', BK,
     "    if not sb.get('pid') or not sa.get('pid'):\n        return 'status not read'\n    return 'yes' if sb['pid'] == sa['pid'] else 'NO: the pid changed'",
     "    return 'yes' if k.get('pid_same') else 'NO: the pid changed'"),
    ('O2 status not read only when both ends are', BK, "if not sb.get('pid') or not sa.get('pid'):", "if not sb.get('pid') and not sa.get('pid'):"),
    ('O3 status not read only at the end', BK, "if not sb.get('pid') or not sa.get('pid'):", "if not sa.get('pid'):"),
    ('O4 the uploader table prints status not read as one daemon', SH,
     "'status not read': \" (its status not read at an end of the window)\"}", "'status not read': ''}"),
    ('O5 the uploader table prints status not read as two daemons', SH,
     "{'yes': '', 'status not read': \" (its status not read at an end of the window)\"}", "{'yes': ''}"),
    ('O6 the status column back to pid_same', SH, "{'n/a' if not k else bracket.one_server(k)}",
     "{'n/a' if 'pid_same' not in k else 'yes' if k['pid_same'] else 'NO: the pid changed'}"),
    ('O7 the status column prints n/a for a part with no pid', SH, "{'n/a' if not k else bracket.one_server(k)}",
     "{'n/a' if not (k.get('before') or {}).get('pid') else bracket.one_server(k)}"),
    # (3) seed_in_window and its use (second head)
    ('S1 every upload row gets the seed rule', BK, "    if (k.get('upload') or {}).get('from_load') != 'window':\n        return None\n    sb, sa",
     "    if not k.get('upload'):\n        return None\n    sb, sa"),
    ('S2 the start\'s uploaded not asked', BK, "if sb.get('uploaded') != '0' or not", "if not"),
    ('S3 a whole upload at the end not asked', BK,
     " or not str(sa.get('uploaded_whole', '')).isdigit() or int(sa['uploaded_whole']) < 1:", ":"),
    ('S4 whole uploads counted from 0', BK, "int(sa['uploaded_whole']) < 1", "int(sa['uploaded_whole']) < 0"),
    ('S5 the end\'s lease not asked', BK, "if end[0] not in LEASE_HELD or not str(end[1]).isdigit():", "if not str(end[1]).isdigit():"),
    ('S6 the end\'s epoch not asked to be a number', BK, "if end[0] not in LEASE_HELD or not str(end[1]).isdigit():", "if end[0] not in LEASE_HELD:"),
    ('S7 a lease held at the start under another epoch taken', BK,
     "    if start[0] in LEASE_HELD and start[1] != end[1]:\n        return None\n", ''),
    ('S8 the epochs compared at any start lease', BK, "if start[0] in LEASE_HELD and start[1] != end[1]:", "if start[1] != end[1]:"),
    ('S9 unsure at the end not taken', BK, "if end[0] not in LEASE_HELD or", "if end[0] != 'held' or"),
    ('S10 the seed rule ignores the pids (redundant with not_uploading?)', BK,
     "    if not (sb.get('pid') and sa.get('pid') and k.get('pid_same') is True):\n        return None\n", ''),
    ('S11 the seed rule asked before the status and pid checks', BK,
     "    if seed_in_window(row):\n        return None\n", ''),  # and put first below
    ('S12 the marker not printed', BK, "            out['verdict'] += f' ({seed})'\n", "            pass\n"),
    ('S13 the seed\'s reason not appended', BK, "f'at its end {lease[1][0]} at epoch {lease[1][1]}){seed}')", "f'at its end {lease[1][0]} at epoch {lease[1][1]})')"),
    ('S14 the seed\'s reason for every row', BK, "        if (k.get('upload') or {}).get('from_load') == 'window':\n            seed = (",
     "        if True:\n            seed = ("),
    ('S15 a seed row not judged (the rule never holds)', BK, "    return f'seed inside window, lease at release (epoch {end[1]})'", "    return None"),
    ('S16 the marker names the start\'s epoch', BK, "lease at release (epoch {end[1]})'", "lease at release (epoch {start[1]})'"),
    ('S17 the rule judges past the other reasons (marker on a wide bracket)', BK,
     "    elif out['width'] > widest:", "    elif out['width'] > widest and not seed_in_window(row):"),
    ('S18 the start\'s whole uploads asked instead of its uploads', BK, "if sb.get('uploaded') != '0' or", "if sb.get('uploaded_whole', '0') != '0' or"),
    ('S19 the summary paragraph says nothing of the seed rule', SH,
     " A row whose seed was '\n          'let go at its first window", "'\n          'let go at its first window"),
]
first = int(sys.argv[2]) if len(sys.argv) > 2 else 0
last = int(sys.argv[3]) if len(sys.argv) > 3 else len(P)
todo = P[first:last]
base = {k: run(c) for f in {p for _, p, _, _ in todo} for k, c in TESTS[f]}
assert all(v == 0 for v in base.values()), base
for name, path, old, new in todo:
    full = f'{tree}/{path}'
    text = open(full).read()
    if text.count(old) != 1:
        print(f'{name}: NOT APPLIED ({text.count(old)} matches)', flush=True)
        continue
    planted = text.replace(old, new)
    if name.startswith('W2 '):
        assert planted.count(WIN_RESUME) == 2
        i = planted.rindex(WIN_RESUME)
        planted = planted[:i] + planted[i + len(WIN_RESUME):]
    if name.startswith('W4 '):
        anchor = '    # The pressure and the cgroups\' CPU at the end of the window, before\n'
        assert planted.count(anchor) == 1
        planted = planted.replace(anchor, WIN_RESUME + anchor)
    if name.startswith('S11 '):
        anchor = "    k = row.get('skyzfsd') or {}\n    if not k.get('upload'):\n        return None\n    sb, sa = k.get('before') or {}, k.get('after') or {}\n    # A reading"
        assert planted.count(anchor) == 1, planted.count(anchor)
        planted = planted.replace(anchor, "    if seed_in_window(row):\n        return None\n" + anchor)
    try:
        open(full, 'w').write(planted)
        r = {k: run(c) for k, c in TESTS[path]}
    finally:
        open(full, 'w').write(text)
    died = [f'{k}({v})' if v == 'TIMEOUT' else k for k, v in r.items() if v != 0]
    print(f"{name}: {'KILLED by ' + ', '.join(died) if died else 'SURVIVED'}", flush=True)
st = subprocess.run(['git', 'status', '--porcelain'], cwd=tree, capture_output=True, text=True).stdout
print('tree clean' if not st.strip() else f'TREE NOT CLEAN:\n{st}', flush=True)
