"""pid_table.py TREE: the three consumers of a daemon row's pid readings, and its verdict.

Every combination of what the daemon's status showed at the window's two ends is written
as `snapshot` writes it (.before and .after), run through TREE's row here-document, and
then through TREE's `summarize` all at once. For each case it prints what the status
table's "one server throughout" column, the uploader table's row name and the intent map
table's row name say, and the verdict of bracket.bracketed between two model rows.

A reading at an end is one of:
  missing  no `skz` line at all (Codex's case on #128)
  failed   `skz read_at_ns` alone (a status read that failed)
  gone     the status read, with no `pid` line: the daemon is gone (counters "left by a
           daemon that is gone"), as status.rs prints for a device that is not live
  7, 8, 0  the status read, with `skz pid 7`, `8` or `0`
Each case is an upload row with an intent map (so it reaches all three tables), held at
epoch 1 at both ends, uploads 3 then 27. Run with python3 -I from a directory of its own.
"""
import json
import os
import re
import subprocess
import sys
import tempfile

tree = sys.argv[1]
HERE = os.path.join(tree, 'tests/bench')
sys.path.insert(0, HERE)
import bracket as bk  # noqa: E402

SCRIPT = os.path.join(HERE, 'topology-bench.sh')
HEREDOC = re.compile(r"<<\s*'PY'[^\n]*\n(.*?)\n\s*PY\n", re.S)
DEVS = 'dev pool 0 0 0 0 0 0 0 0\ndev origin 0 0 0 0 0 0 0 0\n'
STARTS = ['missing', 'failed', 'gone', '7', '0']
ENDS = ['missing', 'failed', 'gone', '7', '8', '0']


def look(kind, t, up):
    if kind == 'missing':
        return DEVS
    if kind == 'failed':
        return DEVS + f'skz read_at_ns {t}\n'
    body = (f'skz read_at_ns {t}\nskz lease held\nskz lease_epoch 1\nskz uploaded {up}\nskz uploaded_whole 1\n'
            'skz cuts 10\nskz intent_state taken_up\nskz intent_syncs 5\nskz intent_sync_ns 500\n'
            'skz intent_sync_max_ns 9\nskz intent_waits 0\n')
    if kind == 'gone':
        return DEVS + body + 'skz counters left_by_a_daemon_that_is_gone,_pid_7\n'
    return DEVS + body + f'skz pid {kind}\nskz counters of_the_serving_daemon,_pid_{kind}\n'


with open(SCRIPT) as f:
    doc = next(x for x in HEREDOC.findall(f.read()) if 'sys.argv[12]' in x)
rows = {}
with tempfile.TemporaryDirectory() as d:
    prog = os.path.join(d, 'row.py')
    with open(prog, 'w') as f:
        f.write(doc)
    out = os.path.join(d, 'out')
    for s in STARTS:
        for e in ENDS:
            name = f'{s}-{e}'
            cd = os.path.join(d, 'cases', name)
            os.makedirs(cd)
            with open(os.path.join(cd, 'zfs-skyzfsd.skzinfo'), 'w') as f:
                f.write('serve_args \nrelease_every 0\nupload http://127.0.0.1:9000 b v\nbuild abc\n')
            base = os.path.join(cd, 'r')
            for suf, text in (('.before', look(s, 1000, 3)), ('.after', look(e, 2000, 27)),
                              ('.pgbench', 'tps = 950 (without initial connection time)\n')):
                with open(base + suf, 'w') as f:
                    f.write(text)
            p = subprocess.run(['python3', '-I', prog, base, 'zfs-skyzfsd', 'tpcb', '60', 'none', '0',
                                base + '.nolog', base + '.nofsync', name, '8', '0', HERE],
                               capture_output=True, text=True, timeout=60)
            if p.returncode:
                print(f'{name}: the row failed: {p.stderr[-300:]}')
                continue
            r = json.loads(p.stdout)
            rows[name] = r
            od = os.path.join(out, name)
            os.makedirs(od)
            with open(os.path.join(od, 'tpcb-zfs-skyzfsd.json'), 'w') as f:
                json.dump(r, f)
    p = subprocess.run(['bash', SCRIPT, 'summarize', out], capture_output=True, text=True, timeout=300)
    if p.returncode:
        print('summarize failed', p.stderr[-1000:])
        sys.exit(2)
    text = p.stdout


def section(after_words):
    if after_words not in text:
        return []
    # The table that follows the words (in its own header line, or after a paragraph).
    rest = text.split(after_words, 1)[1]
    i = rest.find('\n|---')
    return rest[i + 1:].split('\n\n')[0].splitlines()


status = section('one server throughout')
ups = section('What the uploader did in the window')
ims = section('The intent map in the window')


def by_label(lines, col):
    got = {}
    for ln in lines:
        if not ln.startswith('| tpcb | '):
            continue
        f = ln.split(' | ')
        m = re.match(r'zfs-skyzfsd \[([^\]]+)\](.*)', f[1])
        if m:
            got[m.group(1)] = m.group(2).strip() if col is None else f[col]
    return got


st, up, im = by_label(status, 13), by_label(ups, None), by_label(ims, None)
print(f'{"start-end":16} | {"part":7} | {"pid_same":8} | {"status column":20} | {"uploader table":45} | '
      f'{"intent table":45} | verdict')
for name, r in rows.items():
    k = r.get('skyzfsd')
    rr = dict(r, _seq=2, machine='m', origin_delay_ms=0)
    m1 = {'_seq': 1, 'topology': 'zfs-slog', 'tps': 1000, 'machine': 'm', 'origin_delay_ms': 0,
          'workload': 'tpcb'}
    for o in (rr,):
        o.pop('duration_s', None); o.pop('clients', None); o.pop('mem_limit', None)
    v = bk.bracketed(rr, [m1, rr, dict(m1, _seq=3)])['verdict']
    print(f"{name:16} | {'present' if k else 'ABSENT':7} | {str((k or {}).get('pid_same', '-')):8} | "
          f"{st.get(name, '(no line)'):20} | {up.get(name, '(no line)') or '(plain)':45} | "
          f"{im.get(name, '(no line)') or '(plain)':45} | {v[:150]}")
