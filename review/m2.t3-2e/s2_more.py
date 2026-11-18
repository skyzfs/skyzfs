#!/usr/bin/env python3
"""#116 at 8e20e51 (the next look): more cases for S2 at a start that takes
up a state begun after an acknowledged failure, on s2_new_state.py's log
(its three cases print first).

    s2_more.py CHECKOUT
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import s2_new_state as s  # noqa: E402

ns, ns2 = s.ns, s.ns2
one = f'{ns2 + 5}\t> b1 release 1\n{ns2 + 6}\t< b1 ok (0 lines)\n'
late = one + f'{ns - 2}\t> b2 release 2\n{ns + 5}\t< b2 ok (0 lines)\n'
refused = one + f'{ns - 2}\t> b2 release 2\n{ns - 1}\t< b2 refused: not uploading\n'
s.judge('D  the release of the new state\'s cut 2 answered after the third start, cuts_held 0 (WRONG)', 0, s.first + late)
s.judge('E  the same, cuts_held 1 (RIGHT)', 1, s.first + late)
s.judge('F  the release of the new state\'s cut 2 refused, cuts_held 0 (WRONG)', 0, s.first + refused)
s.judge('G  the same, cuts_held 1 (RIGHT)', 1, s.first + refused)
s.judge('H  no release of the new state at all, the old state\'s 1 to 13 answered, cuts_held 0 (WRONG)', 0, s.first)
s.judge('I  no holder\'s release at all, cuts_held 0 (WRONG)', 0, '')
