#!/usr/bin/env python3
"""#119 at cfe8146, the next look: what the head does in the cases the
answers to the first review rest on.

    scenarios2.py CHECKOUT RUN_DIR

RUN_DIR holds the recordings of run 44280164949 (ufailed, umoved, ulost,
ulease, ukill), downloaded by hand into a directory of its own. They are
data: each is copied, changed as the scenario says and judged by the
head's cut_daemon_check.py under `python3 -I`, as cut-record.sh calls
it. Nothing from RUN_DIR is run.

A  the watcher's main (cut_watch.py as a command) against a stand-in
   node B: D2 asked that a `get` that always fails ends the watcher with
   2, and that main has a test.
B  ufailed: D4 asked whether a first cut of the new state that leaves
   out the intent map's regions is told from one that follows it.
C  the balance's start look (D3).
D  ulease's rows before the release (cd849a6), and how many there were.
E  the look before another writer's act: what the judge says when the
   status of the look is one upload older than its row.
"""
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time

ROOT, RUN = sys.argv[1], sys.argv[2]
JUDGE = os.path.join(ROOT, 'tests', 'kernel', 'cut_daemon_check.py')
WATCH = os.path.join(ROOT, 'tests', 'kernel', 'cut_watch.py')

STANDIN = r'''#!/usr/bin/env python3
import os, sys
cmd = sys.argv[1]
if cmd == 'row':
    print('head\t' + 'ab' * 32 + '\t3\nrow\t' + 'cd' * 16 + '\t5\t40\nlease\trunner\t1\t99')
    sys.exit(0)
if cmd == 'get':
    sys.stderr.write('stand-in: get fails\n')
    sys.exit(int(os.environ['STANDIN_GET_RC']))
sys.exit(64)
'''


def watcher(get_rc, stop_first, limit):
    """cut_watch.py's main beside a node B whose `row` works and whose
    `get` ends with get_rc -> (exit or None after limit s, watch.log's
    last line)."""
    tmp = tempfile.mkdtemp(prefix='s2-watch-')
    try:
        nodeb = os.path.join(tmp, 'nodeb')
        with open(nodeb, 'w') as f:
            f.write(STANDIN)
        os.chmod(nodeb, os.stat(nodeb).st_mode | stat.S_IXUSR)
        out = os.path.join(tmp, 'out')
        os.mkdir(out)
        stop = os.path.join(tmp, 'stop')
        if stop_first:
            open(stop, 'w').close()
        env = dict(os.environ, STANDIN_GET_RC=str(get_rc), PYTHONDONTWRITEBYTECODE='1')
        p = subprocess.Popen([sys.executable, '-I', WATCH, out, nodeb, 'http://x', 'b', 'v', stop], env=env)
        t0 = time.monotonic()
        try:
            rc = p.wait(timeout=limit)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait()
            rc = None
        took = time.monotonic() - t0
        log = ''
        if os.path.exists(os.path.join(out, 'watch.log')):
            with open(os.path.join(out, 'watch.log')) as f:
                lines = f.read().splitlines()
            log = lines[-1].split('\t', 1)[-1] if lines else ''
        return rc, round(took), log
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def judged(name, change):
    """The recording NAME of RUN, copied and changed by change(dir) ->
    (exit, {line: (verdict, detail)})."""
    tmp = tempfile.mkdtemp(prefix='s2-judge-')
    try:
        d = os.path.join(tmp, name)
        shutil.copytree(os.path.join(RUN, name), d)
        os.remove(os.path.join(d, 'results.tsv'))
        change(d)
        p = subprocess.run([sys.executable, '-I', JUDGE, name, d, os.path.join(d, 'requests.tsv'), '-',
                            str(512 << 20)], capture_output=True, text=True, timeout=1200)
        lines = {}
        if os.path.exists(os.path.join(d, 'results.tsv')):
            with open(os.path.join(d, 'results.tsv')) as f:
                for ln in f:
                    key, verdict, detail = ln.rstrip('\n').split('\t', 2)
                    lines[key] = (verdict, detail)
        return p.returncode, lines
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def show(title, rc, lines, keys):
    print(f'{title}: exit {rc}')
    for k in keys:
        if k in lines:
            print(f'    {k}\t{lines[k][0]}\t{lines[k][1][:230]}')
        else:
            print(f'    {k}\t(no line)')


def rewrite(d, fn, f):
    path = os.path.join(d, fn)
    with open(path) as fh:
        text = fh.read()
    with open(path, 'w') as fh:
        fh.write(f(text))


def raw_lines(text, keep):
    """uploaded-blocks.tsv with the lines of blocks 512..527 kept only
    where keep(fetch) says."""
    out = []
    for ln in text.splitlines(True):
        x = ln.split('\t')
        if len(x) == 4 and 512 <= int(x[2]) < 528 and not keep(int(x[0])):
            continue
        out.append(ln)
    return ''.join(out)


def raw_to_first(text):
    """The raw blocks' lines moved to fetch 0 (the old state's last cut),
    at the place the file's order asks for."""
    moved, rest = [], []
    for ln in text.splitlines(True):
        x = ln.split('\t')
        (moved if len(x) == 4 and 512 <= int(x[2]) < 528 else rest).append(ln)
    first = [ln for ln in rest if ln.split('\t')[0] == '0']
    number = first[0].split('\t')[1]
    moved = [f"0\t{number}\t{ln.split(chr(9))[2]}\t{ln.split(chr(9))[3]}" for ln in moved]
    first = sorted(first + moved, key=lambda ln: int(ln.split('\t')[2]))
    return ''.join(first + [ln for ln in rest if ln.split('\t')[0] != '0'])


