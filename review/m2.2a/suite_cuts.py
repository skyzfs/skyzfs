#!/usr/bin/env python3
"""#98 at e905c2e: does the conformance suite "while it holds cuts" hold a cut?

    suite_cuts.py CHECKOUT TARGET_DIR

`the_in_place_store_passes_the_conformance_suite_while_it_holds_cuts` gives each start a cut
state with a bound of sixteen blocks, "so that writes are copied for a held cut and cuts are
abandoned at the bound beneath the suite's cases" (harness.rs). A cut needs an uberblock write
that passes the label checksum and a flush after it, and nothing in the suite or its harness
writes one.

Each plant makes the store panic at one moment; the suite is run with it. A run that passes
never reached that moment. The files are restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ST = 'crates/skyzfs-local/src/inplace/state.rs'
INP = 'crates/skyzfs-local/src/inplace.rs'
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--',
       'the_in_place_store_passes_the_conformance_suite_while_it_holds_cuts']
PLANTS = [
    ('a mark is recorded in the cut state', INP,
     "                    } else {\n                        state.set_mark(Some(&mark));",
     "                    } else {\n                        assert!(std::hint::black_box(false), \"review: a mark\");\n"
     "                        state.set_mark(Some(&mark));"),
    ('a cut is frozen', INP,
     "                    let number = state.freeze(&marked, cut.itself);",
     "                    assert!(std::hint::black_box(false), \"review: a cut\");\n"
     "                    let number = state.freeze(&marked, cut.itself);"),
    ('a block is copied for a held cut', ST,
     "        // Aligned, for a read by direct I/O.\n",
     "        assert!(std::hint::black_box(false), \"review: a copy\");\n"),
    ('a cut is abandoned at the bound', ST,
     "        let abandoned = self.load(at::ABANDONED) + 1;",
     "        assert!(std::hint::black_box(false), \"review: an abandoned cut\");\n"
     "        let abandoned = self.load(at::ABANDONED) + 1;"),
    ('a block\'s bit is set in the open map (the control: the suite does reach the cut state)', ST,
     "            self.maps.insert(&mut self.region, open.first, block);\n        }\n        Ok(())",
     "            assert!(std::hint::black_box(false), \"review: a bit\");\n"
     "            self.maps.insert(&mut self.region, open.first, block);\n        }\n        Ok(())"),
]


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    for name, path, old, new in PLANTS:
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        if orig.count(old) != 1:
            print(f'{name}: NOT APPLIED (found {orig.count(old)} times)', flush=True)
            continue
        with open(full, 'w') as f:
            f.write(orig.replace(old, new))
        try:
            p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
        out = p.stdout + p.stderr
        built = any(ln.startswith('test result') and (' 1 passed' in ln or ' 1 failed' in ln)
                    for ln in out.splitlines())
        if not built:
            errs = [ln for ln in out.splitlines() if ln.startswith('error')]
            print(f'{name}: DID NOT BUILD OR RUN: {errs[:1]}', flush=True)
            continue
        print(f'{name}: ' + ('reached (the suite fails)' if p.returncode else 'NEVER REACHED (the suite passes)'),
              flush=True)


if __name__ == '__main__':
    main()
