#!/usr/bin/env python3
"""Defects planted in Model::kill and World::settle_at_loss (pull request
#60), one at a time, and which tests of skyzfs-blk (unit tests) and
skyzfs-blkcheck fail. The file is put back after each plant.

    plants.py WORKTREE [name...]
"""
import os
import re
import subprocess
import sys

W = sys.argv[1]
MODEL = os.path.join(W, 'crates/skyzfs-blk/src/model.rs')
ORIG = open(MODEL, encoding='utf-8').read()

PLANTS = {
    # Outcome 3 offered to a request the lost daemon did not apply: lost.
    'completed-without-applied': (
        '                    .filter(|op| applied.contains(op))\n',
        '                    .filter(|_| true)\n'),
    # Outcome 3 never offered: the model before this change.
    'no-outcome-3': (
        '                for choice in 0..1u64 << either.len() {',
        '                for choice in 0..1u64 {'),
    # Applied means completed: an applied request is never issued again.
    'no-outcome-2': (
        '                for choice in 0..1u64 << either.len() {',
        '                for choice in (1u64 << either.len()) - 1..1u64 << either.len() {'),
    # Every request in flight was applied by the lost daemon.
    'no-outcome-1': (
        '                next.insert((without, applied));\n            }\n            worlds = next;\n        }\n        worlds\n    }\n\n    /// The members of the tail that are, or must precede',
        '            }\n            worlds = next;\n        }\n        worlds\n    }\n\n    /// The members of the tail that are, or must precede'),
    # A request a read showed applied does not count as applied.
    'shown-not-applied': (
        '                applied.extend(&shown);\n',
        ''),
    # The lost daemon may have applied it before it was submitted.
    'applied-before-submitted': (
        '                    lb: w.lb,\n                    ub: now,\n                    flushed: false,\n                };\n                let mut placed = BTreeSet::new();',
        '                    lb: 0,\n                    ub: now,\n                    flushed: false,\n                };\n                let mut placed = BTreeSet::new();'),
    # The completed ones are the ones kept waiting.
    'completed-inverted': (
        '                        .filter(|op| !completed(**op))\n',
        '                        .filter(|op| completed(**op) || !either.contains(*op))\n'),
    # A reissue may be applied from before the loss.
    'reissue-from-zero': (
        '                        .map(|op| Waiting { op: *op, lb: now })\n                        .collect();\n                    next.insert(world);',
        '                        .map(|op| Waiting { op: *op, lb: 0 })\n                        .collect();\n                    next.insert(world);'),
    # What the lost daemon applied is not made durable.
    'applied-not-durable': (
        '            for (mut world, mut applied) in world.settle_at_loss(&self.cx, now) {\n                if healthy {',
        '            for (mut world, mut applied) in world.settle_at_loss(&self.cx, now) {\n                if healthy && applied.is_empty() {'),
    # The lost daemon applied each request whole or not: here only one
    # way of placing it is kept.
    'one-placement': (
        '                for world in placed {\n                    next.insert((world, with.clone()));\n                }',
        '                if let Some(world) = placed.into_iter().next() {\n                    next.insert((world, with.clone()));\n                }'),
}


def run(pkg, extra=()):
    env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(W, 'target'), CARGO_INCREMENTAL='0')
    try:
        p = subprocess.run(['cargo', 'test', '-q', '-p', pkg, *extra], cwd=W, env=env, capture_output=True,
                           text=True, timeout=600)
    except subprocess.TimeoutExpired:
        return ['HUNG (600 s)']
    out = p.stdout + p.stderr
    if 'error[' in out or 'could not compile' in out:
        return ['DID NOT COMPILE: ' + (re.search(r'error[^\n]*', out) or [''])[0]]
    return sorted(set(re.findall(r'^test (\S+) \.\.\. FAILED', out, re.M)) |
                  set(re.findall(r'^---- (\S+) stdout ----', out, re.M)))


names = sys.argv[2:] or list(PLANTS)
try:
    print('unchanged:', run('skyzfs-blk', ['--lib']) or 'nothing fails', '|', run('skyzfs-blkcheck') or 'nothing fails',
          flush=True)
    for name in names:
        old, new = PLANTS[name]
        if ORIG.count(old) != 1:
            print(f'{name}: THE PLANT DOES NOT APPLY ({ORIG.count(old)} places)', flush=True)
            continue
        open(MODEL, 'w', encoding='utf-8').write(ORIG.replace(old, new))
        a = run('skyzfs-blk', ['--lib'])
        b = run('skyzfs-blkcheck')
        short = lambda xs: ', '.join(x.split('::')[-1] for x in xs) or 'nothing fails'
        print(f'{name}:\n  model: {short(a)}\n  replay: {short(b)}', flush=True)
finally:
    open(MODEL, 'w', encoding='utf-8').write(ORIG)
