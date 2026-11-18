"""scenarios.py ROOT: rows made from status lines as a v16 daemon prints them, through ROOT's own
row here-document and `summarize`; prints the two new tables, and bracket.py's verdict for each."""
import json, os, re, subprocess, sys, tempfile
ROOT = os.path.abspath(sys.argv[1]); SCRIPT = os.path.join(ROOT, 'tests/bench/topology-bench.sh')
HEREDOC = re.compile(r"<<\s*'PY'[^\n]*\n(.*?)\n\s*PY\n", re.S)
sys.path.insert(0, os.path.join(ROOT, 'tests/bench'))
import bracket
DEVS = 'dev pool 0 0 0 0 0 0 0 0\ndev origin 0 0 0 0 0 0 0 0\n'
def status(pid=None, **kv):
    # every key a v16 daemon prints of the held cuts, the map and the uploader, zeros unless given
    keys = dict(cuts=0, cut_state='begun', cuts_held=0, cut_copied_bytes=0, cut_bound_bytes=268435456, cuts_abandoned=0,
                cuts_replaced=0, intent_state='begun', intent_region_bytes=4194304, intent_regions=8192, intent_named=0,
                intent_named_at_start=8192, intent_syncs=0, intent_sync_ns=0, intent_sync_max_ns=0, intent_waits=0,
                intent_header_sync='off', intent_header_syncs=0, intent_header_sync_ns=0, intent_header_sync_max_ns=0,
                upload='x', upload_state='idle', upload_cut=0, lease='held', uploaded=0, uploaded_whole=0, upload_blocks=0,
                upload_found_committed=0, upload_committed_gone=0, upload_committed_kept=0, upload_mismatches=0,
                upload_abandoned_at_mismatch=0, upload_cuts_lost=0, upload_given_up=0, upload_errors=0)
    keys.update(kv)
    if pid: keys['pid'] = pid
    return DEVS + ''.join(f'skz {k} {v}\n' for k, v in keys.items())
def row(d, name, before, after, info, tps):
    doc = next(x for x in HEREDOC.findall(open(SCRIPT).read()) if 'sys.argv[12]' in x)
    open(os.path.join(d, 'row.py'), 'w').write(doc)
    base = os.path.join(d, 'r')
    open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w').write(info)
    for sfx, text in (('.before', before), ('.after', after), ('.pgbench', f'tps = {tps} (without initial connection time)\n')):
        open(base + sfx, 'w').write(text)
    p = subprocess.run(['python3', '-I', os.path.join(d, 'row.py'), base, 'zfs-skyzfsd', 'tpcb', '60', 'none', '0',
                        base + '.nolog', base + '.nofsync', 'x', '8', '0', os.path.join(ROOT, 'tests/bench')],
                       capture_output=True, text=True, timeout=60)
    assert p.returncode == 0, p.stderr[-1500:]
    r = json.loads(p.stdout); r['workload'] = name
    return r
UP = 'serve_args --hold-cuts\nrelease_every 0\nupload http://127.0.0.1:9000 b tb-1\nbuild abc\n'
S = {  # name: before, after, info
 'nomap (v16 daemon, no --hold-cuts, no upload)': (
     status(7, cut_state='off', intent_state='off', intent_regions=0, intent_named_at_start=0, upload_state='off', lease='none'),
     status(7, cut_state='off', intent_state='off', intent_regions=0, intent_named_at_start=0, upload_state='off', lease='none'),
     'serve_args \nrelease_every 0\nupload none\nbuild abc\n'),
 'restart (pid 7 then 9, figures from 0)': (
     status(7, cuts=50, uploaded=40, upload_blocks=9000, cuts_replaced=8, intent_syncs=900, intent_sync_ns=900000000, intent_waits=70),
     status(9, cuts=60, uploaded=3, upload_blocks=500, cuts_replaced=1, cuts_abandoned=0, intent_syncs=100, intent_sync_ns=1000000000, intent_waits=5),
     UP + 'first_upload after 170 s (uploaded 1, cuts lost 0)\n'),
 'gone (the daemon died in the window)': (
     status(7, cuts=50, uploaded=40), status(None, cuts=55, uploaded=44), UP + 'first_upload after 170 s (uploaded 1, cuts lost 0)\n'),
 'no lease all window': (
     status(7, cuts=4, upload_state='no_lease', lease='none', cuts_held=3, cuts_abandoned=1),
     status(7, cuts=28, upload_state='no_lease', lease='none', cuts_held=3, cuts_abandoned=1, cuts_replaced=24),
     UP + 'first_upload NONE in 900 s (state no_lease, uploading cut 0 of 4, cuts lost 0)\n'),
 'bound abandons (the first run)': (
     status(7, cuts=24, upload_state='reading', upload_cut=22, cuts_held=2, cuts_abandoned=5, upload_cuts_lost=5),
     status(7, cuts=48, upload_state='reading', upload_cut=45, cuts_held=2, cuts_abandoned=15, upload_cuts_lost=15, cuts_replaced=14),
     UP + 'first_upload NONE in 900 s (state reading, uploading cut 22 of 24, cuts lost 5)\n'),
 'a mismatch': (
     status(7, cuts=24, uploaded=20, cuts_held=1),
     status(7, cuts=48, uploaded=40, cuts_held=1, upload_mismatches=1, upload_abandoned_at_mismatch=3, cuts_abandoned=3, cuts_replaced=1),
     UP + 'first_upload after 170 s (uploaded 1, cuts lost 0)\n'),
}
with tempfile.TemporaryDirectory() as d:
    out = os.path.join(d, 'out', '1'); os.makedirs(out)
    rows = []
    for i, (name, (b, a, info)) in enumerate(S.items()):
        r = row(d, name, b, a, info, 950)
        r['_seq'] = 2; r['machine'] = 'm'; r['mem_limit'] = '1G'; r['clients'] = 8
        rows.append(r)
        json.dump(r, open(os.path.join(out, f'tpcb{i}-zfs-skyzfsd.json'), 'w'))
    p = subprocess.run(['bash', SCRIPT, 'summarize', os.path.join(d, 'out')], capture_output=True, text=True, timeout=120)
    assert p.returncode == 0, p.stderr[-1500:]
    for head in ('What the uploader did in the window', 'The intent map in the window'):
        part = p.stdout.split(head, 1)
        print('==', head)
        print('\n'.join(ln for ln in part[1].split('\n\n')[1].splitlines()) if len(part) == 2 else '(no table)')
    m = lambda seq: {'topology': 'zfs-slog', '_seq': seq, 'machine': 'm', 'workload': None, 'tps': 1000.0, 'origin_delay_ms': 0,
                     'clients': 8, 'duration_s': 60, 'mem_limit': '1G'}
    print('== bracket.py')
    for r in rows:
        ms = [dict(m(1), workload=r['workload']), dict(m(3), workload=r['workload'])]
        print(f"{r['workload']:48s} pid_same {r['skyzfsd'].get('pid_same')}: {bracket.bracketed(r, ms + [r])['verdict']}")
