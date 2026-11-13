#!/usr/bin/env python3
"""Defects planted at 8e8cb23 (pull request #60, second look), one at a
time, and which tests of skyzfs-blk (all its targets) and skyzfs-blkcheck
fail. Each file is put back after its plant.

    plants3.py WORKTREE TARGET_DIR [name...]
"""
import os
import re
import subprocess
import sys
import time

W, TARGET = sys.argv[1], sys.argv[2]
MODEL = 'crates/skyzfs-blk/src/model.rs'
CONF = 'crates/skyzfs-blk/src/conformance.rs'
DEVICE = 'bins/skyzfs-blkcheck/src/device.rs'
REPLAY = 'bins/skyzfs-blkcheck/src/replay.rs'
GUARD = '        if self.recovering.is_some() {\n            return;\n        }\n        // Once the log has failed, reported or not, the restart\n'

PLANTS = {
    # The harness's default says completions can be held past a loss.
    'default-flipped': (CONF,
        '    fn in_flight_at_a_loss(&self) -> InFlight {\n        InFlight::Reissued\n    }',
        '    fn in_flight_at_a_loss(&self) -> InFlight {\n        InFlight::MayHaveCompleted\n    }'),
    # The driver does not ask the harness.
    'driver-ignores-harness': (CONF,
        'self.model.kill(self.harness.in_flight_at_a_loss());',
        'self.model.kill(InFlight::Reissued);'),
    # The device harness does not claim late completions.
    'device-says-reissued': (DEVICE,
        '    fn in_flight_at_a_loss(&self) -> InFlight {\n        InFlight::MayHaveCompleted\n    }',
        '    fn in_flight_at_a_loss(&self) -> InFlight {\n        InFlight::Reissued\n    }'),
    # The replay does not claim late completions.
    'replay-says-reissued': (REPLAY,
        'model.kill(InFlight::MayHaveCompleted);',
        'model.kill(InFlight::Reissued);'),
    # The model takes every loss as one with late completions.
    'model-always-wide': (MODEL, GUARD,
        GUARD.replace('        // Once', '        let in_flight = InFlight::MayHaveCompleted;\n        // Once')),
    # The model takes every loss as one where all is issued again.
    'model-always-reissued': (MODEL, GUARD,
        GUARD.replace('        // Once', '        let in_flight = InFlight::Reissued;\n        // Once')),
    # Applied means completed: an applied request is never issued again.
    'no-outcome-2': (MODEL,
        '                for choice in 0..1u64 << either.len() {',
        '                for choice in (1u64 << either.len()) - 1..1u64 << either.len() {'),
}


def run(pkg, extra=()):
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET, CARGO_INCREMENTAL='0')
    t = time.time()
    try:
        p = subprocess.run(['cargo', 'test', '-q', '-p', pkg, '--no-fail-fast', *extra], cwd=W, env=env,
                           capture_output=True, text=True, timeout=1500)
    except subprocess.TimeoutExpired:
        return 'HUNG (1500 s)'
    out = p.stdout + p.stderr
    if 'error[' in out or 'could not compile' in out:
        return 'DID NOT COMPILE: ' + (re.search(r'error[^\n]*', out) or [''])[0]
    failed = sorted(set(re.findall(r'^test (\S+) \.\.\. FAILED', out, re.M)) |
                    set(re.findall(r'^---- (\S+) stdout ----', out, re.M)))
    ok = sum(int(n) for n in re.findall(r'^test result: \w+\. (\d+) passed', out, re.M))
    return (', '.join(x.split('::')[-1] for x in failed) or 'nothing fails') + f' ({ok} pass, {time.time() - t:.0f} s)'


names = sys.argv[3:] or list(PLANTS)
print('unchanged:\n  skyzfs-blk:', run('skyzfs-blk'), '\n  skyzfs-blkcheck:', run('skyzfs-blkcheck'), flush=True)
for name in names:
    path, old, new = PLANTS[name]
    full = os.path.join(W, path)
    orig = open(full, encoding='utf-8').read()
    if orig.count(old) != 1:
        print(f'{name}: THE PLANT DOES NOT APPLY ({orig.count(old)} places)', flush=True)
        continue
    try:
        open(full, 'w', encoding='utf-8').write(orig.replace(old, new))
        a = run('skyzfs-blk')
        b = run('skyzfs-blkcheck')
        print(f'{name}:\n  skyzfs-blk: {a}\n  skyzfs-blkcheck: {b}', flush=True)
    finally:
        open(full, 'w', encoding='utf-8').write(orig)
