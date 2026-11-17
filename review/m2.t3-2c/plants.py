#!/usr/bin/env python3
"""#113 at 98024d5: plants in the stage's new logic of the judge
(tests/kernel/cut_daemon_check.py), the reference (cut_ref.py) and the
holder (cut_hold.py), one at a time, against the four unit test files of
the changed code. The file is restored after each, and each result is
written to OUT as it comes.

    plants.py CHECKOUT OUT [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT, OUT = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
J = 'tests/kernel/cut_daemon_check.py'
R = 'tests/kernel/cut_ref.py'
H = 'tests/kernel/cut_hold.py'
TESTS = ['tests/kernel/test_cut_daemon_check.py', 'tests/kernel/test_cut_daemon_main.py',
         'tests/kernel/test_cut_hold.py', 'tests/kernel/test_cut_ref.py']

EPOCH = "        d['epoch'] = sum(1 for r in starts if r['i'] < d['prefix'])\n"
RENUM = "        if starts and d['number'] != '-':\n            d['number'] = str(k + 1)\n"
ONE = "            if n == '1':\n                k0 = k\n"
NUM = "        if n != str(k - k0 + 1):\n"
KEYED = "        e = sum(1 for r in starts if r['ns'] < w['asked'])\n"
ROWS = "        if len(rows[at - w['blocks']:at]) != w['blocks'] or set(part) - {w['number']}:\n"
SINCE = "            if first and first[0]['local'] == '1':\n"
WANT = "        want = 'begun' if begun[k] else 'taken_up'\n        want_intent = 'begun' if k == 0 else 'taken_up'\n"
LAX = "        if k and (committed is None or committed > inst['ns']):\n"
BEFORE = "        before = 0 if begun[k] else sum(1 for d in dcuts if d['prefix'] < bounds[k - 1]['i'])\n"
VERD = "    if bad:\n        return 1, False\n"
CALL = "            if name == 'failed':\n                rc = max(rc, failure_line(put, rdir))\n"

PLANTS = [
    # ---- epochs: the cut lines numbered by their place in the whole log ----
    ('E1 a cut line belongs to the states begun after it (the comparison turned round)', J,
     EPOCH, "        d['epoch'] = sum(1 for r in starts if r['i'] > d['prefix'])\n"),
    ('E2 every cut line is of the first state', J,
     EPOCH, "        d['epoch'] = 0\n"),
    ('E3 the lines keep the numbers they carry (no place in the whole log)', J,
     RENUM, "        if False:\n            d['number'] = str(k + 1)\n"),
    ('E4 every later instance begins an epoch, acknowledged or not', J,
     "    starts = [r for r in records if 'instance' in r['flags'] and r.get('begun')]\n    for k, d in enumerate(dcuts):\n",
     "    starts = [r for r in records if 'instance' in r['flags']]\n    for k, d in enumerate(dcuts):\n"),
    ('E5 local is the place in the whole log too (the number the line carries is lost)', J,
     "        d['local'] = d['number']\n        if starts", "        d['local'] = str(k + 1)\n        if starts"),
    # ---- numbers_verdict: either counting rule, and nothing else ----
    ('N1 a new state must count from 1 again (going on is refused)', J,
     ONE, "            if True:\n                k0 = k\n"),
    ('N2 a new state must go on (from 1 again is refused)', J,
     ONE, "            if False:\n                k0 = k\n"),
    ('N3 a new state\'s numbers are not looked at', J,
     NUM, "        if e == 0 and n != str(k - k0 + 1):\n"),
    ('N4 a new state may begin at any number and count on from it', J,
     "            else:\n                said.append(f'state {e + 1} counts on')\n",
     "            else:\n                k0 = k + 1 - int(n) if n.isdigit() else k0\n                said.append(f'state {e + 1} counts on')\n"),
    ('N5 only the first line of a new state is looked at', J,
     NUM, "        if (e == 0 or dcuts[k - 1].get('epoch', 0) != e) and n != str(k - k0 + 1):\n"),
    ('N6 the line does not say which way a state counted', J,
     "    return 'PASS', f'the {len(nums)} cut lines are numbered 1 to {len(nums)}' + (\n        f\" ({'; '.join(said)})\" if said else '')\n",
     "    return 'PASS', f'the {len(nums)} cut lines are numbered 1 to {len(nums)}'\n"),
    ('N7 the numbers read are the places in the whole log, not what the lines carry', J,
     "    nums = [d.get('local', d['number']) for d in dcuts]\n", "    nums = [d['number'] for d in dcuts]\n"),
    # ---- held_by_epoch: written cuts keyed by the state ----
    ('H1 a written cut is of the states begun after its write was asked for (turned round)', J,
     KEYED, "        e = sum(1 for r in starts if r['ns'] > w['asked'])\n"),
    ('H2 every written cut is of the first state', J,
     KEYED, "        e = 0\n"),
    ('H3 every written cut is of the last state', J,
     KEYED, "        e = len(starts)\n"),
    ('H4 held-blocks.tsv need not hold the cut\'s blocks where held.tsv puts them', J,
     ROWS, "        if False:\n"),
    ('H5 lines of held-blocks.tsv after the last cut are no error', J,
     "    if at != len(rows):\n        raise RigError(f'held-blocks.tsv has", "    if False:\n        raise RigError(f'held-blocks.tsv has"),
    ('H6 a written cut with no cut line in its state is skipped, not the rig\'s error', J,
     "        if n is None:\n            raise RigError(f\"the holder wrote cut {w['number']} of state {e + 1}, which has no cut line in the \"\n                           'request log')\n",
     "        if n is None:\n            continue\n"),
    ('H7 the blocks of a written cut are dropped', J,
     "        blocks[n] = part.get(w['number'], {})\n", "        blocks[n] = {}\n"),
    ('H8 held_lines never keys by state (the numbers as the holder wrote them)', J,
     "    if starts:\n        written, blocks = held_by_epoch(", "    if False:\n        written, blocks = held_by_epoch("),
    ('H9 the blocks are taken by the number alone, wherever they are in the file', J,
     "        part = read_hashes('\\n'.join(rows[at:at + w['blocks']]), 'held-blocks.tsv')\n",
     "        part = read_hashes('\\n'.join(rows), 'held-blocks.tsv')\n",
     ROWS, "        if False:\n"),
    # ---- the status's count ----
    ('S1 the status is held to the whole log\'s count, whichever way the state counted', J,
     SINCE, "            if False:\n"),
    ('S2 the status is held to the new state\'s count, whichever way the state counted', J,
     SINCE, "            if first:\n"),
    ('S3 the status\'s count is the cuts before the new state', J,
     "sum(1 for c in ref if c['prefix'] > since))}", "sum(1 for c in ref if c['prefix'] < since))}"),
    # ---- instance_lines ----
    ('I1 a start that acknowledged a failure wants the state taken up, as any later start', J,
     WANT, "        want = 'begun' if k == 0 else 'taken_up'\n        want_intent = 'begun' if k == 0 else 'taken_up'\n"),
    ('I2 a start that acknowledged a failure wants the intent map begun', J,
     WANT, "        want = 'begun' if begun[k] else 'taken_up'\n        want_intent = 'begun' if begun[k] else 'taken_up'\n"),
    ('I3 a start that acknowledged a failure takes either for the intent map', J,
     LAX, "        if k and (begun[k] or committed is None or committed > inst['ns']):\n"),
    ('I4 the acknowledged start\'s count is held to the cuts before it', J,
     BEFORE, "        before = 0 if k == 0 else sum(1 for d in dcuts if d['prefix'] < bounds[k - 1]['i'])\n"),
    ('I5 the line does not say that the start acknowledged a failure', J,
     "            how += ', acknowledging a failure'\n", "            pass\n"),
    ('I6 main passes no begun list to instance_lines', J,
     "            rc = max(rc, instance_lines(put, rdir, instances, dcuts, records, begun))\n",
     "            rc = max(rc, instance_lines(put, rdir, instances, dcuts, records))\n"),
    # ---- cut_state ----
    ('C1 cut_state wants every later start taken up', J,
     "    want = ['begun' if b else 'taken_up' for b in begun]\n", "    want = ['begun'] + ['taken_up'] * (len(instances) - 1)\n"),
    ('C2 cut_state does not look at the last status', J,
     "    ok = said == want and st == want[-1]\n", "    ok = said == want\n"),
    ('C3 main passes no begun list to held_lines', J,
     "            v, e = held_lines(put, rdir, instances, records, dcuts, status, dev, name, begun)\n",
     "            v, e = held_lines(put, rdir, instances, records, dcuts, status, dev, name)\n"),
    # ---- failure ----
    ('F1 serve may end with any status at the failure', J,
     "    if got['failed_rc'] != '3':\n", "    if False:\n"),
    ('F2 a start without the acknowledgement may end with any status', J,
     "    if got['refused_rc'] != '4':\n", "    if False:\n"),
    ('F3 a start without the acknowledgement that ends with 0 or 4', J,
     "    if got['refused_rc'] != '4':\n", "    if got['refused_rc'] not in ('0', '4'):\n"),
    ('F4 no cut held before the failure is no error of the rig', J,
     "    if not got['cuts_held'].isdigit() or int(got['cuts_held']) < 1:\n", "    if not got['cuts_held'].isdigit():\n"),
    ('F5 a failure.tsv without its keys is no error of the rig', J,
     "    for key in ('cuts_held', 'failed_rc', 'refused_rc'):\n        if key not in got:\n",
     "    for key in ('cuts_held', 'failed_rc', 'refused_rc'):\n        if False:\n"),
    ('F6 main does not judge the failure', J,
     CALL, "            if False:\n                rc = max(rc, failure_line(put, rdir))\n"),
    ('F7 main judges the failure and drops the verdict', J,
     CALL, "            if name == 'failed':\n                failure_line(put, rdir)\n"),
    ('F8 main judges a failure in every recording that held cuts', J,
     CALL, "            if True:\n                rc = max(rc, failure_line(put, rdir))\n"),
    # ---- held_reads ----
    ('V1 a VERDICT of the holder is not looked for', J,
     "    bad = [ln.split('\\tVERDICT ', 1)[1] for ln in log.splitlines() if '\\tVERDICT ' in ln]\n", "    bad = []\n"),
    ('V2 held_reads fails and the check goes on to the lines after it', J,
     VERD, "    if bad:\n        rc = 1\n"),
    ('V3 held_reads fails and is no verdict (rc 0)', J,
     VERD, "    if bad:\n        return 0, False\n"),
    ('V4 a refused read is the rig\'s ERROR line', J,
     VERD, "    if bad:\n        return 0, True\n"),
    # ---- the plumbing of the acknowledgement ----
    ('A1 main reads no acknowledgement from instances.tsv', J,
     "            acked = read_acknowledged(read_file(rdir, 'instances.tsv'))\n", "            acked = set()\n"),
    ('A2 instances.tsv may say anything in its third column', J,
     "                or f[2:] not in ([], ['acknowledged']):\n", "                or False:\n"),
    ('A3 every later instance acknowledged a failure', J,
     "    return {ln.split('\\t')[0] for ln in text.splitlines() if ln.endswith('\\tacknowledged')}\n",
     "    return {ln.split('\\t')[0] for ln in text.splitlines()}\n"),
    ('A4 log_lines marks no instance as begun', J,
     "'taken_over': kinds[inst['pid']], 'begun': inst['pid'] in acked})", "'taken_over': kinds[inst['pid']], 'begun': False})"),
    ('A5 records_of drops the mark from the instance record', J,
     "                 'ns': ln['ns'], 'begun': ln.get('begun', False)})", "                 'ns': ln['ns'], 'begun': False})"),
    ('A6 main\'s begun list has the first instance only', J,
     "        begun = [True] + [r['begun'] for r in records if 'instance' in r['flags']]\n",
     "        begun = [True] + [False for r in records if 'instance' in r['flags']]\n"),
    ('A7 lost_at_kill reads the place in the whole log, not the number the line carries', J,
     "                and after[0].get('local', after[0]['number']) == str(before + 2):", "                and after[0]['number'] == str(before + 2):"),
    # ---- the table ----
    ('T1 failed is not a recording the table knows', J,
     "'bound', 'edges', 'takeover', 'failed')", "'bound', 'edges', 'takeover')"),
    ('T2 failed must be caught by the rule mark-dropped', J,
     "    'mark-dropped': dict({r: 'same' for r in RECS}, takeover='caught'),\n",
     "    'mark-dropped': dict({r: 'same' for r in RECS}, takeover='caught', failed='caught'),\n"),
    ('T3 failed must be the same under txg-alone (no "or caught")', J,
     "takeover=('same', 'caught'),\n                      failed=('same', 'caught')),", "takeover=('same', 'caught'),\n                      failed='caught'),"),
    # ---- cut_ref: a begun state ----
    ('R1 a begun state keeps the mark where the device was taken over', R,
     "            if not keep or r.get('begun'):\n", "            if not keep:\n"),
    ('R2 a begun state keeps the newest cut (the repeat rule compares with it)', R,
     "            if r.get('begun'):\n                newest = None\n", "            if False:\n                newest = None\n"),
    ('R3 every instance record forgets the newest cut', R,
     "            if r.get('begun'):\n                newest = None\n", "            if True:\n                newest = None\n"),
    ('R4 a begun state drops the mark only under the design\'s rule', R,
     "            if not keep or r.get('begun'):\n", "            if not keep or (r.get('begun') and rule == 'design'):\n"),
    # ---- cut_hold: whose refusal ----
    ('K1 every refused write of a cut still held is the daemon\'s', H,
     "READ_REFUSED = re.compile(r'refused: read block \\d+ of cut \\d+: ')", "READ_REFUSED = re.compile(r'refused: ')"),
    ('K2 no refused write is the daemon\'s', H,
     "                              verdict=bool(READ_REFUSED.match(last)))\n", "                              verdict=False)\n"),
    ('K3 the reason may stand anywhere in the refusal (search, not match)', H,
     "                              verdict=bool(READ_REFUSED.match(last)))\n", "                              verdict=bool(READ_REFUSED.search(last)))\n"),
    ('K4 a refused release of a cut still held is the daemon\'s too', H,
     "            self.must_be_gone(c['number'], f'its release was refused ({last})')\n",
     "            self.must_be_gone(c['number'], f'its release was refused ({last})', verdict=True)\n"),
    ('K5 a refused read is a verdict even where the cut is gone', H,
     "        if any(c['number'] == number and c['phase'] == 'uploading' for c in self.held()):\n            raise (HolderVerdict if verdict else HolderError)",
     "        if verdict or any(c['number'] == number and c['phase'] == 'uploading' for c in self.held()):\n            raise (HolderVerdict if verdict else HolderError)"),
    ('K6 main ends with 2 at a verdict', H,
     "            print(f'cut_hold: the daemon: {e}', file=sys.stderr)\n            rc = 1\n", "            print(f'cut_hold: the daemon: {e}', file=sys.stderr)\n            rc = 2\n"),
    ('K7 main logs a verdict as ERROR', H,
     "            log.write(f'{time.monotonic_ns()}\\tVERDICT {e}\\n')\n", "            log.write(f'{time.monotonic_ns()}\\tERROR {e}\\n')\n"),
    ('K8 main does not catch a verdict (a traceback, exit 1, no line in the log)', H,
     "        except HolderVerdict as e:\n", "        except KeyboardInterrupt as e:\n"),
    ('K9 the reason is matched whatever number follows "cut" (the cut asked for or not)', H,
     "READ_REFUSED = re.compile(r'refused: read block \\d+ of cut \\d+: ')", "READ_REFUSED = re.compile(r'refused: read block \\d+ of cut ')"),
]


def main():
    cache = {}
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
    with open(OUT, 'a') as out:
        def say(text):
            print(text, flush=True)
            out.write(text + '\n')
            out.flush()
            os.fsync(out.fileno())
        for name, path, *pairs in PLANTS:
            if ONLY and not name.startswith(ONLY):
                continue
            full = os.path.join(ROOT, path)
            orig = cache.setdefault(full, open(full).read())
            counts = [orig.count(old) for old in pairs[0::2]]
            if any(c != 1 for c in counts) or any(old == new for old, new in zip(pairs[0::2], pairs[1::2])):
                say(f'{name}: NOT APPLIED (found {counts} times)')
                continue
            text = orig
            for old, new in zip(pairs[0::2], pairs[1::2]):
                text = text.replace(old, new)
            try:
                with open(full, 'w') as f:
                    f.write(text)
                failed = []
                for t in TESTS:
                    try:
                        p = subprocess.run([sys.executable, '-B', t], cwd=ROOT, env=env, capture_output=True,
                                           text=True, timeout=300)
                    except subprocess.TimeoutExpired:
                        failed.append(f'{os.path.basename(t)} did not end in 300 s')
                        continue
                    failed += sorted({ln.split()[1] for ln in p.stderr.splitlines()
                                      if ln.startswith(('FAIL: ', 'ERROR: '))})
                    if p.returncode != 0 and not any(ln.startswith(('FAIL: ', 'ERROR: '))
                                                     for ln in p.stderr.splitlines()):
                        failed.append(f'{os.path.basename(t)} exit {p.returncode}')
                if failed:
                    say(f'{name}: KILLED by {len(failed)}: {", ".join(failed[:6])}')
                else:
                    say(f'{name}: SURVIVED')
            finally:
                with open(full, 'w') as f:
                    f.write(orig)


if __name__ == '__main__':
    main()
