#!/usr/bin/env python3
"""#106 at ab2d7f2: plants in the answer's fixes, one at a time, against the tests of
skyzfs-local with its test hooks. The file is restored after each; a run that does not
end in 12 minutes is stopped and reported so.

    plants2.py CHECKOUT TARGET_DIR [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
INP = 'crates/skyzfs-local/src/inplace.rs'
INT = 'crates/skyzfs-local/src/inplace/intent.rs'
PLANTS = [
    ('D1a the directory is not synced after the map\'s creation', INP,
     [("                self.dir.sync()?;\n                (intent::IntentMap::new(shape, start, self.mutants), file)\n",
       "                (intent::IntentMap::new(shape, start, self.mutants), file)\n")]),
    ('D2a the drop does not look at the map\'s sync', INP,
     [("            if for_map {\n                if self.intent_syncing() {\n                    self.settle_intent(true);\n                } else if self.failed {\n                    self.settle_failed();\n                } else {\n                    self.begin_intent_sync();\n                }\n            }\n", "")]),
    ('D2b the drop does not go on for a request that waits for the map', INP,
     [("            if !(on_threads || for_map || !self.deferred.is_empty()) {\n", "            if !(on_threads || !self.deferred.is_empty()) {\n")]),
    ('D2c the drop does not run what was deferred when there are no threads', INP,
     [("            self.collect(Duration::from_millis(10));\n            self.run_deferred();\n        }\n        self.pool = None;\n",
       "            self.collect(Duration::from_millis(10));\n        }\n        self.pool = None;\n")]),
    ('L1 a bit set while the sync ran counts as lost when the sync ends', INP,
     [("                lost |= waiting.begun < intent.begun && waiting.commits == intent.commits;\n",
       "                lost |= waiting.commits == intent.commits;\n")]),
    ('L2 a bit a commit cleared counts as lost', INP,
     [("                lost |= waiting.begun < intent.begun && waiting.commits == intent.commits;\n",
       "                lost |= waiting.begun < intent.begun;\n")]),
    ('L3 a bit that a sync left unwritten is set again, with no end', INP,
     [("                lost |= waiting.begun < intent.begun && waiting.commits == intent.commits;\n",
       "                let _ = (waiting.begun, waiting.commits);\n")]),
    ('L4 a lost bit fails the waiting requests and not the volume', INP,
     [("        if lost {\n            if !self.failed {\n                self.fail();\n            }\n            self.settle_failed();\n        }\n",
       "        if lost {\n            self.settle_failed();\n        }\n")]),
    ('C1 direct I/O is checked after the map is kept', INP,
     [("        store.check_direct_io().map_err(StartError::Io)?;\n        if let Some(cuts) = parts.cuts {\n", "        if let Some(cuts) = parts.cuts {\n"),
      ("        store.set_up_io().map_err(StartError::Io)?;\n", "        store.check_direct_io().map_err(StartError::Io)?;\n        store.set_up_io().map_err(StartError::Io)?;\n")]),
    ('N1 the count takes a page\'s last, short word as a whole one', INT,
     [("            let whole = bit == 1 && region % PER_PAGE + 64 <= PER_PAGE && region + 64 <= regions;\n",
       "            let whole = bit == 1 && region + 64 <= regions;\n")]),
    ('N2 the count takes the volume\'s last, short word as a whole one', INT,
     [("            let whole = bit == 1 && region % PER_PAGE + 64 <= PER_PAGE && region + 64 <= regions;\n",
       "            let whole = bit == 1 && region % PER_PAGE + 64 <= PER_PAGE;\n")]),
]


def tests(env):
    cmd = ['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast']
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env, timeout=720)
    except subprocess.TimeoutExpired:
        subprocess.run(['pkill', '-f', os.path.basename(TARGET) + '/debug/deps/skyzfs_local-'], check=False)
        return ['(did not end in 12 minutes)'], []
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
            print(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(failed[:8])}' if failed else
                                 'DID NOT BUILD: ' + ' | '.join(broken)[:300] if broken else 'SURVIVED'), flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
    print('finished', flush=True)


if __name__ == '__main__':
    main()
