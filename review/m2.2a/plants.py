#!/usr/bin/env python3
"""#98 at e905c2e: plant one change at a time, outside the pull request's own mutants, and run
the tests that concern held cuts: the cut state's, the cut rule's through the store, and the
conformance suite while the store holds cuts.

    plants.py CHECKOUT TARGET_DIR [NAME_PREFIX...]

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
    # The records.
    ('S1 wanted: a record begins one epoch early', ST,
     "            return Some(newest.cut.lo + 1);", "            return Some(newest.cut.lo);"),
    ('S2 read: a record also serves the cut of its last epoch', ST,
     "                .find(|(_, record)| record.lo <= number && number < record.hi)",
     "                .find(|(_, record)| record.lo <= number && number <= record.hi)"),
    ('S3 needed: a cut that holds the whole volume needs no record', ST,
     "                && (self.in_parts(h.parts, record.block) || self.is_whole(&h.cut))",
     "                && self.in_parts(h.parts, record.block)"),
    ('S16 change: a record a lost instance made for this change is not looked for', ST,
     "            let done = !holds.is_empty()\n                && self", "            let done = false\n                && self"),
    ('S17 copy: the record names its block before its epochs', ST,
     "        self.store_record(base + 1, lo | (hi << 32));\n        self.store_record(base, block + 1);",
     "        self.store_record(base, block + 1);\n        self.store_record(base + 1, lo | (hi << 32));"),
    ('S18 free_unneeded: nothing is freed', ST,
     "            if of_gone || !self.needed(&record, &holds) {", "            if of_gone {"),
    ('S36 change: the block\'s bit before its copy', ST,
     "                if let Some(lo) = self.wanted(&holds, block) {\n                    self.copy(block, lo, epoch, io)?;\n                }\n            }\n            self.maps.insert(&mut self.region, open.first, block);",
     "                self.maps.insert(&mut self.region, open.first, block);\n                if let Some(lo) = self.wanted(&holds, block) {\n                    self.copy(block, lo, epoch, io)?;\n                }\n            }\n            self.maps.insert(&mut self.region, open.first, block);"),
    # The lo of a cut.
    ('S4 release: the last cut released does not leave its number as the next lo', ST,
     "        if holds.len() == 1 {\n            self.store(at::OPEN_LO, number);\n        }\n", ""),
    ('S5 give: the open map takes the donor\'s lo whoever else is held', ST,
     "                    if self.held().len() == 1 {\n                        self.store(at::OPEN_LO, donor.cut.lo);\n                    }",
     "                    self.store(at::OPEN_LO, donor.cut.lo);"),
    ('S10 freeze: the lo is read before the flushing cut is let go', ST,
     "        if let Some(flushing) = self\n            .holds()\n            .into_iter()\n            .find(|h| h.cut.phase == Phase::Flushing)\n        {\n            self.give(flushing, Heir::Open);\n            self.free_unneeded(None);\n        }\n        let number = self.count() + 1;\n        let lo = self.next_lo();",
     "        let lo = self.next_lo();\n        if let Some(flushing) = self\n            .holds()\n            .into_iter()\n            .find(|h| h.cut.phase == Phase::Flushing)\n        {\n            self.give(flushing, Heir::Open);\n            self.free_unneeded(None);\n        }\n        let number = self.count() + 1;"),
    ('S14 is_whole: a cut whose lo is the whole epoch holds the whole volume too', ST,
     "        whole > cut.lo && whole <= cut.number", "        whole >= cut.lo && whole <= cut.number"),
    ('S34 begin: the whole volume does not count as changed', ST,
     "        region.store(at::WHOLE, 1);", "        region.store(at::WHOLE, 0);"),
    # Taking a map, and the start.
    ('S6 settle: a taking whose donor is still held is not finished', ST,
     "            let Some((donor, heir)) = taken else { break };\n            self.give(donor, heir);",
     "            let _ = taken;\n            break;"),
    ('S7 settle: a cut whose flush was under way does not take the start\'s sync as its flush', ST,
     "            if cut.phase == Phase::Flushing {\n                self.flushed(cut.number);\n            }\n        }\n        let waiting: Vec<Held>",
     "            let _ = cut;\n        }\n        let waiting: Vec<Held>"),
    ('S8 finish: the larger part is named alone before the move', ST,
     "            self.maps.move_into(&mut self.region, small, large);\n            large\n        };",
     "            self.set_parts(heir, Parts { first: large, also: None });\n            self.maps.move_into(&mut self.region, small, large);\n            large\n        };"),
    ('S9 finish: the open map is not named twice on the way (thought to change nothing)', ST,
     "        if heir == Heir::Open && large != parts.first {", "        if false && heir == Heir::Open && large != parts.first {"),
    ('S11 flushed: a waiting cut is replaced whatever its number (thought to change nothing)', ST,
     "            .find(|h| h.cut.phase == Phase::Waiting && h.cut.number < number)",
     "            .find(|h| h.cut.phase == Phase::Waiting)"),
    ('S12 abandon: an oldest cut not being uploaded gives its map to the open map', ST,
     "            holds\n                .get(1)\n                .map_or(Heir::Open, |next| Heir::Cut(next.cut.number))",
     "            Heir::Open"),
    ('S13 settle: two waiting cuts are left as they are (thought not to be reached)', ST,
     "            for older in older {\n                self.give(*older, Heir::Cut(newest.cut.number));\n            }",
     "            let _ = (older, newest);"),
    ('S15 change: after the bound let cuts go, the held cuts and the open map are not read again', ST,
     "                    while self.copied() + BLOCK_SIZE > self.bound() && self.abandon() {}\n                    holds = self.holds();\n                    open = self.open();",
     "                    while self.copied() + BLOCK_SIZE > self.bound() && self.abandon() {}"),
    ('S19 settle: a mark the newest cut took is kept', ST,
     "        if self.mark() == self.newest() {\n            self.set_mark(None);\n        }\n        for map in 0..MAPS {",
     "        for map in 0..MAPS {"),
    ('S20 settle: the newest cut\'s state is not made good', ST,
     "            if newest.number == count && self.newest() != Some(newest.state) {", "            if false {"),
    ('S21 settle: a cut recorded and not counted is left in its slot', ST,
     "            if self.load(base) > count {\n                self.store(base, 0);\n            }", "            let _ = base;"),
    ('S22 settle: the maps nothing names are not cleared', ST,
     "        for map in 0..MAPS {\n            self.let_go(map);\n        }\n        self.free_unneeded(None);\n    }\n\n    // ---- records ----",
     "        self.free_unneeded(None);\n    }\n\n    // ---- records ----"),
    ('S23 let_go: a map is cleared though something names it', ST,
     "        if map < MAPS && !self.mutants.has(Mutants::MAP_NOT_CLEARED) && !self.owned()[map] {",
     "        if map < MAPS && !self.mutants.has(Mutants::MAP_NOT_CLEARED) {"),
    ('S24 give: the donor is let go before its heir names its map', ST,
     "        if !taken {\n            self.finish(heir);\n            match heir {",
     "        if !taken {\n            self.finish(heir);\n            self.drop_cut(donor.cut.number);\n            match heir {"),
    ('S35 flush_failed: the failed cut\'s map is dropped', ST,
     "        self.give(cut, heir);\n        self.free_unneeded(None);\n    }\n\n    /// Cut `number`, the one being uploaded",
     "        let _ = heir;\n        self.drop_map(cut);\n        self.free_unneeded(None);\n    }\n\n    /// Cut `number`, the one being uploaded"),
    ('S37 release: the cut that waits is not the next to be uploaded', ST,
     "        if let Some(waiting) = holds.iter().find(|h| h.cut.phase == Phase::Waiting) {\n            self.set_phase(waiting.cut.number, Phase::Uploading);\n        }\n        self.free_unneeded(None);\n        true",
     "        self.free_unneeded(None);\n        true"),
    ('S38 freeze: the mark the cut took is kept', ST,
     "        if self.mark().as_ref() == Some(state) {\n            self.set_mark(None);\n        }\n", ""),
    # The maps.
    ('M1 move_into: the summary of the map moved from is left', MAPS,
     "            if !first {\n                region.store(at_from, 0);\n            }\n        }\n        self.clear_summary(region, from);",
     "            if !first {\n                region.store(at_from, 0);\n            }\n        }"),
    ('M2 clear: the summary before the words', MAPS,
     "        for word in self.noted_words(region, map) {\n            let at = self.bits_base(map) + word;\n            if region.load(at) != 0 {\n                region.store(at, 0);\n            }\n        }\n        self.clear_summary(region, map);\n    }\n\n    fn clear_summary",
     "        let words = self.noted_words(region, map);\n        self.clear_summary(region, map);\n        for word in words {\n            let at = self.bits_base(map) + word;\n            if region.load(at) != 0 {\n                region.store(at, 0);\n            }\n        }\n    }\n\n    fn clear_summary"),
    ('M3 move_into: the word moved to is not noted in its summary', MAPS,
     "            self.note_word(region, to, word);\n            let at_to", "            let at_to"),
    # The store.
    ('I1 store: a write-zeroes does not pass through the cut state', INP,
     "                if let Err(e) = self.note_change(offset, len) {\n                    let result = self.mutated(Err(e));\n                    return self.complete(id, result, None);\n                }\n", ""),
    ('I2 store: a flush that completes is told to the cut state as failed, and one that fails as completed', INP,
     "                if result.is_ok() {\n                    state.flushed(number);\n                } else {\n                    state.flush_failed(number);\n                }",
     "                if result.is_ok() {\n                    state.flush_failed(number);\n                } else {\n                    state.flushed(number);\n                }"),
    ('I3 store: the completion of a cut\'s flush is not told to the cut state', INP,
     "        if let Some(number) = self.flushing.remove(&id) {\n            if let Some(state) = &mut self.state {", "        if let Some(number) = self.flushing.remove(&id) {\n            if let Some(state) = &mut self.state.as_mut().filter(|_| false) {"),
    ('I4 store: the next instance is not given the mark', INP,
     "            .restore(cut_state.count(), newest, cut_state.mark());", "            .restore(cut_state.count(), newest, None);"),
    ('I5 store: the next instance is not given the count', INP,
     "            .restore(cut_state.count(), newest, cut_state.mark());", "            .restore(0, newest, cut_state.mark());"),
    ('I6 store: the next instance is not given the newest cut', INP,
     "            .restore(cut_state.count(), newest, cut_state.mark());", "            .restore(cut_state.count(), None, cut_state.mark());"),
    ('I7 store: the change is noted for the write\'s first block alone', INP,
     "        state.change(first, end - first, &mut files)", "        state.change(first, (end - first).min(1), &mut files)"),
    ('I8 store: a copy that cannot be made does not fail the write', INP,
     "                    if let Err(e) = self.note_change(offset, len) {\n                        let result = self.mutated(Err(e));\n                        return self.complete(id, result, Some(data));\n                    }",
     "                    let _ = self.note_change(offset, len);"),
]


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    for name, path, old, new in PLANTS:
        if ONLY and not any(name.startswith(p + ' ') for p in ONLY):
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
