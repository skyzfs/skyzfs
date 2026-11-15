#!/usr/bin/env python3
"""Plant one change at a time in bins/skyzfs-nodeb/src/main.rs of #97 and run its tests.

    plants_tool.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
PATH = os.path.join(ROOT, 'bins/skyzfs-nodeb/src/main.rs')

PLANTS = [
    ('T1 put: the holes are not written',
     "        eng.write_zeroes(o, l)\n            .map_err(|e| format!(\"hole {o}:{l}: {e}\"))?;\n", "        let _ = (o, l);\n"),
    ('T2 put: the holes are written before the image',
     None, None),
    ('T3 put: no commit before the snapshot',
     "    let head = eng\n        .commit(CommitKind::Import)\n        .await\n        .map_err(|e| format!(\"commit: {e}\"))?;\n    let (_, snapshot)",
     "    let head = [0u8; 32];\n    let (_, snapshot)"),
    ('T4 get: a chunk is written only if its first byte is not zero',
     "        if b.iter().any(|&x| x != 0) {", "        if b.first().is_some_and(|&x| x != 0) {"),
    ('T5 get: the file is not set to the volume\'s size',
     "    out.set_len(size).map_err(|e| format!(\"{out_path}: {e}\"))?;\n", ""),
    ('T6 get: reads the head whatever ref is asked for',
     "    let eng = Engine::open_ref(store, refs, vol, reference, EngineConfig::default())",
     "    let eng = Engine::open_ref(store, refs, vol, \"head\", EngineConfig::default())"),
    ('T7 get: a ref that is not there reads as the head',
     "    else {\n        return Ok(None);\n    };\n    let eng = Engine::open_ref(store, refs, vol, reference, EngineConfig::default())",
     "    else {\n        return Err(\"planted\".into());\n    };\n    let eng = Engine::open_ref(store, refs, vol, reference, EngineConfig::default())"),
    ('T8 runs: a zero block inside a run is written (runs never split)',
     "        if b.iter().all(|&x| x == 0) {\n            continue;\n        }\n", ""),
    ('T9 runs: the last short block is dropped',
     "            _ => out.push((at, b.len())),", "            _ if b.len() == BLOCK => out.push((at, b.len())),\n            _ => {}"),
    ('T10 volume_id: the last 16 bytes of the hash',
     "    b.copy_from_slice(&h.as_bytes()[..16]);", "    b.copy_from_slice(&h.as_bytes()[16..]);"),
    ('T11 parse_hole: any offset',
     "    if !o.is_multiple_of(512) || !l.is_multiple_of(512) || l == 0 {", "    if !l.is_multiple_of(512) || l == 0 {"),
    ('T12 put: the snapshot is taken under another name',
     "        .snapshot_create(snapshot)", "        .snapshot_create(\"u\")"),
]


def main():
    orig = open(PATH).read()
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    for name, old, new in PLANTS:
        if old is None:
            continue
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
