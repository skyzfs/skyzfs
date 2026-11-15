#!/usr/bin/env python3
"""#103 at 008274f: plants in the code the answer added (the placement behind a flush, the
per-cut scope of unplaced blocks, the count of copied blocks, the holder after a refusal), run
as `plants.py` runs its own.

    plants3.py CHECKOUT [PREFIX ...]
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import plants  # noqa: E402

J, H = plants.J, plants.H
BEHIND = "        if received:\n            received[next(reversed(received))].append(r)\n        else:\n            add(r)\n"
plants.PLANTS[:] = [
    ('Q1 a write-zeroes is placed at its receipt though a flush waits', J, BEHIND, "        add(r)\n"),
    ('Q2 it is placed behind the first flush that waits, not the last', J, BEHIND,
     "        if received:\n            received[next(iter(received))].append(r)\n        else:\n            add(r)\n"),
    ('Q3 what waits behind a flush is dropped at its barrier', J,
     "            last_barrier = add(r)\n            for x in after:\n                add(x)\n", "            last_barrier = add(r)\n"),
    ('Q4 what waits behind a flush is placed before its barrier', J,
     "            last_barrier = add(r)\n            for x in after:\n                add(x)\n",
     "            for x in after:\n                add(x)\n            last_barrier = add(r)\n"),
    ('Q5 no block is left unplaced by a write-zeroes', J,
     "                for b in r['ambiguous']:\n                    live[b] = r['seq']\n", "                pass\n"),
    ('Q6 any later write places the block again, whenever it was received', J,
     "                    if b in live and r['seq'] > live[b]:", "                    if b in live:"),
    ('Q7 no later write places the block again', J,
     "                    if b in live and r['seq'] > live[b]:", "                    if False:"),
    ('Q8 a wrong block is the rig\'s error wherever it is', J,
     "    ok = True if not bad else (False if definite else None)", "    ok = True if not bad else None"),
    ('Q9 a wrong block is a verdict wherever it is', J,
     "    ok = True if not bad else (False if definite else None)", "    ok = not bad"),
    ('Q10 a wrong block that is placed does not make the failure definite', J,
     "        definite = definite or bool(here - set(live))\n", ""),
    ('Q11 a copied block is counted whenever the write was applied', J,
     "            elif cut_ref.is_write(r) and 'ns' in r and r['ns'] < w['asked']:", "            elif cut_ref.is_write(r) and 'ns' in r:"),
    ('Q12 a write-zeroes counts as copied though it was answered after the write was asked', J,
     "                if r.get('done_ns', w['asked']) < w['asked']:", "                if True:"),
    ('Q13 the copied blocks are counted from the cut before', J,
     "        for r in records[d['prefix'] + 1:]:", "        for r in records:"),
    ('Q14 edges need not hold a cut across its write-zeroes', J,
     "        if not held_over:\n", "        if False:\n"),
    ('Q15 the first written cut counts for edges\' floor', J,
     "        held_over = [(n, z) for n, _, z in later if z]", "        held_over = [(n, z) for n, _, z in cp if z]"),
    ('Q16 a cut still being uploaded after a refusal is no error', H,
     "            raise HolderError(f'cut {number} is still being uploaded after {why}')", "            pass"),
    ('Q17 a refused write is followed by a release', H,
     "            self.must_be_gone(c['number'], f'its write was refused ({last})')\n            return True\n",
     "            self.d.send(f\"release {c['number']}\")\n            return True\n"),
    ('Q18 a refused release is not looked at', H,
     "        if not ok:\n            self.must_be_gone(c['number'], f'its release was refused ({last})')\n", ""),
]

if __name__ == '__main__':
    plants.main()
