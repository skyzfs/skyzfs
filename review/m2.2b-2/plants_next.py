#!/usr/bin/env python3
"""#101 at 89a87c0: the plants of `plants.py` that passed at 7e4b7b8, again, and two in the code
the answer added (the commands after the volume has failed).

    plants_next.py CHECKOUT TARGET_DIR [PREFIX ...]
    plants_next.py CHECKOUT TARGET_DIR none      the tests as the head has them
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import plants  # noqa: E402

CUT = plants.CUT
KEEP = ('C7', 'C12', 'R7', 'W1', 'W2', 'W3', 'W4')
plants.PLANTS[:] = [p for p in plants.PLANTS if p[0].split()[0] in KEEP] + [
    ('F1 a command is carried out after the volume has failed', CUT,
     "            Ok(_) if cuts.failed() => refused(&token, FAILED),",
     "            Ok(_) if cuts.failed() && false => refused(&token, FAILED),"),
    ('F2 a write under way goes on after the volume has failed', CUT,
     "    if cuts.failed() {", "    if cuts.failed() && false {"),
]

if __name__ == '__main__':
    plants.main()
