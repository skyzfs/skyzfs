#!/usr/bin/env python3
"""#100 at 59d09bf: plant one change at a time and run the tests of skyzfs-mapped and
skyzfs-local (with test-hooks, which has the kill test on real files): first the tests of the
mapped region, the cut state and the store's cuts, and the whole of both crates only where
those pass.

    plants.py CHECKOUT TARGET_DIR [PREFIX ...]

With prefixes, only the plants whose name starts with one of them. The file is restored after
each plant.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
LINUX = 'crates/skyzfs-mapped/src/linux.rs'
STATE = 'crates/skyzfs-local/src/inplace/state.rs'
INP = 'crates/skyzfs-local/src/inplace.rs'
START = 'crates/skyzfs-local/src/inplace/start.rs'
FAST = [
    ['cargo', 'test', '-p', 'skyzfs-mapped', '--no-fail-fast'],
    ['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast', '--lib', '--',
     'inplace::state', 'inplace::tests::cuts'],
    ['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast', '--test', 'inplace_real',
     '--', 'held_cut'],
]
# For a plant the fast runs pass: everything. The pool's wake test is left out: it fails now
# and then on a loaded machine, whatever is planted.
FULL = [['cargo', 'test', '-p', 'skyzfs-mapped', '-p', 'skyzfs-local', '--features', 'skyzfs-local/test-hooks',
         '--no-fail-fast', '--', '--skip', 'a_job_wakes_one_waiting_thread']]

PLANTS = [
    # the mapped region
    ('M1 a region of another length is said to be kept', LINUX,
     "            (true, false) => Opened::Resized,", "            (true, false) => Opened::Kept,"),
    ('M2 the file is not allocated', LINUX,
     "        if let Err(e) = fallocate(&file, FallocateFlags::empty(), 0, len as u64) {",
     "        if let Err(e) = (if len == 0 { fallocate(&file, FallocateFlags::empty(), 0, len as u64) } else { Ok(()) }) {"),
    ('M2b only the first page of the file is allocated', LINUX,
     "        if let Err(e) = fallocate(&file, FallocateFlags::empty(), 0, len as u64) {",
     "        if let Err(e) = fallocate(&file, FallocateFlags::empty(), 0, (len as u64).min(4096)) {"),
    ('M3 the region is not locked', LINUX,
     "        if let Err(e) = unsafe { mlock(at, len) } {",
     "        let _ = mlock;\n        if let Err(e) = Ok::<(), rustix::io::Errno>(()) {"),
    ('M4 the mapping is private', LINUX,
     "                MapFlags::SHARED,", "                MapFlags::PRIVATE,"),
    ('M5 the file is emptied at each open', LINUX,
     "            .truncate(false)", "            .truncate(true)"),
    ('M6 the file is readable by all', LINUX,
     "            .mode(0o600)", "            .mode(0o644)"),
    ('M7 a new region is said to be kept', LINUX,
     "            (false, _) => Opened::Created,", "            (false, _) => Opened::Kept,"),
    # the region at the open
    ('O1 a named region of another length counts as kept', STATE,
     "                    opened == skyzfs_mapped::Opened::Kept,", "                    opened != skyzfs_mapped::Opened::Created,"),
    ('O2 a named region counts as kept whatever the open said', STATE,
     "                    opened == skyzfs_mapped::Opened::Kept,", "                    opened == opened,"),
    ('O3 the region is opened after the marker goes', START,
     "            .map_err(OpenError::CutRegion)?;\n        if marked {",
     "            .map_err(OpenError::CutRegion);\n        if marked {",
     "        Ok((home, volume, geometry, marked, cuts))", "        Ok((home, volume, geometry, marked, cuts?))"),
    # the trust rules
    ('T1 the rule on the region having been there is dropped', INP,
     "        } else if !kept {", "        } else if !kept && false {"),
    ('T2 a missing copies file is never as good as synced', INP,
     "                    CopiesAtStart::Missing if cut_state.figures().copied == 0 => Ok(cut_state),\n", ""),
    ('T3 a missing copies file is always as good as synced', INP,
     "                    CopiesAtStart::Missing if cut_state.figures().copied == 0 => Ok(cut_state),",
     "                    CopiesAtStart::Missing if cut_state.figures().copied < u64::MAX => Ok(cut_state),"),
    ('T4 the copies file is opened and not synced', INP,
     "                let synced = file.sync();", "                let synced: io::Result<()> = Ok(());"),
    ('T5 a copies file that cannot be opened counts as missing', INP,
     "            Err(e) if e.kind() == io::ErrorKind::NotFound => CopiesAtStart::Missing,",
     "            Err(_) => CopiesAtStart::Missing,\n            #[allow(unreachable_patterns)]\n"
     "            Err(e) if e.kind() == io::ErrorKind::NotFound => CopiesAtStart::Missing,"),
    ('T6 the mark is dropped on a device taken over too', INP,
     "                if !taken_over\n                    && cut_state.mark().is_some()",
     "                if (!taken_over || true)\n                    && cut_state.mark().is_some()"),
    ('T7 the store is told the device was taken over whatever the caller says', START,
     "                    taken_over: self.taken_over,", "                    taken_over: true,"),
    ('T8 the store is told no failure is acknowledged', START,
     "                    acknowledged: self.options.acknowledge_failure,", "                    acknowledged: false,"),
    ('T9 the boot is compared by its second word alone', STATE,
     "        if words(at::BOOT) != id_words(owner.boot) && !mutants.has(Mutants::BOOT_UNCHECKED) {",
     "        if words(at::BOOT)[1] != id_words(owner.boot)[1] && !mutants.has(Mutants::BOOT_UNCHECKED) {"),
    ('T10 the boot is compared by its first word alone', STATE,
     "        if words(at::BOOT) != id_words(owner.boot) && !mutants.has(Mutants::BOOT_UNCHECKED) {",
     "        if words(at::BOOT)[0] != id_words(owner.boot)[0] && !mutants.has(Mutants::BOOT_UNCHECKED) {"),
    ('T11 the identity is compared by its first word alone', STATE,
     "        if words(at::IDENTITY) != id_words(owner.volume)", "        if words(at::IDENTITY)[0] != id_words(owner.volume)[0]"),
    ('T12 the identity is compared by its second word alone', STATE,
     "        if words(at::IDENTITY) != id_words(owner.volume)", "        if words(at::IDENTITY)[1] != id_words(owner.volume)[1]"),
    ('T13 a state not trusted is left in the region and a new one is not begun', INP,
     "                CutState::begin(region, size, bound, owner, self.mutants)\n            }\n        };\n        let newest",
     "                match CutState::restore(region, size, bound, owner, self.mutants) {\n"
     "                    Ok(state) => state,\n"
     "                    Err((_, region)) => CutState::begin(region, size, bound, owner, self.mutants),\n"
     "                }\n            }\n        };\n        let newest"),
    ('T14 a new state begins with the magic first', STATE,
     "        region.store(at::VERSION, VERSION);\n        region.store(at::BLOCKS, blocks);",
     "        region.store(at::MAGIC, MAGIC);\n        region.store(at::VERSION, VERSION);\n        region.store(at::BLOCKS, blocks);"),
    # a copy that cannot be made
    ('A1 only the cut that reads the content goes, not the newer ones', STATE,
     "        for held in holds[from..].iter().rev() {", "        for held in holds[from..=from].iter().rev() {"),
    ('A2 the cuts go oldest first', STATE,
     "        for held in holds[from..].iter().rev() {", "        for held in holds[from..].iter() {"),
    ('A3 every held cut goes', STATE,
     "        for held in holds[from..].iter().rev() {", "        for held in holds.iter().rev() {"),
    ('A4 a cut that counts the whole volume as changed stays', STATE,
     "            .or_else(|| holds.iter().rposition(|h| self.is_whole(&h.cut)));", "            ;"),
    ('A5 the oldest cut whose map holds the block, and every newer one', STATE,
     "            .rposition(|h| self.in_parts(h.parts, block))\n            .or_else(",
     "            .position(|h| self.in_parts(h.parts, block))\n            .or_else("),
    ('A6 the cuts let go are not counted as abandoned', STATE,
     "            self.give(*held, Heir::Open);\n            let abandoned = self.load(at::ABANDONED) + 1;\n"
     "            self.store(at::ABANDONED, abandoned);",
     "            self.give(*held, Heir::Open);"),
    ('A7 the copies of the cuts let go are not freed then', STATE,
     "            self.store(at::ABANDONED, abandoned);\n        }\n        self.free_unneeded(None);\n    }\n\n    /// Copy block",
     "            self.store(at::ABANDONED, abandoned);\n        }\n    }\n\n    /// Copy block"),
    ('A8 the open map is not read again after the cuts went', STATE,
     "                        self.abandon_needing(&holds, block);\n                        holds = self.holds();\n"
     "                        open = self.open();",
     "                        self.abandon_needing(&holds, block);\n                        holds = self.holds();"),
    ('A9 the held cuts are not read again after the cuts went', STATE,
     "                        self.abandon_needing(&holds, block);\n                        holds = self.holds();\n"
     "                        open = self.open();",
     "                        self.abandon_needing(&holds, block);\n                        open = self.open();"),
    ('A10 the cuts give their maps to nobody', STATE,
     "            self.give(*held, Heir::Open);\n            let abandoned = self.load(at::ABANDONED) + 1;",
     "            self.drop_map(*held);\n            let abandoned = self.load(at::ABANDONED) + 1;"),
    ('A11 a record slot whose copy failed is not given back', STATE,
     "        if let Err(e) = done {\n            self.free_records.push(index);", "        if let Err(e) = done {"),
]


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    for name, path, *pairs in PLANTS:
        if ONLY and not name.startswith(ONLY):
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
            failed, broken = tests(FAST, env)
            how = ''
            if not failed and not broken:
                failed, broken = tests(FULL, env)
                how = ' (by the whole suite only)'
        finally:
            with open(full, 'w') as f:
                f.write(orig)
        if broken:
            print(f'{name}: DID NOT BUILD OR RUN: ' + ' | '.join(broken[:3])[:300], flush=True)
        elif failed:
            print(f'{name}: KILLED by {len(failed)}{how}: ' + ', '.join(failed[:5])
                  + (' ...' if len(failed) > 5 else ''), flush=True)
        else:
            print(f'{name}: SURVIVED', flush=True)


def tests(runs, env):
    """Run each command -> (the names of the tests that failed, the build's errors)."""
    failed, broken = set(), []
    for cmd in runs:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env)
        out = p.stdout + p.stderr
        names = {ln.split()[1].split('::')[-1] for ln in out.splitlines()
                 if ln.startswith('test ') and ln.rstrip().endswith('FAILED')}
        failed |= names
        if p.returncode and not names:
            broken += [ln for ln in out.splitlines() if ln.startswith('error')] or ['exit %d' % p.returncode]
    return sorted(failed), broken


if __name__ == '__main__':
    main()
