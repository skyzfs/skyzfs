#!/usr/bin/env python3
"""Plant one change at a time in a checkout of #95 and run the tests that should notice it.

    plants.py CHECKOUT TARGET_DIR [NAME_PREFIX...]

Each plant: (name, file, old, new, tests). `tests` names which cargo runs to make:
  U  skyzfs-uberblock's tests
  L  skyzfs-local's lib tests of the cut rule and the recorded pools
  S  skyzfs-ublk's status tests and skyzfsd's lib tests
The file is restored after each plant.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = sys.argv[3:]

UB = 'crates/skyzfs-uberblock/src/lib.rs'
CUTS = 'crates/skyzfs-local/src/inplace/cuts.rs'
INPLACE = 'crates/skyzfs-local/src/inplace.rs'
STATUS = 'crates/skyzfs-ublk/src/status.rs'
RECORD = 'bins/skyzfsd/src/record.rs'

RUNS = {
    'U': ['cargo', 'test', '-p', 'skyzfs-uberblock'],
    'L': ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'tests::cuts', 'tests::recorded'],
    'S': ['cargo', 'test', '-p', 'skyzfs-ublk', '-p', 'skyzfsd', '--lib'],
    # The request log is in the test-hooks build alone.
    'H': ['cargo', 'test', '-p', 'skyzfsd', '--lib', '--features', 'test-hooks'],
}

PLANTS = [
    # The view
    ('V1 chosen: unknown positions ignored', UB,
     "        self.is_known()\n            && self\n                .held",
     "        true\n            && self\n                .held", 'UL'),
    ('V2 chosen: a tie with another state is itself', UB,
     ".all(|uberblock| uberblock.state == *state)", ".any(|uberblock| uberblock.state == *state)", 'UL'),
    ('V3 changed: an overwritten uberblock stays', UB,
     "                    self.held.remove(&at);\n", "", 'UL'),
    ('V4 changed: a part-covered position is known', UB,
     "let whole = offset <= at && at + READ_LEN as u64 <= end;", "let whole = true;", 'UL'),
    ('V5 changed: a position before the write is touched', UB,
     "if at + READ_LEN as u64 > offset {", "if true {", 'UL'),
    ('V6 zeroed: changes nothing', UB,
     "        self.changed(offset, len, &[]);\n", "        let _ = (offset, len);\n", 'UL'),
    ('V7 rank: sequence without the MMP magic', UB,
     "order.word(&bytes[MMP_MAGIC_AT..]) == MMP_MAGIC && config & MMP_SEQ_VALID_BIT != 0",
     "config & MMP_SEQ_VALID_BIT != 0", 'UL'),
    ('V8 rank: sequence without its valid bit', UB,
     "order.word(&bytes[MMP_MAGIC_AT..]) == MMP_MAGIC && config & MMP_SEQ_VALID_BIT != 0",
     "order.word(&bytes[MMP_MAGIC_AT..]) == MMP_MAGIC", 'UL'),
    ('V9 rank: sequence shifted wrong', UB, "(config & MMP_SEQ_MASK) >> 32", "(config & MMP_SEQ_MASK) >> 16", 'UL'),
    ('V10 rank: timestamp read from guid_sum', UB, "const TIMESTAMP_AT: usize = 32;", "const TIMESTAMP_AT: usize = 24;", 'UL'),
    ('V11 rank: timestamp before group', UB,
     "    /// `ub_txg`.\n    pub txg: u64,\n    /// `ub_timestamp`.\n    pub timestamp: u64,\n    /// `MMP_SEQ`, or 0.",
     "    /// `ub_timestamp`.\n    pub timestamp: u64,\n    /// `ub_txg`.\n    pub txg: u64,\n    /// `MMP_SEQ`, or 0.", 'UL'),
    ('V12 rank: no sequence', UB, "            seq,\n        },\n    })", "            seq: seq * 0,\n        },\n    })", 'UL'),
    ('V13 unknown: only the first ring', UB,
     "            .rings()\n            .flat_map(|ring| (ring.start..ring.end()).step_by(SLOT_MIN as usize))",
     "            .rings()\n            .take(1)\n            .flat_map(|ring| (ring.start..ring.end()).step_by(SLOT_MIN as usize))", 'UL'),
    ('V14 changed: only the first ring', UB,
     "        for ring in self.labels.rings() {\n            let mut at = offset.max(ring.start) / SLOT_MIN * SLOT_MIN;",
     "        for ring in self.labels.rings().take(1) {\n            let mut at = offset.max(ring.start) / SLOT_MIN * SLOT_MIN;", 'UL'),
    ('V15 chosen: no uberblock in view is itself', UB,
     "        let Some(best) = self.held.values().map(rank).max() else {\n            return false;\n        };",
     "        let Some(best) = self.held.values().map(rank).max() else {\n            return true;\n        };", 'UL'),
    ('V16 changed: found not inserted', UB,
     "        for uberblock in found {\n            self.held.insert(uberblock.at, *uberblock);\n        }",
     "        let _ = found;", 'UL'),
    # The rule and the store
    ('C1 count: every cut is itself', CUTS,
     "let mut itself = self.view.chosen(&state, by_group);", "let mut itself = self.view.chosen(&state, by_group) || true;", 'L'),
    ('C2 received: a write-zeroes is not noted', CUTS,
     "                if !self.labels.reaches_a_ring(*offset, *len) {\n                    return;\n                }\n                (*offset, *len, Vec::new())",
     "                let _ = (offset, len);\n                return;", 'L'),
    ('C3 store: a write-zeroes applied is not told', INPLACE,
     "                if result.is_ok() {\n                    self.cuts.zeroed(id);\n                }\n", "", 'L'),
    ('C4 start: a ring that cannot be read is taken as read', INPLACE,
     "            if read.is_ok() {\n                self.cuts.read_at_start(offset, &buf);\n            }",
     "            let _ = read;\n            self.cuts.read_at_start(offset, &buf);", 'L'),
    ('C5 start: the last ring is not read', INPLACE,
     "        for (offset, len) in self.cuts.rings() {\n            let mut buf",
     "        for (offset, len) in self.cuts.rings().into_iter().take(3) {\n            let mut buf", 'L'),
    ('C6 start: the rings are read as zeros', INPLACE,
     "            if read.is_ok() {\n                self.cuts.read_at_start(offset, &buf);\n            }",
     "            let _ = read;\n            self.cuts.read_at_start(offset, &Buf::zeroed(buf.len() / skyzfs_blk::BLOCK_SIZE as usize));", 'L'),
    ('C7 apply: the view is not changed by a write', CUTS,
     "        self.view.changed(change.offset, change.len, &change.found);\n", "", 'L'),
    ('C8 count: itself judged by the newest cut before', CUTS,
     "        let state = uberblock.state;\n        self.newest = Some(state);\n        self.cuts.count += 1;",
     "        let state = self.newest.unwrap_or(uberblock.state);\n        self.newest = Some(uberblock.state);\n        self.cuts.count += 1;", 'L'),
    ('C9 ended: a change not applied is applied', CUTS,
     "        self.changes.remove(&id);\n        self.carrying.remove(&id);",
     "        if let Some(change) = self.changes.remove(&id) {\n            self.apply(change);\n        }\n        self.carrying.remove(&id);", 'L'),
    # The status file and the request log
    ('T1 status: itself and rewind both written as 1', STATUS,
     "cuts.newest.map_or(0, |cut| if cut.itself { 1 } else { 2 }),", "cuts.newest.map_or(0, |_| 1),", 'S'),
    ('T2 status: any value but 0 reads as itself', STATUS,
     "itself: version >= 12 && field(304) == 1,", "itself: version >= 12 && field(304) != 0,", 'S'),
    ('T3 status: version 11 said to say itself', STATUS,
     "itself: version >= 12 && field(304) == 1,", "itself: field(304) == 1,", 'S'),
    ('T4 status: cut_opens printed for version 11', STATUS,
     "                if s.version >= 12 {\n                    let opens", "                if s.version >= 11 {\n                    let opens", 'S'),
    ('T5 status: version 11 cut group not shifted', STATUS, "group: field(after(296)),", "group: field(296),", 'S'),
    ('T6 status: version 11 commands not shifted', STATUS,
     "counters.commands_failed = field(after(272));", "counters.commands_failed = field(272);", 'S'),
    ('T7 status: version 11 io queue not shifted', STATUS,
     "io_queue: Some(io_queue_at(bytes, after(216))),", "io_queue: Some(io_queue_at(bytes, 216)),", 'S'),
    ('T8 status: itself and rewind swapped in print', STATUS,
     'let opens = if newest.itself { "itself" } else { "rewind" };\n                    lines.push',
     'let opens = if newest.itself { "rewind" } else { "itself" };\n                    lines.push', 'S'),
    ('T9 request log: itself and rewind swapped', RECORD,
     'let opens = if cut.itself { "itself" } else { "rewind" };', 'let opens = if cut.itself { "rewind" } else { "itself" };', 'H'),
    ('T10 status: version 11 cuts count not shifted', STATUS,
     "let count = field(after(288));", "let count = field(288);", 'S'),
]


def run(kind):
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    p = subprocess.run(RUNS[kind], cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    failed = sorted({ln.split()[1] for ln in out.splitlines() if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
    if p.returncode and not failed:
        errs = [ln for ln in out.splitlines() if ln.startswith('error')]
        return p.returncode, ['(build) ' + (errs[0][:160] if errs else 'failed')]
    return p.returncode, failed


def main():
    for name, path, old, new, kinds in PLANTS:
        if ONLY and not any(name.startswith(p) for p in ONLY):
            continue
        full = os.path.join(ROOT, path)
        with open(full) as f:
            orig = f.read()
        n = orig.count(old)
        if n != 1:
            print(f'{name}: NOT APPLIED (found {n} times)', flush=True)
            continue
        with open(full, 'w') as f:
            f.write(orig.replace(old, new))
        try:
            got = []
            for k in kinds:
                rc, failed = run(k)
                if rc:
                    got.append(f"{k}: {', '.join(t.split('::')[-1] for t in failed[:5])}"
                               + (f' (+{len(failed) - 5})' if len(failed) > 5 else ''))
        finally:
            with open(full, 'w') as f:
                f.write(orig)
        print(f"{name}: " + ('KILLED by ' + '; '.join(got) if got else 'SURVIVED'), flush=True)


if __name__ == '__main__':
    main()
