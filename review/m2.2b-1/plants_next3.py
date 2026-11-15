#!/usr/bin/env python3
"""#100 at 5fd84fb: the two plants its two new tests are for.

    plants_next3.py CHECKOUT TARGET_DIR
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import plants  # noqa: E402

plants.PLANTS[:] = [
    ('K8 the start takes the state up before it has judged the copies', plants.INP,
     "            match CutState::view(region, size, bound, owner, self.mutants) {",
     "            match CutState::view(region, size, bound, owner, self.mutants).map(|s| s.take_up(start)) {"),
    ('N10 settle: the entry is left when its cut is counted', plants.STATE,
     "            self.store(at::ABANDONED, self.abandoned() + u64::from(gone));",
     "            let now = self.load(at::ABANDONED) + u64::from(gone);\n            self.store(at::ABANDONED, now);"),
]

if __name__ == '__main__':
    plants.main()
