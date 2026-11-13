#!/usr/bin/env python3
"""Defects planted at 0a78798 (pull request 66), one at a time, and which
tests of skyzfs-blk (its library tests) and skyzfs-blkcheck fail. Each
file is put back after its plant.

    plants66.py WORKTREE TARGET_DIR [name...]
"""
import os
import re
import subprocess
import sys
import time

W, TARGET = sys.argv[1], sys.argv[2]
CONF = 'crates/skyzfs-blk/src/conformance.rs'
DEVICE = 'bins/skyzfs-blkcheck/src/device.rs'
REPLAY = 'bins/skyzfs-blkcheck/src/replay.rs'
PLANTS = {
    'the driver tells the model of the loss before it accounts what was handed over': (CONF,
        "        self.account(&before)?;\n        self.model.kill(self.harness.in_flight_at_a_loss());\n",
        "        self.model.kill(self.harness.in_flight_at_a_loss());\n        self.account(&before)?;\n"),
    'the driver does not ask the harness for what was answered before the loss': (CONF,
        "        let before = self.harness.completed_before_loss(&mut target);\n",
        "        let before: Vec<Completion> = Vec::new();\n"),
    'the device harness drops the results, as before': (DEVICE,
        "        self.before_loss.append(&mut got);\n", ""),
    'the device harness hands the results over at every asking': (DEVICE,
        "        std::mem::take(&mut target.before_loss)\n", "        target.before_loss.iter().map(|c| Completion { id: c.id, result: c.result.clone(), buf: None }).collect()\n"),
    'the trace still says before_kill': (DEVICE,
        '                "answered_before_kill",\n', '                "before_kill",\n'),
    'the replayer passes over answered_before_kill': (REPLAY,
        "                answered_before.push(id);\n", ""),
    'the replayer reads the old word as the new': (REPLAY,
        '            ("answered_before_kill", Some(id)) if settling => {',
        '            ("answered_before_kill" | "before_kill", Some(id)) if settling => {'),
    'the replayer tells the model of the loss first': (REPLAY,
        "                settling = false;\n                for id in std::mem::take(&mut answered_before) {",
        "                settling = false;\n                model.kill(InFlight::MayHaveCompleted);\n                for id in std::mem::take(&mut answered_before) {"),
}


def run():
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET, CARGO_INCREMENTAL='0')
    t = time.time()
    outs = []
    for args in (['-p', 'skyzfs-blk', '--lib'], ['-p', 'skyzfs-blkcheck']):
        try:
            p = subprocess.run(['cargo', 'test', '-q', '--no-fail-fast', *args], cwd=W, env=env,
                               capture_output=True, text=True, timeout=1500)
        except subprocess.TimeoutExpired:
            return 'HUNG (1500 s)'
        outs.append(p.stdout + p.stderr)
    out = '\n'.join(outs)
    if 'error[' in out or 'could not compile' in out:
        errs = [l for l in out.splitlines() if l.startswith('error')][:3]
        return 'DOES NOT BUILD: ' + ' | '.join(errs)
    failed = sorted(set(re.findall(r'^    (\S+::\S+)$', out, re.M)))
    passed = sum(int(n) for n in re.findall(r'test result: \w+\. (\d+) passed', out))
    return (('FAILS: ' + ', '.join(failed)) if failed else 'nothing fails') + f' ({passed} passed, {time.time() - t:.0f} s)'


names = sys.argv[3:] or ['unchanged'] + list(PLANTS)
for name in names:
    if name == 'unchanged':
        print(f'{name}: {run()}', flush=True)
        continue
    path, old, new = PLANTS[name]
    full = os.path.join(W, path)
    orig = open(full).read()
    if orig.count(old) != 1:
        print(f'{name}: THE PLANT DOES NOT APPLY ({orig.count(old)} places)', flush=True)
        continue
    open(full, 'w').write(orig.replace(old, new))
    try:
        print(f'{name}: {run()}', flush=True)
    finally:
        open(full, 'w').write(orig)
