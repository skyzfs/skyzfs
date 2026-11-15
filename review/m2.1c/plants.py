#!/usr/bin/env python3
"""Plant one change at a time in a checkout of #96 and run the tests that should notice it.

    plants.py CHECKOUT TARGET_DIR [NAME_PREFIX...]

Each plant: (name, file, old, new, kinds). `kinds` names which cargo runs to make:
  U  skyzfs-uberblock's tests
  L  skyzfs-local's lib tests of the cut rule and the recorded pools
The file is restored after each plant.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = sys.argv[3:]

UB = 'crates/skyzfs-uberblock/src/lib.rs'
CUTS = 'crates/skyzfs-local/src/inplace/cuts.rs'

RUNS = {
    'U': ['cargo', 'test', '-p', 'skyzfs-uberblock'],
    'L': ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'tests::cuts', 'tests::recorded'],
}

HOLDS = ("                        (at - ring.start).is_multiple_of(size)\n"
         "                            && at + size <= to\n"
         "                            && label_checksum_holds(&data[i..i + size as usize], at)")

PLANTS = [
    # The checksum's steps
    ('K1 verifier: no offset', UB,
     "    verifier[..8].copy_from_slice(&order.bytes(offset));\n", "    let _ = (offset, order);\n", 'UL'),
    ('K2 verifier: the offset in the second word', UB,
     "    verifier[..8].copy_from_slice(&order.bytes(offset));", "    verifier[8..16].copy_from_slice(&order.bytes(offset));", 'UL'),
    ('K3 verifier: always little endian', UB,
     "    verifier[..8].copy_from_slice(&order.bytes(offset));",
     "    let _ = order;\n    verifier[..8].copy_from_slice(&offset.to_le_bytes());", 'UL'),
    ('K4 hash: the stored words left in, no verifier', UB,
     "    hasher.update(&slot[..slot.len() - CKSUM_LEN]);\n    hasher.update(verifier);",
     "    let _ = verifier;\n    hasher.update(slot);", 'UL'),
    ('K5 hash: without the checksum magic', UB,
     "    hasher.update(&slot[..slot.len() - CKSUM_LEN]);", "    hasher.update(&slot[..slot.len() - ECK_LEN]);", 'UL'),
    ('K6 words: the digest read little endian', UB,
     "        *word = ByteOrder::Big.word(&digest[8 * n..]);", "        *word = ByteOrder::Little.word(&digest[8 * n..]);", 'UL'),
    ('K7 words: the digest words in reverse', UB,
     "        *word = ByteOrder::Big.word(&digest[8 * n..]);", "        *word = ByteOrder::Big.word(&digest[8 * (3 - n)..]);", 'UL'),
    ('K8 holds: always', UB,
     "    stored == label_checksum(slot, offset, order)", "    let _ = (stored, order, offset);\n    true", 'UL'),
    ('K9 holds: the first word alone compared', UB,
     "    stored == label_checksum(slot, offset, order)", "    stored[0] == label_checksum(slot, offset, order)[0]", 'UL'),
    ('K10 holds: the order always little', UB,
     "    let order = if ByteOrder::Big.word(&slot[eck..]) == ZEC_MAGIC {\n        ByteOrder::Big\n    } else {",
     "    let order = if false {\n        ByteOrder::Big\n    } else {", 'UL'),
    ('K11 holds: stored words always read little endian', UB,
     "        *word = order.word(&slot[eck + 8 + 8 * n..]);", "        *word = ByteOrder::Little.word(&slot[eck + 8 + 8 * n..]);", 'UL'),
    ('K12 holds: a slot without the checksum magic is refused', UB,
     "    } else {\n        ByteOrder::Little\n    };\n    let mut stored",
     "    } else if ByteOrder::Little.word(&slot[eck..]) == ZEC_MAGIC {\n        ByteOrder::Little\n    } else {\n        return false;\n    };\n    let mut stored", 'UL'),
    ('K13 holds: the words read one word early', UB,
     "        *word = order.word(&slot[eck + 8 + 8 * n..]);", "        *word = order.word(&slot[eck + 8 * n..]);", 'UL'),
    # The slot size
    ('Z1 size: 4 KiB alone', UB,
     "uberblock.slot = SLOT_SIZES.into_iter().find(|&size| {", "uberblock.slot = [4u64 << 10].into_iter().find(|&size| {", 'UL'),
    ('Z2 size: largest first', UB,
     "uberblock.slot = SLOT_SIZES.into_iter().find(|&size| {", "uberblock.slot = SLOT_SIZES.into_iter().rev().find(|&size| {", 'UL'),
    ('Z3 size: no alignment in the ring', UB, HOLDS,
     HOLDS.replace("(at - ring.start).is_multiple_of(size)", "true"), 'UL'),
    ('Z4 size: aligned on the volume, not in the ring', UB, HOLDS,
     HOLDS.replace("(at - ring.start).is_multiple_of(size)", "at.is_multiple_of(size)"), 'UL'),
    ('Z5 size: a slot the write holds in part is tried (to the end of the write)', UB, HOLDS,
     HOLDS.replace("&& at + size <= to\n", "&& at + size <= to + size\n")
          .replace("&data[i..i + size as usize]", "&data[i..(i + size as usize).min(data.len())]"), 'UL'),
    ('Z6 size: the checksum held against the ring-relative offset', UB, HOLDS,
     HOLDS.replace("label_checksum_holds(&data[i..i + size as usize], at)",
                   "label_checksum_holds(&data[i..i + size as usize], at - ring.start)"), 'UL'),
    ('Z7 size: 8 KiB never tried', UB,
     "uberblock.slot = SLOT_SIZES.into_iter().find(|&size| {", "uberblock.slot = SLOT_SIZES.into_iter().take(3).find(|&size| {", 'UL'),
    ('Z8 size: 1 KiB never tried', UB,
     "uberblock.slot = SLOT_SIZES.into_iter().find(|&size| {", "uberblock.slot = SLOT_SIZES.into_iter().skip(1).find(|&size| {", 'UL'),
    # Where the check applies
    ('W1 store: a write as received is not checked', CUTS,
     "                let found = self.found(*offset, data);", "                let found = self.labels.uberblocks_in(*offset, data);", 'L'),
    ('W2 store: the rings read at the start are not checked', CUTS,
     "            let found = self.found(offset, data);\n            self.view.changed",
     "            let found = self.labels.uberblocks_in(offset, data);\n            self.view.changed", 'L'),
    ('W3 store: nothing is checked', CUTS,
     "            found.retain(Uberblock::verified);\n        }\n        found", "        }\n        found", 'L'),
    ('W4 view: saw keeps the unverified', UB,
     "        found.retain(Uberblock::verified);\n        self.changed(offset, data.len() as u64, &found);",
     "        self.changed(offset, data.len() as u64, &found);", 'UL'),
    ('W5 store: the verified are dropped, the rest kept', CUTS,
     "            found.retain(Uberblock::verified);\n        }\n        found",
     "            found.retain(|u| !u.verified());\n        }\n        found", 'L'),
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
