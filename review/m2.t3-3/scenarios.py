#!/usr/bin/env python3
"""#119 at 32d8d67 (M2.T3 stage 3): scenarios for the review's findings.

    scenarios.py CHECKOUT [UFAILED_DIR]

CHECKOUT holds the head's tests/kernel; its judge, watcher and test
helpers are imported or run from it and nothing in it is changed.
UFAILED_DIR is the recording `ufailed` of run 44255421061, unpacked (it
is copied; nothing in it is run).

S1  the judge's upload_status where the bucket has no lease and the
    status names no holder, and where the row's epoch is not the
    status's;
S2  a recording of which node B fetched one cut only;
S3  ufailed as the kernel run left it, and with the old state's one
    fetch missed (the watcher's look fell after the new state's first
    commit): the same daemon, the same uploads;
S4  the balance where the start's look fell between the uploader's
    count of a released cut and the store's (serve.rs posts the store's
    figures before it answers the uploader's release);
S5  the watcher beside a node B whose `get` fails every time with 2 (a
    store error, a full disk), and the judge on what it leaves;
S6  umoved where a right daemon uploads one more cut between the rig's
    look at the row and the other writer's head.
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
import time

ROOT = sys.argv[1]
UFAILED = sys.argv[2] if len(sys.argv) > 2 else None
K = os.path.join(ROOT, 'tests', 'kernel')
sys.dont_write_bytecode = True
sys.path.insert(0, K)
import cut_daemon_check as cdc  # noqa: E402
import test_cut_daemon_main as tm  # noqa: E402

STATE, HEAD = tm.STATE, 'cd' * 32


def show(title, rc, lines, keys):
    print(f'{title}: exit {rc}')
    for k in keys:
        if k in lines:
            print(f'    {k}\t{lines[k][0]}\t{lines[k][1][:300]}')


def s1():
    print('S1 the holder the judge wants is whatever the last row says')
    none = lambda w, b, st: (w, b, st.replace('lease_holder runner', 'lease_holder none'),  # noqa: E731
                             {'rows.tsv': f'5\t{HEAD} 7\t{STATE} 1 4\tnone\n'})
    rc, lines = tm.recording(upload=none)
    show('  no lease row in the bucket, lease held and lease_holder none in the status', rc, lines, ['upload_status'])
    epoch = lambda w, b, st: (w, b, st, {'rows.tsv': f'5\t{HEAD} 7\t{STATE} 1 4\trunner 7 99\n'})  # noqa: E731
    rc, lines = tm.recording(upload=epoch)
    show("  the bucket's lease at epoch 7, the status's lease_epoch 1", rc, lines, ['upload_status'])


def s2():
    print('S2 one fetch')

    def one(written, blocks, st):
        acc = {}
        for x in written:
            acc.update(blocks[x['number']])
        last = written[-1]
        blocks[last['number']] = acc
        return [dict(last, blocks=len([h for h in acc.values() if h != cdc.ZERO]))], blocks, st
    rc, lines = tm.recording(upload=one)
    show('  node B fetched the last cut and no other', rc, lines, ['upload_content', 'upload_status'])


def judge(d, name):
    try:
        os.remove(os.path.join(d, 'results.tsv'))
    except OSError:
        pass
    p = subprocess.run([sys.executable, '-I', '-B', os.path.join(K, 'cut_daemon_check.py'), name, d,
                        os.path.join(d, 'requests.tsv'), '-', str(512 << 20)], capture_output=True, text=True,
                       timeout=1200)
    lines = {}
    with open(os.path.join(d, 'results.tsv')) as f:
        for ln in f:
            key, verdict, detail = ln.rstrip('\n').split('\t', 2)
            lines[key] = (verdict, detail)
    return p.returncode, lines


def s3():
    print('S3 ufailed, run 44255421061 (daemon e42ea6a; its lease_holder none fails upload_status either way)')
    if not UFAILED:
        print('  no recording given')
        return
    tmp = tempfile.mkdtemp(prefix='s3-')
    try:
        d = os.path.join(tmp, 'ufailed')
        shutil.copytree(UFAILED, d)
        rc, lines = judge(d, 'ufailed')
        show('  as recorded', rc, lines, ['upload_content'])
        with open(os.path.join(d, 'uploaded.tsv')) as f:
            got = f.read().splitlines()
        image = {}
        with open(os.path.join(d, 'uploaded-blocks.tsv')) as f:
            for ln in f:
                k, n, b, h = ln.rstrip('\n').split('\t')
                image[int(b)] = h
        number = got[1].split('\t')[0]
        whole = {b: h for b, h in image.items() if h != cdc.ZERO}
        with open(os.path.join(d, 'uploaded.tsv'), 'w') as f:
            f.write(got[1] + '\n')
        with open(os.path.join(d, 'uploaded-blocks.tsv'), 'w') as f:
            f.write(''.join(f'0\t{number}\t{b}\t{h}\n' for b, h in sorted(whole.items())))
        rc, lines = judge(d, 'ufailed')
        show("  the same recording, the watcher's fetch of the old state's cut 10 missed", rc, lines, ['upload_content'])
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def s4():
    print("S4 the balance, ukill's second instance (figures of run 44253158409, cuts_replaced as it must have been)")
    end = dict(cuts='466', uploaded='9', upload_found_committed='1', upload_committed_gone='0',
               upload_committed_kept='0', cuts_replaced='115', cuts_abandoned='0', cuts_held='0', upload_state='idle')
    start = dict(cuts='344', uploaded='0', upload_found_committed='1', upload_committed_gone='0',
                 upload_committed_kept='0', cuts_replaced='0', cuts_abandoned='0', cuts_held='2',
                 upload_state='reading')
    print('  the look after the release reached the board whole:', cdc.upload_balance(start, end))
    torn = dict(start, cuts_held='3')
    print("  the look with the uploader's upload_found_committed 1 and the store's cuts_held not yet one less:",
          cdc.upload_balance(torn, end))


NODEB = textwrap.dedent('''\
    #!/usr/bin/env python3
    import sys
    if sys.argv[1] == 'row':
        sys.stdout.write('head\\t' + 'cd' * 32 + '\\t3\\nrow\\t' + 'ab' * 16 + '\\t5\\t40\\nlease\\trunner\\t1\\t99\\n')
    else:
        sys.stderr.write('write watch.img: No space left on device\\n')
        sys.exit(2)
''')


def s5():
    print('S5 a node B whose get always ends with 2')
    tmp = tempfile.mkdtemp(prefix='s5-')
    try:
        out = os.path.join(tmp, 'out')
        os.makedirs(out)
        nodeb = os.path.join(tmp, 'nodeb')
        with open(nodeb, 'w') as f:
            f.write(NODEB)
        os.chmod(nodeb, 0o755)
        stop = os.path.join(tmp, 'stop')
        p = subprocess.Popen([sys.executable, '-I', '-B', os.path.join(K, 'cut_watch.py'), out, nodeb, 'http://x', 'b',
                              'v', stop])
        time.sleep(4)
        with open(stop, 'w'):
            pass
        rc = p.wait(timeout=120)
        with open(os.path.join(out, 'watch.log')) as f:
            log = [ln.split('\t', 1)[1].rstrip('\n') for ln in f]
        fails = sum(1 for ln in log if 'node B get ended with 2' in ln)
        print(f'  the watcher: exit {rc}; {fails} failed gets logged, uploaded.tsv '
              f"{'written' if os.path.exists(os.path.join(out, 'uploaded.tsv')) else 'not written'}; "
              f'its last line: {log[-1]!r}; ERROR lines: {sum(1 for ln in log if ln.startswith("ERROR"))}')
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    rc, lines = tm.recording(upload=lambda w, b, st: ([], {}, st))
    show('  the judge on a recording whose watcher fetched nothing', rc, lines, ['upload_reads', 'upload_content'])


def s6():
    print('S6 umoved, one more cut uploaded before the other writer acted')
    other = 'ee' * 32

    def up(late):
        def f(written, blocks, st):
            m = len(written) // 2
            # The rig read the row at cut m (moved-row.txt, moved-status.txt);
            # with late, the daemon uploaded cut m + 1 before move-head landed.
            n = m + 1 + (1 if late else 0)
            kept = written[:n]
            named, last = written[m], kept[-1]
            o = dict(last, head=other, key='other', blocks=last['blocks'] + 1)
            blocks['other'] = {0: '5a' * 8}
            st = (st.replace('upload_mismatches 0', 'upload_mismatches 1')
                    .replace('upload_abandoned_at_mismatch 0', 'upload_abandoned_at_mismatch 2')
                    .replace('upload_checked matched', 'upload_checked head_moved'))
            st = re.sub(r'uploaded \d+', f'uploaded {n}', st)
            cuts = int(re.search(r'^cuts (\d+)$', st, flags=re.M).group(1))
            st = st.replace('cuts_abandoned 0', f'cuts_abandoned {cuts - n}')
            files = {'moved-row.txt': tm.row_file((STATE, named['number'], named['group'])),
                     'moved-status.txt': f'uploaded {m + 1}\n',
                     'end-row.txt': tm.row_file((STATE, last['number'], last['group']), head=other),
                     'others.tsv': f'head\t{other}\n'}
            return kept + [o], blocks, st, files
        return f
    keys = ['upload_content', 'upload_status', 'upload_balance', 'upload_course']
    rc, lines = tm.recording(upload=up(False), as_name='umoved')
    show('  the other writer acted on the cut the rig read', rc, lines, keys)
    rc, lines = tm.recording(upload=up(True), as_name='umoved')
    show('  a right daemon uploaded one more cut first', rc, lines, keys)


if __name__ == '__main__':
    for s in (s1, s2, s3, s4, s5, s6):
        s()
        print()
