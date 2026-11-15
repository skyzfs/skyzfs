#!/usr/bin/env python3
"""#106 at 21f6f83: plants in the store's keeping of the intent map, one at a time,
against the tests of skyzfs-local with its test hooks. Each plant is a list of
(old, new) edits of inplace.rs or state.rs; the file is restored after each. A run
that does not end in 15 minutes is stopped and reported so.

    plants.py CHECKOUT TARGET_DIR [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
INP = 'crates/skyzfs-local/src/inplace.rs'
STATE = 'crates/skyzfs-local/src/inplace/state.rs'
PLANTS = [
    ('W1 a request that waits for the map does not count as a write in flight', INP,
     [("        let mut busy = self.busy.values().copied().chain(waiting).peekable();\n",
       "        let mut busy = self.busy.values().copied().chain(waiting.take(0)).peekable();\n")]),
    ('W2 a flush does not wait for a write that waits for the map', INP,
     [("            Op::Flush => return busy.any(|(_, _, write)| write),\n",
       "            Op::Flush => return self.busy.values().any(|&(_, _, write)| write),\n")]),
    ('W3 a write-zeroes does not ask the map', INP,
     [("                if !self.name_in_intent(offset, len) {\n                    return self.wait_for_intent(id, Op::WriteZeroes { offset, len });\n                }\n", "")]),
    ('W4 a failed write of a map page is not looked at', INP,
     [("        let begun = written.and_then(|()| {\n", "        let _ = written;\n        let begun = Ok(()).and_then(|()| {\n")]),
    ('W5 a failed sync of the map lets the waiting writes begin', INP,
     [("        intent.map.synced(result.is_ok());\n        if let Some(began) = intent.syncing.take() {\n            intent.syncs.add(began.elapsed());\n        }\n        if result.is_err() {\n",
       "        intent.map.synced(true);\n        if let Some(began) = intent.syncing.take() {\n            intent.syncs.add(began.elapsed());\n        }\n        if false {\n")]),
    ('W6 a failed sync of the map fails the waiting writes and not the volume', INP,
     [("        if result.is_err() {\n            if !self.failed {\n                self.fail();\n            }\n            self.settle_failed();\n            return self.run_deferred();\n",
       "        if result.is_err() {\n            self.settle_failed();\n            return self.run_deferred();\n")]),
    ('W7 the release does not name the cut as the base', INP,
     [("        intent.map.commit(intent::Base { number, group });\n", "        let _ = group;\n")]),
    ('W8 the release clears by the changes after the cut before', INP,
     [("            .and_then(|state| state.changed_after(number));\n", "            .and_then(|state| state.changed_after(number + 1));\n")]),
    ('W9 a failed write of the map at a release does not fail the volume', INP,
     [("        if self.write_intent(false).is_err() {\n            self.fail();\n            self.settle_failed();\n        }\n",
       "        let _ = self.write_intent(false);\n")]),
    ('W10 a write that waited goes on though the volume failed meanwhile', INP,
     [("        if self.failed {\n            let buf = buffer_of(op);\n            return self.complete(id, Err(BlkError::Failed), buf);\n        }\n        match op {\n            Op::Write { offset, data } => self.write(id, offset, data),\n",
       "        match op {\n            Op::Write { offset, data } => self.write(id, offset, data),\n")]),
    ('W11 a volume that fails leaves the requests that wait for the map waiting', INP,
     [("        for Request { id, op } in waiting {\n            let buf = buffer_of(op);\n            self.complete(id, Err(BlkError::Failed), buf);\n        }\n", "        drop(waiting);\n")]),
    ('S1 a start that acknowledges a failure does not take the map up', INP,
     [("        let found_map = found_map.and_then(|map| {\n            map.trust(Some(owner.start))?;\n            Ok(map)\n        });\n",
       "        let found_map = found_map.and_then(|map| {\n            map.trust(Some(owner.start))?;\n            if acknowledged {\n                return Err(intent::Distrust::Token);\n            }\n            Ok(map)\n        });\n")]),
    ('S2 the map is judged by this start\'s own token', INP,
     [("            map.trust(Some(owner.start))?;\n", "            map.trust(Some(start))?;\n")]),
    ('S3 a start whose sync of the map fails serves all the same', INP,
     [("            intent.map.synced(synced.is_ok());\n            synced?;\n", "            intent.map.synced(synced.is_ok());\n            let _ = synced;\n")]),
    ('S4 a start whose write of the map fails serves all the same', INP,
     [("        self.keep_intent(shape, start, found_map, map_file)\n            .map_err(|e| StartError::Intent(e.to_string()))\n",
       "        let _ = self.keep_intent(shape, start, found_map, map_file);\n        Ok(())\n")]),
    ('S5 the new state counts the whole volume though the map was taken up', STATE,
     [("        match changed {\n            None => region.store(at::WHOLE, 1),\n", "        region.store(at::WHOLE, 1);\n        match changed {\n            None => {}\n")]),
    ('S6 a region is carried without its last block', STATE,
     [("                    maps.insert_range(&mut region, 0, first, count);\n", "                    maps.insert_range(&mut region, 0, first, count.saturating_sub(1));\n")]),
    ('S7 the map is kept under the token the open found', INP,
     [("                map.kept_by(start);\n", "                map.kept_by(owner_start_for_plant);\n"),
      ("        self.keep_intent(shape, start, found_map, map_file)\n", "        self.keep_intent(shape, owner.start, found_map, map_file)\n"),
      ("                map.kept_by(owner_start_for_plant);\n", "                map.kept_by(start);\n")]),
    ('S8 a map taken up is not written before the start serves', INP,
     [("                map.kept_by(start);\n                (map, file)\n", "                (map, file)\n")]),
    ('S9 a new map is begun over the file found, not in a new file', INP,
     [("                match self.dir.remove(intent::INTENT_NAME) {\n                    Ok(()) => {}\n                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}\n                    Err(e) => return Err(e),\n                }\n", "")]),
]


def tests(env):
    cmd = ['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast']
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env, timeout=900)
    except subprocess.TimeoutExpired:
        subprocess.run(['pkill', '-f', 'debug/deps/skyzfs_local-'], check=False)
        subprocess.run(['pkill', '-f', 'out/skyzfs_local-'], check=False)
        return ['(did not end in 15 minutes)'], []
    out = (p.stdout + p.stderr).splitlines()
    failed = sorted({ln.split()[1].split('::')[-1] for ln in out if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
    broken = [] if failed or p.returncode == 0 else [ln for ln in out if ln.startswith('error')][:3]
    return failed, broken


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    if not ONLY:
        print(f'as the head has it: {tests(env)}', flush=True)
    for name, path, edits in PLANTS:
        if ONLY and not name.startswith(ONLY):
            continue
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        text = orig
        ok = True
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
            print(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(failed[:8])}' if failed else
                                 'DID NOT BUILD: ' + ' | '.join(broken)[:300] if broken else 'SURVIVED'), flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
    print('finished', flush=True)


if __name__ == '__main__':
    main()
