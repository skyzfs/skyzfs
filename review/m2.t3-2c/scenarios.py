#!/usr/bin/env python3
"""#113 at 98024d5: the recording `failed` of a run's artifact, changed by
hand into what a daemon with a defect (or with the other counting rule)
would leave, and run through that head's judge. Each scenario works on a
copy; the artifact is not changed.

    python3 -I scenarios.py CHECKOUT ARTIFACT_DIR...
"""
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile

root, arts = sys.argv[1], sys.argv[2:]
sys.path.insert(0, os.path.join(root, 'tests/kernel'))
import cut_daemon_check as cdc  # noqa: E402

JUDGE = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
ZERO = hashlib.sha256(bytes(4096)).digest()[:8].hex()
REGION = 1024       # blocks of an intent region of 4 MiB
SHOWN = ('cuts', 'cut_numbers', 'status', 'held_reads', 'cut_state', 'held_content', 'instance_1', 'instance_2',
         'failure', 'rig_error')


def rd(d, name):
    with open(os.path.join(d, name)) as f:
        return f.read()


def wr(d, name, text):
    with open(os.path.join(d, name), 'w') as f:
        f.write(text)


def judge(d):
    if os.path.exists(os.path.join(d, 'results.tsv')):
        os.remove(os.path.join(d, 'results.tsv'))
    p = subprocess.run([sys.executable, '-I', '-B', JUDGE, 'failed', d, os.path.join(d, 'requests.tsv'), '-',
                        str(512 << 20)], capture_output=True, text=True, timeout=1500)
    lines = {}
    for ln in rd(d, 'results.tsv').splitlines():
        f = ln.split('\t')
        lines[f[0]] = (f[1], f[2])
    return p.returncode, lines


def show(title, rc, lines, keys=SHOWN):
    print(f'  {title}: exit {rc}')
    for k in keys:
        if k in lines and (lines[k][0] not in ('PASS', 'INFO') or k in ('cut_numbers', 'status')):
            print(f'      {k}\t{lines[k][0]}\t{lines[k][1][:330]}')


def sections(d):
    """held.tsv's lines and each one's block lines of held-blocks.tsv, in order."""
    held = [ln.split('\t') for ln in rd(d, 'held.tsv').splitlines()]
    rows = rd(d, 'held-blocks.tsv').splitlines()
    out, at = [], 0
    for h in held:
        n = int(h[3])
        out.append((h, [r.split('\t') for r in rows[at:at + n]]))
        at += n
    assert at == len(rows)
    return out


def put_sections(d, secs):
    wr(d, 'held.tsv', ''.join('\t'.join(h[:3] + [str(len(rows))] + h[4:]) + '\n' for h, rows in secs))
    wr(d, 'held-blocks.tsv', ''.join('\t'.join(r) + '\n' for _, rows in secs for r in rows))
    image = {}
    for _, rows in secs:
        for _, b, h in rows:
            image[int(b)] = h
    wr(d, 'held-image.tsv', ''.join(f'{b}\t{h}\n' for b, h in sorted(image.items()) if h != ZERO))


def image_before(secs, k):
    image = {}
    for _, rows in secs[:k]:
        for _, b, h in rows:
            image[int(b)] = h
    return image


def renumber(d, by, pid2, first2):
    """The second state's cut lines, held lines and block lines carry their number + by."""
    out = []
    for ln in rd(d, 'requests.tsv').splitlines():
        f = ln.split('\t')
        if f[1] == pid2 and f[2] == 'cut':
            f[6] = str(int(f[6]) + by)
            ln = '\t'.join(f)
        out.append(ln)
    wr(d, 'requests.tsv', '\n'.join(out) + '\n')
    secs = sections(d)
    for h, rows in secs[first2:]:
        h[0] = str(int(h[0]) + by)
        for r in rows:
            r[0] = str(int(r[0]) + by)
    put_sections(d, secs)


def sub(d, name, old, new, count=1):
    text = rd(d, name)
    assert text.count(old) >= 1, (name, old)
    wr(d, name, text.replace(old, new, count))


