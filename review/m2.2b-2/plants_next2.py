#!/usr/bin/env python3
"""#101 at 449b20e: the commands after the volume has failed (design §5.2: the instance that saw
the failure still writes out the cuts whose flush completed), each way of refusing them planted
on the store's side of the commands; and C7, C12, R7 of `plants.py` again.

    plants_next2.py CHECKOUT TARGET_DIR [PREFIX ...]
    plants_next2.py CHECKOUT TARGET_DIR none      the tests as the head has them
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import plants  # noqa: E402

CUT = plants.CUT
KEEP = ('C7', 'C12', 'R7')
plants.PLANTS[:] = [p for p in plants.PLANTS if p[0].split()[0] in KEEP] + [
    ('F3 a held cut is not read once the volume has failed', CUT,
     "        self.read_cut(number, block, buf)\n",
     "        if self.is_failed() {\n            return Err(io::Error::other(\"the volume has failed\"));\n        }\n"
     "        self.read_cut(number, block, buf)\n"),
    ('F4 no cut is listed as held once the volume has failed', CUT,
     "        self.held_cuts()\n            .into_iter()\n",
     "        self.held_cuts()\n            .into_iter()\n            .filter(|_| !self.is_failed())\n"),
    ('F5 no cut is let go once the volume has failed', CUT,
     "        self.release_cut(number)\n", "        !self.is_failed() && self.release_cut(number)\n"),
    ('F6 a cut has no map once the volume has failed', CUT,
     "        self.cut_map(number)\n",
     "        if self.is_failed() {\n            return Vec::new();\n        }\n        self.cut_map(number)\n"),
]

if __name__ == '__main__':
    plants.main()
