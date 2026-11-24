r"""plants_next.py TREE [FIRST [LAST]]: plants on #132's head (99b49b9; its bench files are
0a3c307's), one at a time in TREE.

A plant of topology-bench.sh is followed by test_copy_limit.sh, test_embedded_python.py and
tests/lib/test_skz_store.sh (as root); of bracket.py by test_bracket.py and
test_embedded_python.py; of tests/kernel/cut_check.py or nodeb_check.py by test_cut_check.py and
test_nodeb_check.py. KILLED: a test failed (TIMEOUT counts). SURVIVED: none did. NOT APPLIED: the
old text was not found exactly once. The file is restored after each plant (try/finally), every
test runs under a 600 s limit, and the tree is checked clean at the end. Run with python3 -I.
"""
import subprocess
import sys

tree = sys.argv[1]
SH, BK = 'tests/bench/topology-bench.sh', 'tests/bench/bracket.py'
CK, NB = 'tests/kernel/cut_check.py', 'tests/kernel/nodeb_check.py'
T_COPY = ('copy_limit', ['bash', 'tests/bench/test_copy_limit.sh'])
T_EMB = ('embedded', ['python3', 'tests/bench/test_embedded_python.py'])
T_STORE = ('skz_store', ['bash', 'tests/lib/test_skz_store.sh'])
T_BRK = ('bracket', ['python3', 'tests/bench/test_bracket.py'])
T_CUT = ('cut_check', ['python3', 'tests/kernel/test_cut_check.py'])
T_NB = ('nodeb_check', ['python3', 'tests/kernel/test_nodeb_check.py'])
TESTS = {SH: [T_COPY, T_EMB, T_STORE], BK: [T_BRK, T_EMB], CK: [T_CUT, T_NB], NB: [T_NB, T_CUT]}


def run(cmd):
    try:
        return subprocess.run(cmd, cwd=tree, capture_output=True, text=True, timeout=600).returncode
    except subprocess.TimeoutExpired:
        return 'TIMEOUT'


