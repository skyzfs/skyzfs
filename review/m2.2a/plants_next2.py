#!/usr/bin/env python3
"""#98 at bf369ca: plants in the code the answer to the review changed. One at a time, with the
tests that concern held cuts: the cut state's, the cut rule's through the store, and the
conformance suite while the store holds cuts.

    plants_next2.py CHECKOUT TARGET_DIR [NAME...]

The file is restored after each plant.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = sys.argv[3:]
ST = 'crates/skyzfs-local/src/inplace/state.rs'
INP = 'crates/skyzfs-local/src/inplace.rs'
HAR = 'crates/skyzfs-local/src/inplace/harness.rs'
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'inplace::state', 'inplace::tests::cuts',
       'while_it_holds_cuts']

ENTRY = ("        self.store(\n            at::ABANDONED,\n"
         "            abandoned | ((oldest.entry as u64 + 1) << ABANDONING_SHIFT),\n        );\n")
PLANTS = [
    # Whether the newest cut opens as itself.
    ('N1 newest_cut: the flag is never read as set', ST,
     "        let itself = self.load(at::NEWEST_SEL) & NEWEST_ITSELF != 0;", "        let itself = false;"),
    ('N2 put_newest: the flag is never stored', ST,
     "        let flags = if itself { NEWEST_ITSELF } else { 0 };",
     "        let flags = if itself && false { NEWEST_ITSELF } else { 0 };"),
    ('N3 settle: the newest cut is made good by its state alone, not its flag', ST,
     "self.newest_cut() != Some((newest.state, newest.itself))", "self.newest() != Some(newest.state)"),
    ('N4 put_double: the slot in use is read with its flag', ST,
     "        let next = if self.load(sel) & SEL_SLOT == 1 { 2 } else { 1 };",
     "        let next = if self.load(sel) == 1 { 2 } else { 1 };"),
    ('N5 double: the slot is read with its flag', ST,
     "        match self.load(sel) & SEL_SLOT {", "        match self.load(sel) {"),
    ('N15 the start reads the flag from the held cuts, as at e905c2e', INP,
     ".restore(cut_state.count(), cut_state.newest_cut(), cut_state.mark());",
     ".restore(cut_state.count(), cut_state.newest().map(|n| (n, cut_state.held().iter()"
     ".any(|c| c.number == cut_state.count() && c.itself))), cut_state.mark());"),
    # The abandoned count.
    ('N6 abandon: the entry of the cut being abandoned is not recorded', ST, ENTRY, ""),
    ('N7 abandon: the count moves first', ST, ENTRY, "        self.store(at::ABANDONED, abandoned + 1);\n"),
    ('N8 settle: an abandoning under way is counted whether or not its cut is gone', ST,
     "            let gone = self.held_at(entry - 1).is_none();", "            let gone = true;"),
    ('N9 settle: an abandoning under way is never counted', ST,
     "            let gone = self.held_at(entry - 1).is_none();", "            let gone = false;"),
    ('N10 settle: the entry is left when its cut is counted', ST,
     "            self.store(at::ABANDONED, self.abandoned() + u64::from(gone));",
     "            let now = self.load(at::ABANDONED) + u64::from(gone);\n            self.store(at::ABANDONED, now);"),
    ('N12 figures: the abandoned count is read with the entry', ST,
     "            abandoned: self.abandoned(),", "            abandoned: self.load(at::ABANDONED),"),
    # read_cut.
    ('N13 read_cut: the block after the last is taken', INP,
     "        if block >= self.geometry.size_bytes() / skyzfs_blk::BLOCK_SIZE {",
     "        if block > self.geometry.size_bytes() / skyzfs_blk::BLOCK_SIZE {"),
    ('N14 read_cut: a buffer of any size is taken', INP,
     "        if buf.len() != skyzfs_blk::BLOCK_USIZE {", "        if false {"),
    # The copies and the suite.
    ('N16 a copy is written over the image', INP,
     "        open_copies(self.dir, self.copies)?.write_at(u64::from(slot) * skyzfs_blk::BLOCK_SIZE, buf)",
     "        self.home.write_at(u64::from(slot) * skyzfs_blk::BLOCK_SIZE, buf)"),
    ('N17 harness: a kill starts with a cut state of its own', HAR,
     "            settle_at_failure,\n            region,\n        )",
     "            settle_at_failure,\n            self.new_region(),\n        )"),
    ('N18 harness: no mark before a flush', HAR,
     "                    store.mark_as_applied(State {\n                        txg: group,\n"
     "                        root: [group; ROOT_WORDS],\n                    });",
     "                    let _ = State {\n                        txg: group,\n"
     "                        root: [group; ROOT_WORDS],\n                    };"),
    # Three plants of the first look whose lines the answer changed, as the code has them now.
    ("N19 settle: the newest cut's state and flag are not made good (S20 of the first look)", ST,
     "            if newest.number == count && self.newest_cut() != Some((newest.state, newest.itself)) {",
     "            if false && newest.number == count {"),
    ('N20 store: the next instance is not given the count (I5 of the first look)', INP,
     ".restore(cut_state.count(), cut_state.newest_cut(), cut_state.mark());",
     ".restore(0, cut_state.newest_cut(), cut_state.mark());"),
    ('N21 store: the next instance is not given the newest cut (I6 of the first look)', INP,
     ".restore(cut_state.count(), cut_state.newest_cut(), cut_state.mark());",
     ".restore(cut_state.count(), None, cut_state.mark());"),
]


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    if ONLY == ['check']:
        for name, path, old, new in PLANTS:
            print(name.split()[0], open(os.path.join(ROOT, path)).read().count(old))
        return
    for name, path, old, new in PLANTS:
        if ONLY and name.split()[0] not in ONLY:
            continue
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        n = orig.count(old)
        if n != 1:
            print(f'{name}: NOT APPLIED (found {n} times)', flush=True)
            continue
        with open(full, 'w') as f:
            f.write(orig.replace(old, new))
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
