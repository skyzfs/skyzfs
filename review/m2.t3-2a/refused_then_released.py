#!/usr/bin/env python3
"""#103 at d905bd4: a write the daemon refuses while the cut is still held, and the holder.

    refused_then_released.py KERNEL_TESTS_DIR [RECORDING_DIR NAME DEV_BYTES OUT_DIR]

`Holder.step` sends `release N` after every write, refused or not (cut_hold.py:204 to :207).
The daemon refuses a write for more reasons than the cut having gone: the rig's image or its
`.blocks` file cannot be opened or written (cut_commands.rs, `begin` and `slice`). The cut is
then still the one being uploaded, and the release lets it go unwritten.

Part 1, against the pull request's own fake daemon, changed in one place: the write of cut 1 is
refused as for a full disk and the cut stays held. Prints what the holder wrote and what is
left held.

Part 2, with a recording of the run: the second cut the holder wrote is taken out of held.tsv
and held-blocks.tsv, as if its write had been refused so and the cut released. Prints what the
judge says of the daemon.
"""
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

KDIR = sys.argv[1]
sys.path.insert(0, KDIR)
import cut_hold as ch  # noqa: E402
import test_cut_hold as t  # noqa: E402

OLD = ("            reply(f'{token} refused: cut {n} is no longer held after 0 of {len(blocks)} blocks\\n')\n"
       "            del cuts[n]\n")
NEW = "            reply(f'{token} refused: {path}: write block 0: No space left on device (os error 28)\\n')\n"
assert t.FAKE.count(OLD) == 1
fake = t.FAKE.replace(OLD, NEW)

d = tempfile.mkdtemp()
try:
    vrun, out, image = os.path.join(d, 'vrun'), os.path.join(d, 'out'), os.path.join(d, 'image')
    os.makedirs(vrun)
    os.makedirs(out)
    with open(image, 'wb') as f:
        f.truncate(16 * 4096)
    cuts = {'1': [5, 'itself', {'0': 1}], '2': [6, 'itself', {'1': 3}]}
    proc = subprocess.Popen([sys.executable, '-I', '-c', fake, vrun, json.dumps(cuts), json.dumps([1])])
    while not os.path.exists(os.path.join(vrun, 'ready')):
        time.sleep(0.01)
    log = io.StringIO()
    holder = ch.Holder(ch.Daemon(proc.pid, vrun, log), image, out)
    error = None
    try:
        while holder.step():
            pass
        left = holder.held()
    except ch.HolderError as e:
        error, left = e, None
    print('part 1: the fake holds cuts 1 and 2; the write of cut 1 is refused for a full disk, and cut 1 stays held')
    print(f'    the holder wrote cuts {holder.written}; left held: {left}; the holder\'s error: {error}')
    for ln in log.getvalue().splitlines():
        print('    ' + ln.split('\t', 1)[1][:150])
    open(os.path.join(vrun, 'stop'), 'w').close()
    proc.wait(timeout=10)
finally:
    shutil.rmtree(d)

if len(sys.argv) > 2:
    rdir, name, dev, outdir = sys.argv[2:6]
    if os.path.exists(outdir):
        shutil.rmtree(outdir)
    shutil.copytree(rdir, outdir)
    held = open(os.path.join(outdir, 'held.tsv')).read().splitlines()
    gone = held[1].split('\t')[0]
    with open(os.path.join(outdir, 'held.tsv'), 'w') as f:
        f.write(''.join(ln + '\n' for k, ln in enumerate(held) if k != 1))
    kept = [ln for ln in open(os.path.join(outdir, 'held-blocks.tsv')) if ln.split('\t')[0] != gone]
    with open(os.path.join(outdir, 'held-blocks.tsv'), 'w') as f:
        f.write(''.join(kept))
    p = subprocess.run([sys.executable, '-I', os.path.join(KDIR, 'cut_daemon_check.py'), name, outdir,
                        os.path.join(outdir, 'requests.tsv'), '-', dev], capture_output=True, text=True)
    print(f'part 2: {name} with cut {gone} released unwritten: the judge exits {p.returncode} '
          f'(1 is a verdict against the daemon, 2 the rig\'s error)')
    for ln in p.stdout.splitlines():
        if 'held_content' in ln:
            print('    ' + ln[:330])
