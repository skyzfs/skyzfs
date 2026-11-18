"""plants.py COPY PROBE: plant each change in COPY (a copy of the head's tree), one at a time,
and run the PR's own tests and the probe (refusals.sh). KILLED: a test failed, or the probe's
output differs from the unchanged tree's. SURVIVED: nothing noticed. NOT APPLIED: the text
was not found exactly once. The file is restored after each plant (try/finally)."""
import os, subprocess, sys
copy, probe = sys.argv[1], sys.argv[2]
SH, YML = 'tests/bench/topology-bench.sh', '.github/workflows/topology-bench.yml'
def run(cmd):
    try:
        p = subprocess.run(cmd, cwd=copy, capture_output=True, text=True, timeout=900)
        return p.returncode, p.stdout + p.stderr
    except subprocess.TimeoutExpired:
        return 'TIMEOUT', ''
def tests():
    out = {}
    out['copy_limit'] = run(['bash', 'tests/bench/test_copy_limit.sh'])[0]
    out['embedded'] = run(['python3', '-I', 'tests/bench/test_embedded_python.py'])[0]
    out['skz_store'] = run(['bash', 'tests/lib/test_skz_store.sh'])[0]
    return out
def probe_out():
    return run(['bash', probe, copy])[1]
base_t, base_p = tests(), probe_out()
assert all(v == 0 for v in base_t.values()), base_t
P = [
 # the refusals
 ('R1 TB_SKZ_UPLOAD value not checked', SH, '[ -z "$SKZ_UPLOAD" ] || [ "$SKZ_UPLOAD" = 1 ] || { echo "TB_SKZ_UPLOAD is 1 or unset', 'true || { echo "TB_SKZ_UPLOAD is 1 or unset'),
 ('R2 the four settings not checked', SH, '[ -n "${!_v:-}" ] || { echo "TB_SKZ_UPLOAD=1 needs', 'true || { echo "TB_SKZ_UPLOAD=1 needs'),
 ('R3 TB_S3_SECRET left out of the check', SH, 'for _v in TB_S3_ENDPOINT TB_S3_BUCKET TB_S3_KEY_ID TB_S3_SECRET; do', 'for _v in TB_S3_ENDPOINT TB_S3_BUCKET TB_S3_KEY_ID; do'),
 ('R4 upload with the releaser let through', SH, '[ -z "$SKZ_RELEASE" ] || { echo "TB_SKZ_UPLOAD=1 and TB_SKZ_RELEASE', 'true || { echo "TB_SKZ_UPLOAD=1 and TB_SKZ_RELEASE'),
 ('R5 upload without a daemon let through', SH, 'if [ "$SKZ_UPLOAD" = 1 ] && [[ " $TOPOLOGIES " != *" zfs-skyzfsd "* ]]; then', 'if false; then'),
 ('R6 upload without a daemon exits 0', SH, 'echo "TB_SKZ_UPLOAD=1 is for zfs-skyzfsd only, not: $TOPOLOGIES" >&2; exit 64', 'echo "TB_SKZ_UPLOAD=1 is for zfs-skyzfsd only, not: $TOPOLOGIES" >&2; exit 0'),
 ('R7 TB_SKZ_UPLOAD_WAIT_S not checked', SH, '[[ $SKZ_UPLOAD_WAIT_S =~ ^[0-9]+$ ]] || {', 'true || {'),
 # what an upload row does
 ('U1 the keys not handed to the daemon', SH, 'export AWS_ACCESS_KEY_ID="$TB_S3_KEY_ID" AWS_SECRET_ACCESS_KEY="$TB_S3_SECRET"', ':'),
 ('U2 the create names no place', SH, 'args+=(--endpoint "$TB_S3_ENDPOINT" --bucket "$TB_S3_BUCKET" --volume "$SKZ_VOLUME_NAME")', ':'),
 ('U3 one volume name for every create', SH, 'SKZ_VOLUME_NAME="tb-$(date +%s%N)"', 'SKZ_VOLUME_NAME="${SKZ_VOLUME_NAME:-tb-$(date +%s%N)}"'),
 ('U4 no upload line in .skzinfo', SH, 'echo "upload $TB_S3_ENDPOINT $TB_S3_BUCKET $SKZ_VOLUME_NAME" >> "$OUT/$TOPO.skzinfo"', ':'),
 ('W1 the wait never runs', SH, '    if [ "$1" = skyzfsd ] && [ "$SKZ_UPLOAD" = 1 ] && [ "$SKZ_UPLOAD_WAIT_S" -gt 0 ]; then\n        lim 120 zpool sync "$ZFS_POOL"\n        skz_first_upload\n    fi\n', ''),
 ('W2 no sync before the wait', SH, '        lim 120 zpool sync "$ZFS_POOL"\n        skz_first_upload\n', '        skz_first_upload\n'),
 ('W3 the wait on every daemon row', SH, 'if [ "$1" = skyzfsd ] && [ "$SKZ_UPLOAD" = 1 ] && [ "$SKZ_UPLOAD_WAIT_S" -gt 0 ]; then', 'if [ "$1" = skyzfsd ]; then'),
 ('W4 the wait ends at uploaded 2', SH, '[[ $up =~ ^[0-9]+$ ]] && [ "$up" -ge 1 ]', '[[ $up =~ ^[0-9]+$ ]] && [ "$up" -ge 2 ]'),
 ('W5 a run-out wait fails the row', SH, 'log "skyzfsd: no upload committed in $SKZ_UPLOAD_WAIT_S s (state ${st:-n/a}, cuts lost ${lost:-n/a}); the windows begin all the same"\n            return 0', 'log "skyzfsd: no upload committed in $SKZ_UPLOAD_WAIT_S s (state ${st:-n/a}, cuts lost ${lost:-n/a}); the windows begin all the same"\n            return 1'),
 ('W6 the wait off by one second (gt)', SH, '-ge "$SKZ_UPLOAD_WAIT_S" ]', '-gt "$SKZ_UPLOAD_WAIT_S" ]'),
 # the uploader's table
 ('T1 cuts_abandoned shown at the end, not over the window', SH, "{d('cuts_abandoned')}", "{sa.get('cuts_abandoned', 'n/a')}"),
 ('T2 mismatches shown at the end', SH, "{d('upload_mismatches')}", "{sa.get('upload_mismatches', 'n/a')}"),
 ('T3 errors shown at the end', SH, "{d('upload_errors')} | {d('upload_cuts_lost')}", "{sa.get('upload_errors', 'n/a')} | {d('upload_cuts_lost')}"),
 ('T4 abandoned and mismatches swapped', SH, "{d('cuts_abandoned')} | {d('upload_mismatches')}", "{d('upload_mismatches')} | {d('cuts_abandoned')}"),
 ('T5 the abandoned column reads abandoned_at_mismatch', SH, "{d('cuts_abandoned')}", "{d('upload_abandoned_at_mismatch')}"),
 ('T6 lost shown at the end', SH, "{d('upload_cuts_lost')}", "{sa.get('upload_cuts_lost', 'n/a')}"),
 ('T7 held at the start, not the end', SH, "{sa.get('cuts_held', 'n/a')}", "{sb.get('cuts_held', 'n/a')}"),
 ('T8 first upload column empty', SH, "{r['skyzfsd'].get('info', {}).get('first_upload', 'not waited for')}", "{'not waited for'}"),
 ('T9 replaced shown at the end', SH, "{d('cuts_replaced')}", "{sa.get('cuts_replaced', 'n/a')}"),
 ('T10 the delta for a gauge-like reading: negative allowed through as n/a', SH, "return str(int(a) - int(b)) if a.isdigit() and b.isdigit() else 'n/a'", "return str(max(int(a) - int(b), 0)) if a.isdigit() and b.isdigit() else 'n/a'"),
 # the intent map's table
 ('I1 named at end from the start', SH, "{sa.get('intent_named', 'n/a')} / {sa.get('intent_regions', 'n/a')}", "{sb.get('intent_named', 'n/a')} / {sa.get('intent_regions', 'n/a')}"),
 ('I2 longest map sync from the start', SH, "{ms('intent_sync_max_ns')}", "{'n/a' if not sb.get('intent_sync_max_ns', '').isdigit() else f\"{int(sb['intent_sync_max_ns']) / 1e6:.2f}\"}"),
 ('I3 header sync said from the start', SH, "{sa.get('intent_header_sync', 'n/a')}", "{sb.get('intent_header_sync', 'n/a')}"),
 ('I4 waits at the end', SH, "{s(dn('intent_waits'))}", "{sa.get('intent_waits', 'n/a')}"),
 ('I5 header ms each from the map ns', SH, "{each('intent_header_syncs', 'intent_header_sync_ns')}", "{each('intent_header_syncs', 'intent_sync_ns')}"),
 ('I6 rows with no map kept (filter on intent_state)', SH, "ims = [r for r in runs if r.get('skyzfsd', {}).get('after', {}).get('intent_syncs', '').isdigit()]", "ims = [r for r in runs if r.get('skyzfsd', {}).get('after', {}).get('intent_syncs', '').isdigit() and r['skyzfsd']['after'].get('intent_state') != 'off']"),
 ('I7 named at end from regions', SH, "{sa.get('intent_named', 'n/a')} / {sa.get('intent_regions', 'n/a')}", "{sa.get('intent_regions', 'n/a')} / {sa.get('intent_regions', 'n/a')}"),
 ('I8 ms each divides by zero when no syncs', SH, "if dn(n) and dn(ns) is not None else 'n/a'", "if dn(n) is not None and dn(ns) is not None else 'n/a'"),
]
for name, rel, old, new in P:
    path = os.path.join(copy, rel)
    orig = open(path).read()
    if orig.count(old) != 1:
        print(f'{name}: NOT APPLIED ({orig.count(old)} matches)'); continue
    try:
        open(path, 'w').write(orig.replace(old, new))
        t, p = tests(), probe_out()
    finally:
        open(path, 'w').write(orig)
    died = [k for k, v in t.items() if v != 0]
    changed = [b for a, b in zip(base_p.splitlines(), p.splitlines()) if a != b]
    verdict = 'KILLED' if died or changed else 'SURVIVED'
    print(f"{name}: {verdict}; PR's tests that died: {', '.join(died) or 'none'}; probe: "
          f"{'changed: ' + changed[0][:110] if changed else 'unchanged'}")
