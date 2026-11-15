#!/usr/bin/env python3
"""#103 at d905bd4: plants in the content judge (tests/kernel/cut_daemon_check.py) and the holder
(tests/kernel/cut_hold.py), one at a time, against the rig's unit tests. The file is restored
after each.

    plants.py CHECKOUT [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT = sys.argv[1]
ONLY = tuple(sys.argv[2:])
J = 'tests/kernel/cut_daemon_check.py'
H = 'tests/kernel/cut_hold.py'
TESTS = ['tests/kernel/test_cut_daemon_check.py', 'tests/kernel/test_cut_daemon_main.py', 'tests/kernel/test_cut_hold.py']

PLANTS = [
    # the model
    ('J1 a write sets the block after the one it touches', J,
     "            model[b] = hs[k] if hs is not None else None", "            model[b + 1] = hs[k] if hs is not None else None"),
    ('J2 a discard reads as zeros alone', J,
     "            model[b] = None if cur is None else (cur | {ZERO} if isinstance(cur, frozenset) else frozenset({cur, ZERO}))",
     "            model[b] = ZERO"),
    ('J3 a discard changes nothing in the model', J,
     "            model[b] = None if cur is None else (cur | {ZERO} if isinstance(cur, frozenset) else frozenset({cur, ZERO}))",
     "            model[b] = cur"),
    ('J4 a write-zeroes changes nothing in the model', J,
     "        for b in whole:\n            model[b] = ZERO\n", "        for b in whole:\n            pass\n"),
    ('J5 a block a write covers in part is taken as the block before the write', J,
     "            model[b] = hs[k] if hs is not None else None",
     "            model[b] = hs[k] if hs is not None and hs[k] is not None else model.get(b, ZERO)"),
    ('J6 a block discarded in part stays known', J,
     "            model[b] = None if cur is None else (cur | {ZERO} if isinstance(cur, frozenset) else frozenset({cur, ZERO}))\n"
     "        for b in part:\n            model[b] = None\n",
     "            model[b] = None if cur is None else (cur | {ZERO} if isinstance(cur, frozenset) else frozenset({cur, ZERO}))\n"
     "        for b in part:\n            pass\n"),
    ('J7 a block zeroed in part stays known', J,
     "            model[b] = ZERO\n        for b in part:\n            model[b] = None\n",
     "            model[b] = ZERO\n        for b in part:\n            pass\n"),
    # the comparison
    ('J8 a block of either content is always right', J,
     "    if isinstance(expected, frozenset):\n        return got in expected\n",
     "    if isinstance(expected, frozenset):\n        return True\n"),
    ('J9 only the blocks the image holds are compared', J,
     "    for b in set(model) | set(image):\n        e = model.get(b, ZERO)\n        v = same(",
     "    for b in set(image):\n        e = model.get(b, ZERO)\n        v = same("),
    ('J10 only the blocks the reference wrote are compared', J,
     "    for b in set(model) | set(image):\n        e = model.get(b, ZERO)\n        v = same(",
     "    for b in set(model):\n        e = model.get(b, ZERO)\n        v = same("),
    ('J11 a block the reference does not know counts as right and compared', J,
     "        if v is None:\n            unknown += 1\n            continue\n", ""),
    # the cut's instant and the image
    ('J12 the reference is played one record past the barrier', J,
     "        while at < d['prefix']:\n            apply(model, records[at])",
     "        while at < d['prefix'] + 2:\n            apply(model, records[at])"),
    ('J13 the reference is played to the cut before', J,
     "        while at < d['prefix']:\n            apply(model, records[at])",
     "        while at < (dcuts[by_number[n] - 1]['prefix'] if by_number[n] else 0):\n            apply(model, records[at])"),
    ('J14 the image does not accumulate: each cut is its own map alone', J,
     "        image.update(blocks[n])\n", "        image = dict(blocks[n])\n"),
    ('J15 a wrong block is not a verdict', J,
     "        if wrong:\n            shown = [x for x in wrong if x is not None]\n            bad.append(",
     "        if wrong:\n            shown = [x for x in wrong if x is not None]\n            notes.append("),
    ('J16 the held group and answer are not compared with the cut line', J,
     "        if (w['group'], w['opens'] == 'itself') != (d['txg'], d['itself']):",
     "        if False and (w['group'], w['opens'] == 'itself') != (d['txg'], d['itself']):"),
    ('J17 the held answer (itself or rewind) is not compared', J,
     "        if (w['group'], w['opens'] == 'itself') != (d['txg'], d['itself']):",
     "        if w['group'] != d['txg']:"),
    ('J18 the last cut need not be written', J,
     "    if written[-1]['number'] != last:", "    if False and written[-1]['number'] != last:"),
    ('J19 the image after the drain is not looked at', J,
     "    if final is not None:\n        stray = [", "    if final is not None and False:\n        stray = ["),
    ('J20 no cut written is no error', J,
     "    if not written:\n        raise RigError('the holder wrote no cut')\n", "    if not written:\n        return True, 'no cut written', {c: ('caught', '') for c in CONTENT_CONTROLS}\n"),
    ('J21 a control with nothing wrong counts as caught', J,
     "        controls[c] = ('caught', f'cut {mid}: {len(wrong)} blocks differ') if wrong \\\n",
     "        controls[c] = ('caught', f'cut {mid}: {len(wrong)} blocks differ') if wrong is not None \\\n"),
    ('J22 content_stale compares the image after the write', J,
     "            ctl['content_stale'] = held_compare(model, prev_image)[3]", "            ctl['content_stale'] = held_compare(model, image)[3]"),
    ('J23 content_early compares with the reference at the instant', J,
     "            ctl['content_early'] = held_compare(early, image)[3]", "            ctl['content_early'] = held_compare(model, image)[3]"),
    # the lines
    ('J24 an ERROR in the holder\'s log is not looked for', J,
     "    if err:\n        raise RigError(f\"the holder:", "    if err and False:\n        raise RigError(f\"the holder:"),
    ('J25 the state need not be begun', J,
     "    ok = said == ['begun'] and st == 'begun'", "    ok = True"),
    ('J26 the state is judged by the status alone', J,
     "    ok = said == ['begun'] and st == 'begun'", "    ok = st == 'begun'"),
    ('J27 the state is judged by the log alone', J,
     "    ok = said == ['begun'] and st == 'begun'", "    ok = said == ['begun']"),
    ('J28 cuts_held is not compared with the list', J,
     "    ok = sh.get('cuts_held') == str(len(final))", "    ok = True"),
    ('J29 a refused held at the stop is not the rig\'s error', J,
     "    if any(ln.startswith('refused') for ln in final):", "    if False and any(ln.startswith('refused') for ln in final):"),
    ('J30 a wrong content with a write-zeroes in the log is a verdict', J,
     "    if not ok and any(r.get('op') == 'write_zeroes' for r in records):", "    if False:"),
    ('J31 a wrong content is the rig\'s error whatever the log holds', J,
     "    if not ok and any(r.get('op') == 'write_zeroes' for r in records):", "    if not ok:"),
    ('J32 a control that differs from its expectation is no verdict', J,
     "        rc = rc if good else 1\n    return rc, errored", "    return rc, errored"),
    ('J33 a failed held_content is no verdict', J,
     "        put('held_content', 'PASS' if ok else 'FAIL', detail)\n        rc = rc if ok else 1\n",
     "        put('held_content', 'PASS' if ok else 'FAIL', detail)\n"),
    ('J34 the holder\'s lines are not judged though it ran', J,
     "        if os.path.exists(os.path.join(rdir, 'holder.log')):", "        if False:"),
    ('J35 a discard is not put among the records', J,
     "                add({'flags': ['trim'], 'offset': off, 'bytes': n})", "                pass"),
    ('J36 the hashes of a write are not checked against its length', J,
     "                if len(hs) != len(range(off // BLOCK, (off + n - 1) // BLOCK + 1)) \\\n"
     "                        or any(", "                if any("),
    # the holder
    ('H1 a reply is taken for the token by its beginning alone', H,
     "    if not lines or not lines[-1].startswith(token + ' '):", "    if not lines or not lines[-1].startswith(token):"),
    ('H2 a reply is read by its first line', H,
     "    last = lines[-1][len(token) + 1:]", "    last = lines[0][len(token) + 1:] if lines[0].startswith(token) else lines[-1][len(token) + 1:]"),
    ('H3 two signals for each command', H,
     "            os.kill(self.pid, signal.SIGUSR2)\n", "            os.kill(self.pid, signal.SIGUSR2)\n            os.kill(self.pid, signal.SIGUSR2)\n"),
    ('H4 a refused write is recorded all the same', H,
     "        if ok:\n            blocks = read_blocks(", "        if True:\n            blocks = read_blocks("),
    ('H5 the cut that waits is written when none is being uploaded', H,
     "        up = [c for c in self.held() if c['phase'] == 'uploading']", "        up = [c for c in self.held() if c['phase'] != 'flushing']"),
    ('H6 a written cut is not released', H,
     "        ok, _, last = self.d.send(f\"release {c['number']}\")\n", ""),
    ('H7 a block is hashed at the next block\'s place', H,
     "            f.seek(b * BLOCK)\n", "            f.seek((b + 1) * BLOCK)\n"),
    ('H8 the image\'s zero blocks are listed too', H,
     "                if h != ZERO:\n", "                if True:\n"),
    ('H9 a reply that never comes is waited for without end', H,
     "            if now >= end:\n", "            if False:\n"),
    ('H10 the volume\'s run directory is named in decimal', H,
     "f'v-{st.st_dev:x}-{st.st_ino:x}'", "f'v-{st.st_dev}-{st.st_ino}'"),
    ('H11 a held line with a phase the daemon does not have is taken', H,
     "                or f[2] not in ('uploading', 'waiting', 'flushing') or f[3] not in ('itself', 'rewind'):",
     "                or f[3] not in ('itself', 'rewind'):"),
    ('H12 the command is written in place, not renamed', H,
     "        tmp = os.path.join(self.vrun, 'cut-command.tmp')", "        tmp = os.path.join(self.vrun, 'cut-command')"),
    ('H13 a daemon that is gone is not noticed', H,
     "            raise HolderError(f'the daemon {self.pid} is gone') from None", "            pass"),
    ('H14 the hash is of the whole SHA-256', H,
     "    return hashlib.sha256(data).digest()[:8].hex()", "    return hashlib.sha256(data).digest().hex()"),
    ('H15 a block past the image\'s end is hashed as what was read', H,
     "            if len(data) != BLOCK:", "            if False:"),
]


def main():
    cache = {}
    for name, path, *pairs in PLANTS:
        if ONLY and not name.startswith(ONLY):
            continue
        full = os.path.join(ROOT, path)
        orig = cache.setdefault(full, open(full).read())
        counts = [orig.count(old) for old in pairs[0::2]]
        if any(c != 1 for c in counts):
            print(f'{name}: NOT APPLIED (found {counts} times)', flush=True)
            continue
        text = orig
        for old, new in zip(pairs[0::2], pairs[1::2]):
            text = text.replace(old, new)
        try:
            with open(full, 'w') as f:
                f.write(text)
            failed = []
            hung = False
            for t in TESTS:
                try:
                    p = subprocess.run([sys.executable, t], cwd=ROOT, capture_output=True, text=True, timeout=180)
                except subprocess.TimeoutExpired:
                    hung = True
                    failed.append(f'{os.path.basename(t)} did not end in 180 s')
                    continue
                failed += sorted({ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL: ', 'ERROR: '))})
                if p.returncode != 0 and not any(ln.startswith(('FAIL: ', 'ERROR: ')) for ln in p.stderr.splitlines()):
                    failed.append(f'{os.path.basename(t)} exit {p.returncode}')
            if failed:
                print(f'{name}: KILLED by {len(failed)}: {", ".join(failed[:6])}', flush=True)
            else:
                print(f'{name}: SURVIVED', flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)


if __name__ == '__main__':
    main()