P = [
    # D2: one_server's split, and its consumers
    ('N1 D2 split dropped (every end with no pid: status not read)', BK,
     "        read = [s for s in nopid if set(s) - {'read_at_ns', 'pid'}]\n"
     "        return 'no serving daemon at an end' if read else 'status not read'",
     "        return 'status not read'"),
    ('N2 D2 every end with no pid read as no serving daemon', BK,
     "return 'no serving daemon at an end' if read else 'status not read'", "return 'no serving daemon at an end'"),
    ('N3 D2 the clock line counted as a field', BK, "set(s) - {'read_at_ns', 'pid'}", "set(s) - {'pid'}"),
    ('N4 D2 the tables print a gone daemon as two daemons', SH,
     ",\n                           'no serving daemon at an end': \" (NO SERVING DAEMON at an end of the window)\"}",
     "}"),
    # seed rows and the counts
    ('N5 seed_row never set', BK, r"        return 'no upload had committed at its window\'s start'", "        return None"),
    ('N6 seed_row for a row that does not upload', BK,
     "    if not k.get('upload'):\n        return None\n    if (k.get('before') or {}).get('uploaded') == '0':",
     "    if (k.get('before') or {}).get('uploaded') == '0':"),
    ('N7 seed_row from uploaded_whole', BK, ".get('uploaded') == '0':\n        return 'no upload",
     ".get('uploaded_whole', '0') == '0':\n        return 'no upload"),
    ('N8 the median counts the seed rows too', SH,
     "    steady = [(r, b) for r, b in judged if not b['seed_row']]", "    steady = judged"),
    ('N9 the counts swap at least and below', SH,
     'return f"{at + below} with a verdict ({at} at least, {below} below)',
     'return f"{at + below} with a verdict ({below} at least, {at} below)'),
    ('N10 a marked BELOW verdict not counted', SH,
     "b['verdict'].startswith('BELOW')", "b['verdict'] == 'BELOW 0.90'"),
    ('N11 the seed rows\' line not printed', SH, "    if seeds:\n        print(", "    if False:\n        print("),
    # released_here and the none rule
    ('N12 seed_in_window ignores released_here', BK,
     "    if (k.get('upload') or {}).get('from_load') != 'window' or k.get('released_here') is not True:\n"
     "        return None\n    sb, sa",
     "    if (k.get('upload') or {}).get('from_load') != 'window':\n        return None\n    sb, sa"),
    ('N13 a row with no released_here taken as released here', BK,
     "!= 'window' or k.get('released_here') is not True:", "!= 'window' or k.get('released_here') is False:"),
    ('N14 the clause "did not let MinIO go" dropped', BK,
     "        if (k.get('upload') or {}).get('from_load') == 'window' and k.get('released_here') is not True:\n"
     "            seed = \"; its window did not let MinIO go, so its seed was not measured from its release\"\n"
     "        elif", "        if"),
    ('N15 the start rule back to 27fe831\'s (held at the end\'s epoch taken)', BK,
     "    if start[0] != 'none':\n        return None\n",
     "    if start[0] in LEASE_HELD and start[1] != end[1]:\n        return None\n"),
    ('N16 the start rule refuses only a held lease (lost, missing taken)', BK,
     "    if start[0] != 'none':", "    if start[0] in LEASE_HELD:"),
    ('N17 the row reads released_here True always', SH,
     "    k['released_here'] = os.path.exists(base + '.released')", "    k['released_here'] = True"),
    ('N18 the window\'s mark not written', SH,
     '        [ -z "${1:-}" ] || cp "$SKZ_SEED_RELEASED" "$OUT/$1.released" 2>/dev/null || true\n', ''),
    ('N19 run_workload passes no tag (no mark)', SH,
     '    [ "$SKZ_UPLOAD_FROM_LOAD" != window ] || skz_minio_resume "$tag"\n',
     '    [ "$SKZ_UPLOAD_FROM_LOAD" != window ] || skz_minio_resume\n'),
    ('N20 the mark written though the unpause fails', SH,
     '    if skz_minio unpause; then\n        SKZ_MINIO_PAUSED=""\n'
     '        [ -z "${1:-}" ] || cp "$SKZ_SEED_RELEASED" "$OUT/$1.released" 2>/dev/null || true\n',
     '    [ -z "${1:-}" ] || cp "$SKZ_SEED_RELEASED" "$OUT/$1.released" 2>/dev/null || true\n'
     '    if skz_minio unpause; then\n        SKZ_MINIO_PAUSED=""\n'),
    ('N21 an earlier row\'s mark not removed', SH,
     '"$OUT/$tag.perfscript.gz" \\\n        "$OUT/$tag.released"', '"$OUT/$tag.perfscript.gz"'),
    # D4, D5 and the earlier survivors
    ('N22 D4 the watcher names the first window again', SH,
     "s after MinIO was let go at the start of the window that released it\"",
     "s after MinIO was let go at the first window's start\""),
    ('N23 D5 the BEFORE line dropped', SH,
     '            elif [ "${1:-}" = watch ] && [[ $rel =~ ^[0-9]+$ ]]; then\n'
     '                how="$how; BEFORE MinIO was let go (at $rel, the read at $now): the freeze did not hold"\n', ''),
    ('N24 D5 the BEFORE line without watch', SH,
     '            elif [ "${1:-}" = watch ] && [[ $rel =~ ^[0-9]+$ ]]; then',
     '            elif [[ $rel =~ ^[0-9]+$ ]]; then'),
    ('W7 the release time written after the unpause', SH,
     '    [ "$SKZ_UPLOAD_FROM_LOAD" != window ] || date +%s > "$SKZ_SEED_RELEASED"\n    if skz_minio unpause; then\n',
     '    if skz_minio unpause; then\n        [ "$SKZ_UPLOAD_FROM_LOAD" != window ] || date +%s > "$SKZ_SEED_RELEASED"\n'),
    ('W13 the release time not asked to be before the read that saw it', SH,
     ' && [ "$now" -ge "$rel" ]; then\n                how="$how; $(( prev', '; then\n                how="$how; $(( prev'),
    ('O7 the status column prints n/a for a start with no pid', SH, "{'n/a' if not k else bracket.one_server(k)}",
     "{'n/a' if not (k.get('before') or {}).get('pid') else bracket.one_server(k)}"),
    ('S17 the marker judged past a bracket too wide', BK,
     "    elif out['width'] > widest:", "    elif out['width'] > widest and not seed_in_window(row):"),
    ('S19 the paragraph says nothing of the seed rule', SH,
     " A row whose seed was '\n          'let go at its first window", "'\n          'let go at its first window"),
    ('S19b the paragraph says nothing of the seed rows', SH,
     " A seed row '\n          '(one that says it uploads", "'\n          '(one that says it uploads"),
    # the cut rig's and node B's pick (99b49b9)
    ('R1 the pick ignores differs', CK, "            if differs is None or differs(i):\n                return i\n",
     "            return i\n"),
    ('R2 the passed-over writes not kept', CK, "            if same is not None:\n                same.append(i)\n", ''),
    ('R3 the whole record compared, not the copies', CK,
     "                out.append((max(a, lo), min(b, lo + asize)))", "                out.append((a, b))"),
    ('R4 a discard leaves the bytes', CK, "            buf[s - lo:e - lo] = bytes(e - s)", "            pass"),
    ('R5 the withheld write applied all the same', CK, "if j == skip or not r['bytes']", "if not r['bytes']"),
    ('R6 the base image read as zeros', CK, "    buf = bytearray(base(lo, hi - lo))", "    buf = bytearray(hi - lo)"),
    ('R7 the cut\'s records counted to its uberblock write, not its prefix', CK,
     "            if (bytes_at(records, read, base, cut['prefix'], lo, hi)\n"
     "                    != bytes_at(records, read, base, cut['prefix'], lo, hi, skip=i)):",
     "            if (bytes_at(records, read, base, cut['ub_record'], lo, hi)\n"
     "                    != bytes_at(records, read, base, cut['ub_record'], lo, hi, skip=i)):"),
    ('R8 differs always true', CK, "                return True\n        return False\n    return differs",
     "                return True\n        return True\n    return differs"),
    ('R9 a write\'s bytes taken from its start, not from the range', CK,
     "buf[s - lo:e - lo] = read(r)[s - a:e - a]", "buf[s - lo:e - lo] = read(r)[0:e - s]"),
    ('R10 the cut rig\'s main picks without differs', CK,
     "        v = pick_withheld(ck.records, mid, bps, withholding_changes(ck.records, ck.read, ck.base_read, mid, bps), same)",
     "        v = pick_withheld(ck.records, mid, bps)"),
    ('R11 base_read reads zeros', CK, "        return data + bytes(n - len(data))", "        return bytes(n)"),
    ('R12 node B picks without differs', NB,
     "        v = cut_check.pick_withheld(ck.records, cut, bps, cut_check.withholding_changes(ck.records, ck.read, "
     "ck.base_read, cut, bps))", "        v = cut_check.pick_withheld(ck.records, cut, bps)"),
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
    try:
        open(full, 'w').write(text.replace(old, new))
        r = {k: run(c) for k, c in TESTS[path]}
    finally:
        open(full, 'w').write(text)
    died = [f'{k}({v})' if v == 'TIMEOUT' else k for k, v in r.items() if v != 0]
    print(f"{name}: {'KILLED by ' + ', '.join(died) if died else 'SURVIVED'}", flush=True)
st = subprocess.run(['git', 'status', '--porcelain'], cwd=tree, capture_output=True, text=True).stdout
print('tree clean' if not st.strip() else f'TREE NOT CLEAN:\n{st}', flush=True)
