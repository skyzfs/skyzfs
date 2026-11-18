"""plants3.py COPY: the plants of the review's findings at 72aaa06 (the answer to the
first review), one at a time in COPY (a copy of the head's tests and workflows), each
followed by the bench's tests: test_copy_limit.sh, test_embedded_python.py and
tests/lib/test_skz_store.sh. KILLED: a test failed. SURVIVED: none did. NOT APPLIED:
the text was not found exactly once. The file is restored after each plant."""
import subprocess, sys
copy = sys.argv[1]
SH, MN = 'tests/bench/topology-bench.sh', 'tests/bench/minio.sh'
T = {'copy_limit': ['bash', 'tests/bench/test_copy_limit.sh'],
     'embedded': ['python3', 'tests/bench/test_embedded_python.py'],
     'skz_store': ['bash', 'tests/lib/test_skz_store.sh']}
def run(cmd):
    try:
        return subprocess.run(cmd, cwd=copy, capture_output=True, text=True, timeout=900).returncode
    except subprocess.TimeoutExpired:
        return 'TIMEOUT'
base = {k: run(c) for k, c in T.items()}
assert all(v == 0 for v in base.values()), base
P = [
 ('R1 TB_SKZ_UPLOAD value not checked', SH, '[ -z "$SKZ_UPLOAD" ] || [ "$SKZ_UPLOAD" = 1 ] || { echo "TB_SKZ_UPLOAD is 1 or unset', 'true || { echo "TB_SKZ_UPLOAD is 1 or unset'),
 ('R2 the four settings not checked', SH, '[ -n "${!_v:-}" ] || { echo "TB_SKZ_UPLOAD=1 needs', 'true || { echo "TB_SKZ_UPLOAD=1 needs'),
 ('R3 TB_S3_SECRET left out of the check', SH, 'for _v in TB_S3_ENDPOINT TB_S3_BUCKET TB_S3_KEY_ID TB_S3_SECRET; do', 'for _v in TB_S3_ENDPOINT TB_S3_BUCKET TB_S3_KEY_ID; do'),
 ('R4 upload with the releaser let through', SH, '[ -z "$SKZ_RELEASE" ] || { echo "TB_SKZ_UPLOAD=1 and TB_SKZ_RELEASE', 'true || { echo "TB_SKZ_UPLOAD=1 and TB_SKZ_RELEASE'),
 ('R5 upload beside or without the daemon let through', SH, 'if [ "$SKZ_UPLOAD" = 1 ] && [ "${_topos[*]}" != zfs-skyzfsd ]; then', 'if false; then'),
 ('R6 that refusal exits 0', SH, 'echo "TB_SKZ_UPLOAD=1 is for zfs-skyzfsd only, not: $TOPOLOGIES" >&2; exit 64', 'echo "TB_SKZ_UPLOAD=1 is for zfs-skyzfsd only, not: $TOPOLOGIES" >&2; exit 0'),
 ('R7 TB_SKZ_UPLOAD_WAIT_S not checked', SH, '[[ $SKZ_UPLOAD_WAIT_S =~ ^[0-9]+$ ]] || {', 'true || {'),
 ('R8 upload with copies repeated let through', SH, '[ -z "$SKZ_COPY_REPEAT" ] || { echo "TB_SKZ_UPLOAD=1 and TB_SKYZFSD_COPY_REPEAT', 'true || { echo "TB_SKZ_UPLOAD=1 and TB_SKYZFSD_COPY_REPEAT'),
 ('R9 every upload row refused, the daemon alone too', SH, 'if [ "$SKZ_UPLOAD" = 1 ] && [ "${_topos[*]}" != zfs-skyzfsd ]; then', 'if [ "$SKZ_UPLOAD" = 1 ]; then'),
 ('U1 the keys not handed to the daemon', SH, 'export AWS_ACCESS_KEY_ID="$TB_S3_KEY_ID" AWS_SECRET_ACCESS_KEY="$TB_S3_SECRET"', ':'),
 ('U4 no upload line in .skzinfo', SH, 'echo "upload $TB_S3_ENDPOINT $TB_S3_BUCKET $SKZ_VOLUME_NAME" >> "$OUT/$TOPO.skzinfo"', ':'),
 ('T1 abandoned: the end value', SH, "{d('cuts_abandoned')} (", "{sa.get('cuts_abandoned', 'n/a')} ("),
 ('T2 mismatches: the end value', SH, "{d('upload_mismatches')} | {d('upload_errors')}", "{sa.get('upload_mismatches', 'n/a')} | {d('upload_errors')}"),
 ('T3 errors: the end value', SH, "{d('upload_mismatches')} | {d('upload_errors')}", "{d('upload_mismatches')} | {sa.get('upload_errors', 'n/a')}"),
 ('T4 abandoned and mismatches swapped', SH, "{d('cuts_abandoned')} ({d('upload_abandoned_at_mismatch')}) | \"\n                  f\"{d('upload_mismatches')}",
  "{d('upload_mismatches')} ({d('upload_abandoned_at_mismatch')}) | \"\n                  f\"{d('cuts_abandoned')}"),
 ('T11 abandoned at a mismatch: the end value', SH, "({d('upload_abandoned_at_mismatch')})", "({sa.get('upload_abandoned_at_mismatch', 'n/a')})"),
 ('T12 held at the start reads the end', SH, "{sb.get('cuts_held', 'n/a')} →", "{sa.get('cuts_held', 'n/a')} →"),
 ('T13 no mark for two daemons', SH, "if r['skyzfsd'].get('pid_same') is False else ''", "if False else ''"),
 ('I1 named at end: the start value', SH, "{sa.get('intent_named', 'n/a')} / {sa.get('intent_regions', 'n/a')}", "{sb.get('intent_named', 'n/a')} / {sa.get('intent_regions', 'n/a')}"),
 ('I2 longest map sync: the start value', SH, "{ms('intent_sync_max_ns')}", "{sb.get('intent_sync_max_ns', 'n/a')}"),
 ('I3 header sync said from the start', SH, "{sa.get('intent_header_sync', 'n/a')}", "{sb.get('intent_header_sync', 'n/a')}"),
 ('I8 "ms each" divides by a count of 0', SH, "if dn(n) and dn(ns) is not None else 'n/a'", "if dn(n) is not None and dn(ns) is not None else 'n/a'"),
 ('I9 rows with the map off kept', SH, "\n           and r['skyzfsd']['after'].get('intent_state', 'off') != 'off']", ']'),
 ('W7 an unreadable status never ends the wait', SH, '        if [ "$unread" -ge 3 ]; then\n', '        if false; then\n'),
 ('W8 one unreadable status ends the wait', SH, '        if [ "$unread" -ge 3 ]; then\n', '        if [ "$unread" -ge 1 ]; then\n'),
 ('M1 minio.sh down deletes any directory', MN, '    [ "$(basename -- "${1:?}")" = minio-data ] || {', '    true || {'),
]
# An optional second argument, FIRST: start at plant FIRST (0-based), so that
# the list can be run in two halves on two copies.
first = int(sys.argv[2]) if len(sys.argv) > 2 else 0
for name, path, old, new in P[first:]:
    full = f'{copy}/{path}'
    text = open(full).read()
    if text.count(old) != 1:
        print(f'{name}: NOT APPLIED ({text.count(old)} matches)', flush=True)
        continue
    try:
        open(full, 'w').write(text.replace(old, new))
        r = {k: run(c) for k, c in T.items()}
    finally:
        open(full, 'w').write(text)
    died = [k for k, v in r.items() if v != 0]
    print(f"{name}: {'KILLED by ' + ', '.join(died) if died else 'SURVIVED'}", flush=True)
