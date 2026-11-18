#!/usr/bin/env python3
"""#113 at 73869d1: plants in the bound on the digits of a torn line's
times (cut_daemon_check.py's torn_line), one at a time, against the four
unit test files and tests/kernel/test_cut_record.sh; and next2_plants.py's
again where asked for by prefix (P1-P19 are the rule's).

  B1-B10 the bound of 73869d1

The file is restored after each, and each result is written to OUT as it
comes.

    next3_plants.py CHECKOUT OUT [PREFIX ...]
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import next2_plants  # noqa: E402,F401  (adds its plants; reads CHECKOUT, OUT and the prefixes from sys.argv)
import next_plants  # noqa: E402

J = next_plants.J
NXT = "    if not nxt.isdigit() or len(nxt) > 20:\n"
SKIP = "        if len(m.group(1)) > len(nxt) or len(h.group(1).lstrip('0')) > len(nxt):\n            continue\n"

next_plants.NEW += [
    ('B1 no bound on the digits (torn_line as 3fe4133 had it: T3 as it was)', J,
     NXT, "    if not nxt.isdigit():\n", SKIP, ""),
    ('B2 the start\'s time is not held to the next line\'s digits', J,
     SKIP, SKIP.replace("len(m.group(1)) > len(nxt) or ", "")),
    ('B3 the torn line\'s own time is not held to the next line\'s digits', J,
     SKIP, SKIP.replace(" or len(h.group(1).lstrip('0')) > len(nxt)", "")),
    ('B4 the torn line\'s own time is counted with its leading zeros', J,
     SKIP, SKIP.replace(".lstrip('0')", "")),
    ('B5 a start as long as the next line\'s time is skipped (> to >=)', J,
     SKIP, SKIP.replace("len(m.group(1)) > len(nxt)", "len(m.group(1)) >= len(nxt)")),
    ('B6 a torn line\'s time as long as the next line\'s is skipped (> to >=)', J,
     SKIP, SKIP.replace("len(h.group(1).lstrip('0')) > len(nxt)", "len(h.group(1).lstrip('0')) >= len(nxt)")),
    ('B7 a next line\'s time of any length is taken (no bound of 20)', J,
     NXT, "    if not nxt.isdigit():\n"),
    ('B8 no torn line where the next line\'s time has more than 12 digits (13 after 1000 s)', J,
     NXT, NXT.replace("> 20", "> 12")),
    ('B9 no torn line where the next line\'s time has more than 11 digits', J,
     NXT, NXT.replace("> 20", "> 11")),
    ('B10 a candidate with too long a time ends the search (continue to return None)', J,
     SKIP, SKIP.replace("continue", "return None")),
]

if __name__ == '__main__':
    next_plants.main()
