#!/usr/bin/env python3
"""#97 at its next head (75bf72b): plants in bins/skyzfs-nodeb/src/main.rs for what the head
added (a ref that cannot be read, the exit codes), and the first round's survivors again.

    plants_tool2.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
PATH = os.path.join(ROOT, 'bins/skyzfs-nodeb/src/main.rs')
OPEN = "    let eng = match Engine::open_ref(store, refs, vol, reference, EngineConfig::default()).await {"

PLANTS = [
    # The first round's survivors.
    ('T3 put: no commit before the snapshot',
     "    let head = eng\n        .commit(CommitKind::Import)\n        .await\n        .map_err(|e| format!(\"commit: {e}\"))?;\n    let (_, snapshot)",
     "    let head = [0u8; 32];\n    let (_, snapshot)"),
    ('T5 get: the file is not set to the volume\'s size',
     "    out.set_len(size).map_err(|e| format!(\"{out_path}: {e}\"))?;\n", ""),
    ('T6 get: reads the head whatever ref is asked for', OPEN,
     "    let eng = match Engine::open_ref(store, refs, vol, \"head\", EngineConfig::default()).await {"),
    # What the head added.
    ('U1 get: a commit that cannot be opened is the rig\'s error',
     "        Err(e) => return Ok(Got::Unreadable(format!(\"open {reference}: {e}\"))),",
     "        Err(e) => return Err(format!(\"open {reference}: {e}\")),"),
    ('U2 get: a range that cannot be read is the rig\'s error',
     "            Err(e) => return Ok(Got::Unreadable(format!(\"read at {at}: {e}\"))),",
     "            Err(e) => return Err(format!(\"read at {at}: {e}\")),"),
    ('U3 get: a ref that cannot be read ends with exit 2', "            Ok(4)\n", "            Ok(2)\n"),
    ('U4 get: a ref that is not there ends with exit 2', "            Ok(3)\n", "            Ok(2)\n"),
    ('U5 get: a ref that is not there reads as one that cannot be read',
     "        return Ok(Got::Absent);", "        return Ok(Got::Unreadable(\"planted\".into()));"),
]


def main():
    orig = open(PATH).read()
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    for name, old, new in PLANTS:
        n = orig.count(old)
        if n != 1:
            print(f'{name}: NOT APPLIED (found {n} times)', flush=True)
            continue
        with open(PATH, 'w') as f:
            f.write(orig.replace(old, new))
        try:
            p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-nodeb'], cwd=ROOT, capture_output=True, text=True, env=env)
            out = p.stdout + p.stderr
            failed = sorted({ln.split()[1].split('::')[-1] for ln in out.splitlines()
                             if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
            if p.returncode and not failed:
                errs = [ln for ln in out.splitlines() if ln.startswith('error')]
                failed = ['(build) ' + (errs[0][:120] if errs else 'failed')]
        finally:
            with open(PATH, 'w') as f:
                f.write(orig)
        print(f"{name}: " + ('KILLED by ' + ', '.join(failed) if p.returncode else 'SURVIVED'), flush=True)


if __name__ == '__main__':
    main()
