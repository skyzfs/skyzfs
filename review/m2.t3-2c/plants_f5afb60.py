#!/usr/bin/env python3
"""#113 at f5afb60: the plants of plants.py again on the new head (those
of the either-way counting rule no longer apply and say so), and new
ones in what the new commit changed: numbers_verdict, where a new state
must count from 1 again, one rule at a time (M1 to M6), and the status's
count, held to the last state's cuts (Q1 to Q3).

    plants_f5afb60.py CHECKOUT OUT [PREFIX ...]
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import plants  # noqa: E402  (reads CHECKOUT, OUT and the prefixes from sys.argv)

J = plants.J
FIRST = ("            if n != '1':\n"
         "                return 'FAIL', (f\"state {e + 1}'s first cut line (request log line {dcuts[k]['line']}) \"\n"
         "                                f\"carries the number {n!r}, not 1: a new state counts from 1 again\")\n"
         "            k0 = k\n"
         "            said.append(f'state {e + 1} counts from 1 again')\n")
SINCE = "        since = starts[-1]['i'] if starts else None\n"

plants.PLANTS += [
    # ---- numbers_verdict at f5afb60: a new state counts from 1 again ----
    ('M1 a new state may count on, as before the commit (from 1 again or on, the line says which)', J,
     FIRST, "            if n == '1':\n                k0 = k\n                said.append(f'state {e + 1} counts from 1 again')\n"
            "            else:\n                said.append(f'state {e + 1} counts on')\n"),
    ('M2 a new state must count on (from 1 again is refused)', J,
     FIRST, "            if n == '1':\n"
            "                return 'FAIL', f\"state {e + 1}'s first cut line carries the number 1\"\n"
            "            said.append(f'state {e + 1} counts on')\n"),
    ('M3 the new state\'s own refusal is gone (the line then fails by the rule for every line, in its words)', J,
     "            if n != '1':\n                return 'FAIL', (f\"state", "            if False:\n                return 'FAIL', (f\"state"),
    ('M4 the count is not begun again at a new state (its first line must be 1, its later lines go on)', J,
     "            k0 = k\n            said.append(f'state {e + 1} counts from 1 again')\n",
     "            said.append(f'state {e + 1} counts from 1 again')\n"),
    ('M5 a new state whose first line is not 1 passes', J,
     "            if n != '1':\n                return 'FAIL', (f\"state", "            if n != '1':\n                return 'PASS', (f\"state"),
    ('M6 a new state is not seen (every line is of the first state)', J,
     "        if e != epoch:\n            epoch = e\n            if n != '1':", "        if False:\n            epoch = e\n            if n != '1':"),
    # ---- the status's count at f5afb60 ----
    ('Q1 the status is held to the whole log\'s count', J, SINCE, "        since = None\n"),
    ('Q2 the status is held to the cuts since the first new state, not the last', J,
     SINCE, "        since = starts[0]['i'] if starts else None\n"),
    ('Q3 the status is held to the last state\'s cuts only where its first line carries 1, as before the commit', J,
     SINCE, "        since = None\n        if starts:\n            first = [d for d in dcuts if d['epoch'] == len(starts)]\n"
            "            if first and first[0]['local'] == '1':\n                since = starts[-1]['i']\n"),
]

if __name__ == '__main__':
    plants.main()
