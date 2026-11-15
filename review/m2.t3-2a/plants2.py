#!/usr/bin/env python3
"""#103 at 718d198: plants in the code that commit added to the judge, run as `plants.py` runs
its own.

    plants2.py CHECKOUT [PREFIX ...]
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import plants  # noqa: E402

J = plants.J
plants.PLANTS[:] = [
    ('N1 a wrong block under an unplaced write-zeroes is a verdict', J,
     "    if not ok and wrong & amb:", "    if False:"),
    ('N2 a wrong block is the rig\'s error wherever it is', J,
     "    if not ok and wrong & amb:", "    if not ok:"),
    ('N3 no block of a write-zeroes is unplaced', J,
     "                add({'flags': ['discard'], 'offset': off, 'bytes': n, 'op': ln['op'], 'ambiguous': amb})",
     "                add({'flags': ['discard'], 'offset': off, 'bytes': n, 'op': ln['op'], 'ambiguous': set()})"),
    ('N4 every block of a write-zeroes is unplaced', J,
     "                add({'flags': ['discard'], 'offset': off, 'bytes': n, 'op': ln['op'], 'ambiguous': amb})",
     "                add({'flags': ['discard'], 'offset': off, 'bytes': n, 'op': ln['op'], 'ambiguous': set(range(lo, hi + 1))})"),
    ('N5 bound need not abandon a cut', J,
     "        ok = ab is not None and ab.isdigit() and int(ab) >= 1", "        ok = True"),
    ('N6 a missing cuts_abandoned in bound is no verdict', J,
     "        rc = rc if ok else 1\n    else:\n        put('cuts_abandoned', 'INFO'", "    else:\n        put('cuts_abandoned', 'INFO'"),
    ('N7 edges without a write-zeroes is no error', J,
     "        if not nz or not nt:", "        if not nt:"),
    ('N8 edges without a discard is no error', J,
     "        if not nz or not nt:", "        if not nz:"),
    ('N9 only five of the wrong blocks are known for the unplaced ones', J,
     "        compared, unknown, either, wrong = held_compare(model, image, limit=10 ** 9)",
     "        compared, unknown, either, wrong = held_compare(model, image, limit=5)"),
    ('N10 the recording\'s name is not given to the second stage', J,
     "            v, e = held_lines(put, rdir, instances[0], records, dcuts, status, dev, name)",
     "            v, e = held_lines(put, rdir, instances[0], records, dcuts, status, dev)"),
]

if __name__ == '__main__':
    plants.main()
