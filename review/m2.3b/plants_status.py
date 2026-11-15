#!/usr/bin/env python3
"""#106 at 21f6f83: plants in status version 14 and in what the daemon shows of the
map, one at a time, against the tests of skyzfs-ublk and skyzfsd (with its test hooks).

    plants_status.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ST = 'crates/skyzfs-ublk/src/status.rs'
SERVE = 'bins/skyzfsd/src/serve.rs'
CLI = 'bins/skyzfsd/src/cli.rs'
PLANTS = [
    ('T1 the waits are written where the named-at-start is, and the reverse', ST,
     [("        intent.named_at_start,\n", "        intent.WAITS_PLANT,\n"), ("        intent.waits,\n", "        intent.named_at_start,\n"),
      ("        intent.WAITS_PLANT,\n", "        intent.waits,\n")]),
    ('T2 the waits are read from the longest sync\'s place', ST, [("            waits: field(416),\n", "            waits: field(408),\n")]),
    ('T3 the key intent_waits is not printed', ST, [("                (\"intent_waits\", intent.waits),\n", "")]),
    ('T4 a start that began a new map shows no region named at the start', SERVE,
     [("        Some(IntentAtStart::Begun(_)) => (CutStateShown::Begun, figures.regions),\n",
       "        Some(IntentAtStart::Begun(_)) => (CutStateShown::Begun, 0),\n")]),
    ('T5 the region given is not handed to the store', SERVE,
     [("            intent_region: serve.intent_region_mib.map(|mib| mib << 20),\n", "            intent_region: None,\n")]),
    ('T6 --intent-region-mib is taken without --hold-cuts', CLI,
     [("                if !hold_cuts {\n                    return Err(UsageError::Value(name(), \"only with --hold-cuts\"));\n                }\n", "")]),
    ('T7 --intent-region-mib takes a size that is not a power of two', CLI,
     [("                if !mib.is_power_of_two()\n                    || !(MIN_INTENT_REGION_MIB..=MAX_INTENT_REGION_MIB).contains(&mib)\n",
       "                if !(MIN_INTENT_REGION_MIB..=MAX_INTENT_REGION_MIB).contains(&mib)\n")]),
]
RUNS = [['cargo', 'test', '-p', 'skyzfs-ublk', '--no-fail-fast', 'status'],
        ['cargo', 'test', '-p', 'skyzfsd', '--features', 'test-hooks', '--no-fail-fast']]


def tests(env):
    failed, broken = [], []
    for cmd in RUNS:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env)
        out = (p.stdout + p.stderr).splitlines()
        f = [ln.split()[1].split('::')[-1] for ln in out if ln.startswith('test ') and ln.rstrip().endswith('FAILED')]
        failed += f
        if p.returncode != 0 and not f:
            broken += [ln for ln in out if ln.startswith('error')][:2]
    return sorted(set(failed)), broken


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    print(f'as the head has it: {tests(env)}', flush=True)
    for name, path, edits in PLANTS:
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        text, ok = orig, True
        for old, new in edits:
            if text.count(old) != 1:
                print(f'{name}: NOT APPLIED ({text.count(old)} matches of {old[:50]!r})', flush=True)
                ok = False
                break
            text = text.replace(old, new)
        if not ok:
            continue
        try:
            with open(full, 'w') as f:
                f.write(text)
            failed, broken = tests(env)
            print(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(failed[:6])}' if failed else
                                 'DID NOT BUILD: ' + ' | '.join(broken)[:300] if broken else 'SURVIVED'), flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
    print('finished', flush=True)


if __name__ == '__main__':
    main()
