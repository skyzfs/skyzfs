#!/usr/bin/env python3
"""#113 at ab34565 and at 3fe4133: the plants of plants.py and
next_plants.py again (those whose text is gone on the head at hand say
NOT APPLIED), and new ones in what the two commits changed, one at a
time, against the four unit test files and
tests/kernel/test_cut_record.sh:

  Y1-Y10 cut-record.sh: the wait for the holder's first cut, and the two
         recordings that end at a verdict there
  Z1-Z8  cut_daemon_check.py: a holder's verdict is judged before the
         lines that need a whole recording
  X5b    next_plants.py's X5 (the held cuts before the instances' lines),
         whose text ab34565 moved into a branch
  P1-P19 cut_daemon_check.py at 3fe4133: the torn line on a page of the file
  U1-U8  cut-record.sh at 3fe4133: hold_idle and holder_gone

Y1 to Y5 and Y10 are of ab34565's text of the wait, which 3fe4133 moved
into holder_gone: U5 to U8 are their like there.

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

LOOP = "    for p in range(-offset % PAGE or PAGE, len(ln), PAGE):\n"
SKIP = "        if not m or m.group(2) in by_pid or not h:\n            continue\n"
WHOLE_PID = "        if h.group(3) is not None and h.group(2) != last['pid']:\n            continue\n"
PREFIX = "        if h.group(2) is not None and not last['pid'].startswith(h.group(2)):\n            continue\n"
START_BOUNDS = "        if not before <= ns <= int(nxt):\n"
TORN_BOUNDS = "        if h.group(2) is not None and not before <= int(h.group(1)) <= ns:\n"
GONE_W = "        kill -0 \"$HOLD_PID\" 2>/dev/null || { holder_gone \"before it wrote a cut\"; return 1; }\n"
GONE_I = "            kill -0 \"$HOLD_PID\" 2>/dev/null || { holder_gone \"while it was asked to pause\"; return 1; }\n"
GONE = ("holder_gone() {\n"
        "    holder_ended \"$1\"\n"
        "    grep -q \"$(printf '\\tVERDICT ')\" \"$RDIR/holder.log\" \\\n"
        "        || rig_error \"the holder ended with 0 $1: $(tail -n 2 \"$RDIR/holder.log\" 2>/dev/null)\"\n"
        "}\n")

next_plants.NEW += [
    # ---- cut_daemon_check.py at 3fe4133: the torn line on a page of the file ----
    ('P1 the start may follow at any byte of the line, not at a page of the file', J,
     LOOP, "    for p in range(1, len(ln)):\n"),
    ('P2 a page is 8192 bytes', J, "PAGE = 4096\n", "PAGE = 8192\n"),
    ('P3 a page is 2048 bytes', J, "PAGE = 4096\n", "PAGE = 2048\n"),
    ('P4 the pages are counted from the line\'s beginning, not the file\'s', J,
     LOOP, "    for p in range(PAGE, len(ln), PAGE):\n"),
    ('P5 the file\'s offset counts no newlines', J,
     "        at, offset = offset, offset + len(ln.encode()) + 1\n", "        at, offset = offset, offset + len(ln.encode())\n"),
    ('P6 the start line may carry a leading zero', J,
     "START_LINE = re.compile(r'([1-9]\\d*)\\t([1-9]\\d*)\\tstart", "START_LINE = re.compile(r'(\\d+)\\t(\\d+)\\tstart"),
    ('P7 the start may be of a pid already started (the author\'s survivor)', J,
     SKIP, "        if not m or not h:\n            continue\n"),
    ('P8 what is left may carry any whole pid', J, WHOLE_PID, ""),
    ('P9 what is left may carry any digits where its pid is cut', J, PREFIX, ""),
    ('P10 the start may be earlier than the last whole line', J, START_BOUNDS, "        if not ns <= int(nxt):\n"),
    ('P11 the start may be later than the line after it (the author\'s survivor)', J,
     START_BOUNDS, "        if not before <= ns:\n"),
    ('P12 the torn line\'s own time is not held to the lines around it', J, TORN_BOUNDS, "        if False:\n"),
    ('P13 the torn line may be earlier than the last whole line', J,
     TORN_BOUNDS, "        if h.group(2) is not None and not int(h.group(1)) <= ns:\n"),
    ('P14 the torn line may be later than the start', J,
     TORN_BOUNDS, "        if h.group(2) is not None and not before <= int(h.group(1)):\n"),
    ('P15 the torn line\'s daemon may write after it', J,
     "        if rec['pid'] in cut_short:\n", "        if False:\n"),
    ('P16 a start line later than the line after it is let through', J,
     "            if nxt.isdigit() and rec['ns'] > int(nxt):\n", "            if False:\n"),
    ('P17 only a line that does not parse is looked at (as e88773b)', J,
     "        if torn is not None:\n            cut = torn_line(",
     "        if torn is not None and (not 8 <= len(f) <= 10 or not f[0].isdigit()):\n            cut = torn_line("),
    ('P18 the last instance\'s start, not its last whole line, bounds the times', J,
     "    before = last['lines'][-1]['ns'] if last['lines'] else last['ns']\n", "    before = last['ns']\n"),
    ('P19 what is left must hold the whole time and its tab', J,
     "TORN_HEAD = re.compile(r'(\\d+)(?:\\t(\\d*)(\\t.*)?)?', re.S)", "TORN_HEAD = re.compile(r'(\\d+)\\t(\\d*)(\\t.*)?', re.S)"),
    # ---- cut-record.sh at 3fe4133: hold_idle and holder_gone ----
    ('U1 hold_idle does not look whether the holder has ended (as before: 120 s, then the rig\'s error)', C,
     GONE_I, ""),
    ('U2 after a verdict hold_idle says the holder is idle (the recording goes on)', C,
     GONE_I, GONE_I.replace("return 1", "return 0")),
    ('U3 edges does not take hold_idle\'s 1', C,
     "    hold_idle || { export_pool; return 0; }\n", "    hold_idle\n"),
    ('U4 edges ends at the verdict without exporting the pool', C,
     "    hold_idle || { export_pool; return 0; }\n", "    hold_idle || return 0\n"),
    ('U5 a holder that ended with 0 while the recording waits ends it for the check too', C,
     GONE, "holder_gone() {\n    holder_ended \"$1\"\n}\n"),
    ('U6 the gone holder\'s status is not taken (a VERDICT line alone decides)', C,
     GONE, GONE.replace("    holder_ended \"$1\"\n", "")),
    ('U7 the wait for the first cut does not look whether the holder has ended (300 s, then the rig\'s error)', C,
     GONE_W, ""),
    ('U8 after a verdict before the first cut the wait says "wrote one"', C,
     GONE_W, GONE_W.replace("return 1", "return 0")),
]

if __name__ == '__main__':
    next_plants.main()