def status_less_one(text):
    out = []
    for ln in text.splitlines(True):
        if ln.startswith('uploaded '):
            ln = f"uploaded {int(ln.split()[1]) - 1}\n"
        out.append(ln)
    return ''.join(out)


def main():
    print('A  the watcher as a command, node B\'s row working')
    for title, rc, stop, limit in (('get always ends with 2, no stop', 2, False, 30),
                                   ('get always ends with 2, the stop there from the start', 2, True, 30),
                                   ('get ends with 4 (content not readable)', 4, False, 30)):
        got = watcher(rc, stop, limit)
        print(f'    {title}: exit {got[0]} after {got[1]} s; log: {got[2][:110]}')

    U = ('upload_content', 'upload_raw', 'upload_balance', 'upload_course')
    print('\nB  ufailed (run 44280164949), the 16 blocks written at 2 MiB before the failure')
    rc, lines = judged('ufailed', lambda d: None)
    show('    as recorded', rc, lines, U[:2])
    rc, lines = judged('ufailed', lambda d: rewrite(d, 'uploaded-blocks.tsv', lambda t: raw_lines(t, lambda f: False)))
    show('    node B\'s image never holds them (a first cut that leaves out the map\'s regions)', rc, lines, U[:2])
    rc, lines = judged('ufailed', lambda d: rewrite(d, 'uploaded-blocks.tsv', raw_to_first))
    show('    node B\'s image holds them at the old state\'s last cut already (a cut made of the raw write)', rc, lines,
         U[:2])

    print('\nC  the balance\'s start look')
    rc, lines = judged('ufailed', lambda d: rewrite(d, 'balance-start.txt',
                                                    lambda t: t.replace('cuts_held 0', 'cuts_held 1')))
    show('    ufailed, balance-start.txt with a cut held', rc, lines, U[2:3])
    rc, lines = judged('ufailed', lambda d: os.remove(os.path.join(d, 'balance-start.txt')))
    show('    ufailed, no balance-start.txt', rc, lines, ('upload_balance', 'rig_error'))
    rc, lines = judged('ukill', lambda d: None)
    show('    ukill as recorded', rc, lines, U[2:3])
    for fn in ('status-start-2.txt', 'balance-start.txt', 'status.txt'):
        with open(os.path.join(RUN, 'ukill', fn)) as f:
            st = dict(ln.split(' ', 1) for ln in f.read().splitlines() if ' ' in ln)
        print(f"    ukill {fn}: " + ', '.join(f"{k} {st.get(k)}" for k in (
            'upload_state', 'cuts', 'cuts_held', 'uploaded', 'upload_found_committed', 'cuts_replaced', 'cuts_abandoned')))

    print('\nD  ulease, node B\'s rows before the other holder let go')
    with open(os.path.join(RUN, 'ulease', 'release-ns.txt')) as f:
        until = int(f.read().split()[0])
    with open(os.path.join(RUN, 'ulease', 'rows.tsv')) as f:
        rows = [ln.split('\t') for ln in f.read().splitlines()]
    with open(os.path.join(RUN, 'ulease', 'requests.tsv')) as f:
        first = f.readline().split('\t')[1]
        last = [ln.split('\t')[0] for ln in f if ln.split('\t')[1] == first][-1]
    before = [r for r in rows if int(r[0]) < until]
    print(f"    rows {len(rows)}, before the release {len(before)}; the last of them "
          f"{(until - int(before[-1][0])) / 1e9:.2f} s before the release and "
          f"{(int(before[-1][0]) - int(last)) / 1e9:+.2f} s from the first instance's last request")

    def head_moves(t):
        x = t.splitlines(True)
        f = x[1].split('\t')
        x[1] = '\t'.join([f[0], 'ee' * 32 + ' 1', f[2], f[3]])
        return ''.join(x)

    def row_named(t):
        x = t.splitlines(True)
        f = x[1].split('\t')
        x[1] = '\t'.join([f[0], f[1], 'ab' * 16 + ' 1 4', f[3]])
        return ''.join(x)
    rc, lines = judged('ulease', lambda d: None)
    show('    as recorded', rc, lines, U[3:])
    rc, lines = judged('ulease', lambda d: rewrite(d, 'rows.tsv', head_moves))
    show('    head another commit at the second look', rc, lines, U[3:])
    rc, lines = judged('ulease', lambda d: rewrite(d, 'rows.tsv', row_named))
    show('    the row names a cut at the second look', rc, lines, U[3:])

    print('\nE  the status of the look one upload older than its row (the daemon right)')
    rc, lines = judged('umoved', lambda d: rewrite(d, 'moved-status.txt', status_less_one))
    show('    umoved', rc, lines, ('upload_status', 'upload_course'))
    rc, lines = judged('ulost', lambda d: rewrite(d, 'lost-status.txt', status_less_one))
    show('    ulost', rc, lines, ('upload_status', 'upload_course'))
    for name, fn in (('umoved', 'moved'), ('ulost', 'lost')):
        with open(os.path.join(RUN, name, f'{fn}-status.txt')) as f:
            st = dict(ln.split(' ', 1) for ln in f.read().splitlines() if ' ' in ln)
        with open(os.path.join(RUN, name, f'{fn}-row.txt')) as f:
            row = [ln.split('\t') for ln in f.read().splitlines() if ln.startswith('row\t')][0]
        print(f"    {name} as recorded: the look's status has cuts {st.get('cuts')}, cut_group {st.get('cut_group')}, "
              f"age_ms {st.get('age_ms')}, seconds {st.get('seconds')}; its row names cut {row[2]} of group {row[3]}")


if __name__ == '__main__':
    main()
