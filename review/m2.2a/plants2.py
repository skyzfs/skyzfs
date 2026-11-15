#!/usr/bin/env python3
"""#98 at e905c2e, a second batch: plant one change at a time, outside the pull request's own mutants, and run
the tests that concern held cuts: the cut state's, the cut rule's through the store, and the
conformance suite while the store holds cuts.

    plants2.py CHECKOUT TARGET_DIR [NAME_PREFIX...]

The file is restored after each plant.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = sys.argv[3:]
ST = 'crates/skyzfs-local/src/inplace/state.rs'
MAPS = 'crates/skyzfs-local/src/inplace/state/maps.rs'
INP = 'crates/skyzfs-local/src/inplace.rs'
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'inplace::state', 'inplace::tests::cuts',
       'while_it_holds_cuts']

PLANTS = [
    # The bound.
    ('P2 change: the bound is reached one copy early', ST,
     "                if self.copied() + BLOCK_SIZE > self.bound() {\n",
     "                if self.copied() + BLOCK_SIZE >= self.bound() {\n",
     "                    while self.copied() + BLOCK_SIZE > self.bound() && self.abandon() {}",
     "                    while self.copied() + BLOCK_SIZE >= self.bound() && self.abandon() {}"),
    ('P3 change: at the bound only the oldest cut goes', ST,
     "                    while self.copied() + BLOCK_SIZE > self.bound() && self.abandon() {}",
     "                    let _ = self.abandon();"),
    ('P4 abandon: the abandoned cut is not counted', ST,
     "        self.store(at::ABANDONED, abandoned);", "        let _ = abandoned;"),
    # The phases.
    ('P1 settle: with no cut being uploaded, the oldest that waits is left waiting', ST,
     "            if let Some(oldest) = held.iter().find(|c| c.phase == Phase::Waiting) {\n                self.set_phase(oldest.number, Phase::Uploading);",
     "            if let Some(oldest) = held.iter().find(|c| c.phase == Phase::Waiting) {\n                self.set_phase(oldest.number, Phase::Waiting);"),
    ('P5 flushed: with no cut being uploaded, the cut whose flush completed waits', ST,
     "            None if !uploading => self.set_phase(number, Phase::Uploading),",
     "            None if !uploading => self.set_phase(number, Phase::Waiting),"),
    # The mark.
    ('P8 settle: the start clears the mark whatever it is', ST,
     "        if self.mark() == self.newest() {\n            self.set_mark(None);",
     "        if true {\n            self.set_mark(None);"),
    ('P9 freeze: a cut clears the mark whatever it is', ST,
     "        if self.mark().as_ref() == Some(state) {\n            self.set_mark(None);",
     "        if true {\n            self.set_mark(None);"),
    # The records.
    ('P11 give: a cut that takes a map keeps its own lo', ST,
     "                            parts.with_lo(held.cut.lo.min(donor.cut.lo)),",
     "                            parts.with_lo(held.cut.lo),"),
    ('P14 needed: a record is needed by any held cut whose map holds its block', ST,
     "            record.lo <= h.cut.number\n                && h.cut.number < record.hi\n                && (self",
     "            (self"),
    ('P15 wanted: the oldest held cut whose map holds the block dates the record', ST,
     "        if let Some(newest) = holds.iter().rev().find(|h| self.in_parts(h.parts, block)) {",
     "        if let Some(newest) = holds.iter().find(|h| self.in_parts(h.parts, block)) {"),
    # The store.
    ('Q1 store: a write the threads applied does not record its mark at once', INP,
     "            if write && result.is_ok() {\n                self.cuts.applied(id);\n                self.record_cuts();",
     "            if write && result.is_ok() {\n                self.cuts.applied(id);"),
    ('Q2 store: a write applied on the caller\'s thread does not record its mark at once', INP,
     "                    self.cuts.applied(id);\n                    self.record_cuts();",
     "                    self.cuts.applied(id);"),
    ('Q3 store: a flush at its barrier does not record its cut at once', INP,
     "        self.cuts.barrier(id);\n        self.record_cuts();",
     "        self.cuts.barrier(id);"),
]


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    for name, path, *pairs in PLANTS:
        if ONLY and not any(name.startswith(p + ' ') for p in ONLY):
            continue
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        counts = [orig.count(old) for old in pairs[0::2]]
        if counts != [1] * len(counts):
            print(f'{name}: NOT APPLIED (found {counts} times)', flush=True)
            continue
        changed = orig
        for old, new in zip(pairs[0::2], pairs[1::2]):
            changed = changed.replace(old, new)
        with open(full, 'w') as f:
            f.write(changed)
        try:
            p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
            out = p.stdout + p.stderr
            failed = sorted({ln.split()[1].split('::')[-1] for ln in out.splitlines()
                             if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
            if p.returncode and not failed:
                errs = [ln for ln in out.splitlines() if ln.startswith('error')]
                failed = ['(build) ' + (errs[0][:160] if errs else 'failed')]
        finally:
            with open(full, 'w') as f:
                f.write(orig)
        print(f"{name}: " + ('KILLED by ' + ', '.join(failed) if p.returncode else 'SURVIVED'), flush=True)


if __name__ == '__main__':
    main()
