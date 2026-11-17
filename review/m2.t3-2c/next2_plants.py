#!/usr/bin/env python3
"""#113 at ab34565: the plants of plants.py and next_plants.py again on
this head (those whose text is gone say NOT APPLIED), and new ones in
what ab34565 changed, one at a time, against the four unit test files
and tests/kernel/test_cut_record.sh:

  Y1-Y10 cut-record.sh: the wait for the holder's first cut, and the two
         recordings that end at a verdict there
  Z1-Z8  cut_daemon_check.py: a holder's verdict is judged before the
         lines that need a whole recording
  X5b    next_plants.py's X5 (the held cuts before the instances' lines),
         whose text ab34565 moved into a branch

The file is restored after each, and each result is written to OUT as it
comes.

    next2_plants.py CHECKOUT OUT [PREFIX ...]
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import next_plants  # noqa: E402  (reads CHECKOUT, OUT and the prefixes from sys.argv)

J, C = next_plants.J, next_plants.C

DEAD = ("        if ! kill -0 \"$HOLD_PID\" 2>/dev/null; then\n"
        "            holder_ended \"before it wrote a cut\"\n"
        "            grep -q \"$(printf '\\tVERDICT ')\" \"$RDIR/holder.log\" \\\n"
        "                || rig_error \"the holder ended with 0 before it wrote a cut: "
        "$(tail -n 2 \"$RDIR/holder.log\" 2>/dev/null)\"\n"
        "            return 1\n"
        "        fi\n")
TAKEOVER = "    # (a)\n    holder_wrote_one || { export_pool; return 0; }\n"
FAILED = "    create_pool\n    load 2\n    holder_wrote_one || { export_pool; return 0; }\n    holder_leave\n    load 1\n"
BRANCH = ("            if '\\tVERDICT ' in read_file(rdir, 'holder.log'):\n"
          "                v, e = held_lines(put, rdir, instances, records, dcuts, status, dev, name, begun)\n"
          "                rc, errored = max(rc, v), errored or e\n")
ORDER = ("                rc = max(rc, instance_lines(put, rdir, instances, dcuts, records, begun))\n"
         "                if name == 'failed':\n"
         "                    rc = max(rc, failure_line(put, rdir))\n"
         "                v, e = held_lines(put, rdir, instances, records, dcuts, status, dev, name, begun)\n"
         "                rc, errored = max(rc, v), errored or e\n")

next_plants.NEW += [
    # ---- cut-record.sh at ab34565: the wait for the holder's first cut ----
    ('Y1 the wait does not look whether the holder has ended (as before the commit: 300 s, then the rig\'s error)', C,
     DEAD, ""),
    ('Y2 the ended holder\'s status is not taken (a VERDICT line alone decides)', C,
     DEAD, DEAD.replace("            holder_ended \"before it wrote a cut\"\n", "")),
    ('Y3 a holder that ended with 0 before a cut ends the recording for the check too', C,
     DEAD, DEAD.replace("            grep -q \"$(printf '\\tVERDICT ')\" \"$RDIR/holder.log\" \\\n"
                        "                || rig_error \"the holder ended with 0 before it wrote a cut: "
                        "$(tail -n 2 \"$RDIR/holder.log\" 2>/dev/null)\"\n", "")),
    ('Y4 after a verdict before the first cut the wait says "wrote one" (the recording goes on)', C,
     DEAD, DEAD.replace("            return 1\n", "            return 0\n")),
    ('Y5 a verdict before the first cut is the rig\'s error (as before, said at once)', C,
     DEAD, DEAD.replace("            return 1\n", "            rig_error \"the holder ended before it wrote a cut\"\n")),
    ('Y6 takeover does not take the wait\'s 1 (set -e ends the recording with it, and no line)', C,
     TAKEOVER, "    # (a)\n    holder_wrote_one\n"),
    ('Y7 failed does not take the wait\'s 1', C,
     FAILED, FAILED.replace(" || { export_pool; return 0; }", "")),
    ('Y8 takeover ends at the verdict without exporting the pool', C,
     TAKEOVER, "    # (a)\n    holder_wrote_one || return 0\n"),
    ('Y9 failed goes on after the verdict (the wait\'s 1 is dropped)', C,
     FAILED, FAILED.replace(" || { export_pool; return 0; }", " || true")),
    ('Y10 the ended holder\'s status is taken by a bare wait', C,
     DEAD, DEAD.replace("            holder_ended \"before it wrote a cut\"\n",
                        "            wait \"$HOLD_PID\"; HOLD_PID=\"\"\n")),
    # ---- cut_daemon_check.py at ab34565: a holder's verdict first ----
    ('Z1 a verdict is not judged first (the order of e88773b for every recording)', J,
     "            if '\\tVERDICT ' in read_file(rdir, 'holder.log'):\n", "            if False:\n"),
    ('Z2 every recording with a holder is judged as one that ended at a verdict (no instance or failure line)', J,
     "            if '\\tVERDICT ' in read_file(rdir, 'holder.log'):\n", "            if True:\n"),
    ('Z3 after a verdict the failure line is judged as well', J,
     BRANCH, BRANCH.replace("                v, e = held_lines(", "                if name == 'failed':\n"
                            "                    rc = max(rc, failure_line(put, rdir))\n                v, e = held_lines(")),
    ('Z4 after a verdict the instances\' lines are judged as well (the failure line is not)', J,
     BRANCH, BRANCH.replace("                v, e = held_lines(", "                rc = max(rc, instance_lines(put, rdir, "
                            "instances, dcuts, records, begun))\n                v, e = held_lines(")),
    ('Z5 a verdict judged first does not count (rc as it was)', J,
     BRANCH, BRANCH.replace("                rc, errored = max(rc, v), errored or e\n", "                errored = errored or e\n")),
    ('Z6 held_lines goes on to the held cuts\' files after a verdict', J,
     "    if bad:\n        return 1, False\n", "    if bad:\n        rc = 1\n"),
    ('Z7 a verdict is judged first in failed only (takeover in the order of e88773b)', J,
     "            if '\\tVERDICT ' in read_file(rdir, 'holder.log'):\n",
     "            if name == 'failed' and '\\tVERDICT ' in read_file(rdir, 'holder.log'):\n"),
    ('Z8 a verdict is judged first in takeover only', J,
     "            if '\\tVERDICT ' in read_file(rdir, 'holder.log'):\n",
     "            if name == 'takeover' and '\\tVERDICT ' in read_file(rdir, 'holder.log'):\n"),
    ('X5b the held cuts are judged before the instances\' lines again (with no verdict)', J,
     ORDER,
     "                v, e = held_lines(put, rdir, instances, records, dcuts, status, dev, name, begun)\n"
     "                rc, errored = max(rc, v), errored or e\n"
     "                rc = max(rc, instance_lines(put, rdir, instances, dcuts, records, begun))\n"
     "                if name == 'failed':\n"
     "                    rc = max(rc, failure_line(put, rdir))\n"),
]

if __name__ == '__main__':
    next_plants.main()