for art in arts:
    src = os.path.join(art, 'failed')
    tmp = tempfile.mkdtemp(prefix='rv-scen-')
    print(f'== {os.path.basename(art)}')
    try:
        def copy(name):
            d = os.path.join(tmp, name)
            shutil.copytree(src, d)
            return d

        # What the recording is.
        d = copy('s0')
        inst = cdc.parse_log(rd(d, 'requests.tsv'))
        pid1, pid2, start2 = inst[0]['pid'], inst[1]['pid'], inst[1]['ns']
        n1 = sum(1 for ln in inst[0]['lines'] if ln['event'] == 'cut')
        n2 = sum(1 for ln in inst[1]['lines'] if ln['event'] == 'cut')
        secs = sections(d)
        first2 = next(k for k, (h, _) in enumerate(secs) if int(h[4]) > start2)
        orig_image = rd(d, 'held-image.tsv')
        put_sections(d, secs)
        assert rd(d, 'held-image.tsv') == orig_image, 'the replay of held-blocks.tsv is not held-image.tsv'
        before = image_before(secs, first2)
        h, rows = secs[first2]
        differ = [r for r in rows if before.get(int(r[1]), ZERO) != r[2]]
        by_region = {}
        for r in rows:
            by_region.setdefault(int(r[1]) // REGION, [0, 0])[0] += 1
        for r in differ:
            by_region[int(r[1]) // REGION][1] += 1
        whole = {g: v for g, v in by_region.items() if v[0] == REGION}
        print(f'  the recording: state 1 counted {n1} cuts and its holder wrote '
              f"{[int(x[0][0]) for x in secs[:first2]]}; state 2 counted {n2} and its holder wrote "
              f'{[int(x[0][0]) for x in secs[first2:]]}')
        print(f"  the new state's first written cut is its cut {h[0]} (group {h[1]}), {len(rows)} blocks; "
              f'{len(differ)} of them differ from the image the first state\'s holder left (what a cut that '
              f'lacked them would get wrong); whole regions in its map: '
              f"{', '.join(f'region {g}: {v[1]} of {v[0]} differ' for g, v in sorted(whole.items()))}; "
              f'outside whole regions: {sum(v[0] for g, v in by_region.items() if g not in whole)} blocks, '
              f'{sum(v[1] for g, v in by_region.items() if g not in whole)} differ')
        rc, lines = judge(d)
        show('S0 as recorded', rc, lines)

        # S1: the first cut after the failure lacks one region the intent map named.
        g = max(whole, key=lambda x: whole[x][1])
        d = copy('s1')
        secs = sections(d)
        secs[first2] = (secs[first2][0], [r for r in secs[first2][1] if int(r[1]) // REGION != g])
        put_sections(d, secs)
        rc, lines = judge(d)
        show(f"S1 the new state's first cut lacks region {g} ({whole[g][1]} blocks that changed)", rc, lines)

        # S2: it lacks one block that changed.
        d = copy('s2')
        secs = sections(d)
        one = differ[len(differ) // 2][1]
        secs[first2] = (secs[first2][0], [r for r in secs[first2][1] if r[1] != one])
        put_sections(d, secs)
        rc, lines = judge(d)
        show(f"S2 the new state's first cut lacks one block that changed (block {one})", rc, lines)

        # S3: it holds only what the new instance wrote itself (the store's
        # mutant INTENT_NOT_CARRIED: the map taken up and none of its regions counted).
        d = copy('s3')
        recs, dcuts = cdc.records_of(cdc.log_lines(inst, {pid2: False}, {pid2}), cdc.read_marks(rd(d, 'marks.tsv')))
        cdc.epochs(dcuts, recs)
        bound = next(r['i'] for r in recs if 'instance' in r['flags'])
        cut = next(c for c in dcuts if c['epoch'] == 1 and c['local'] == secs[first2][0][0])
        own = set()
        for r in recs[bound:cut['prefix']]:
            if not r['flags'] and r['bytes']:
                own |= set(range(r['offset'] // 4096, (r['offset'] + r['bytes'] - 1) // 4096 + 1))
        secs = sections(d)
        kept = [r for r in secs[first2][1] if int(r[1]) in own]
        lost = sum(1 for r in differ if int(r[1]) not in own)
        secs[first2] = (secs[first2][0], kept)
        put_sections(d, secs)
        rc, lines = judge(d)
        show(f"S3 the new state's first cut holds only the {len(kept)} blocks its own instance wrote "
             f'({lost} changed blocks of the regions lost)', rc, lines)

        # S4: the other counting rule: the new state counts on.
        d = copy('s4')
        renumber(d, n1, pid2, first2)
        sub(d, 'status.txt', f'cuts {n2}\n', f'cuts {n1 + n2}\n')
        rc, lines = judge(d)
        show(f'S4 the new state counts on ({n1 + 1}..{n1 + n2}), the status says cuts {n1 + n2}', rc, lines)

        # S5: counts on, and the status gives the new state's count alone.
        d = copy('s5')
        renumber(d, n1, pid2, first2)
        rc, lines = judge(d)
        show(f'S5 the new state counts on, the status says cuts {n2}', rc, lines)

        # S6: counts from 1, and the status gives the total.
        d = copy('s6')
        sub(d, 'status.txt', f'cuts {n2}\n', f'cuts {n1 + n2}\n')
        rc, lines = judge(d)
        show(f'S6 the new state counts from 1, the status says cuts {n1 + n2}', rc, lines)

        # S7: neither rule: the new state begins at 5.
        d = copy('s7')
        renumber(d, 4, pid2, first2)
        sub(d, 'status.txt', f'cuts {n2}\n', f'cuts {n2 + 4}\n')
        rc, lines = judge(d)
        show('S7 the new state begins at 5 (neither rule)', rc, lines)

        # S8: the defect the stage pins, as the unit test has it: the cuts
        # line and the status say taken_up, nothing else differs.
        d = copy('s8')
        sub(d, 'requests.tsv', f'\t{pid2}\tcuts\t-\t-\t-\t-\tbegun\n', f'\t{pid2}\tcuts\t-\t-\t-\t-\ttaken_up\n')
        sub(d, 'status-start-2.txt', 'cut_state begun\n', 'cut_state taken_up\n')
        sub(d, 'status.txt', 'cut_state begun\n', 'cut_state taken_up\n')
        rc, lines = judge(d)
        show('S8 the acknowledged start says taken_up, and nothing else differs', rc, lines)

        # S9: the same defect as a daemon would leave it: the state taken
        # up, so the count goes on and the cut that was being uploaded at
        # the failure is still held; the new holder writes it first.
        d = copy('s9')
        renumber(d, n1, pid2, first2)
        sub(d, 'status.txt', f'cuts {n2}\n', f'cuts {n1 + n2}\n')
        sub(d, 'requests.tsv', f'\t{pid2}\tcuts\t-\t-\t-\t-\tbegun\n', f'\t{pid2}\tcuts\t-\t-\t-\t-\ttaken_up\n')
        sub(d, 'status-start-2.txt', 'cut_state begun\n', 'cut_state taken_up\n')
        sub(d, 'status.txt', 'cut_state begun\n', 'cut_state taken_up\n')
        secs = sections(d)
        last1 = int(secs[first2 - 1][0][0])
        old = next(ln for ln in inst[0]['lines'] if ln['event'] == 'cut' and int(ln['sectors']) == last1 + 1)
        asked = int(secs[first2][0][4]) - 1000
        blk = secs[first2][1][0]
        secs.insert(first2, ([str(last1 + 1), old['result'], old['sector'], '1', str(asked), str(asked + 500)],
                             [[str(last1 + 1), blk[1], blk[2]]]))
        put_sections(d, secs)
        rc, lines = judge(d)
        show(f'S9 the acknowledged start took the state up: it counts on and its holder first writes cut '
             f'{last1 + 1} of the first state, held across the failure', rc, lines)
    finally:
        shutil.rmtree(tmp)
