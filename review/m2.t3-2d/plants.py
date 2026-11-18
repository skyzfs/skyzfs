#!/usr/bin/env python3
"""#115 at afd217a (M2.T3 2d): plants, one at a time, against the unit
tests of the changed files.

    plants.py CHECKOUT OUT SET [PREFIX ...]

SET is one of:
  new      the plants of this review, in what #115 changed
           (cut_daemon_check.py: unstored_at_kill, held_across_start,
           across_failure, instance_lines, parse_log)
  2b       review/m2.t3-2b/plants.py's list (#111 at 8210332)
  2b-next  review/m2.t3-2b/next_plants.py's (#111 at 1f71211)
  2c       review/m2.t3-2c's lists: plants.py, next_plants.py,
           next2_plants.py, next3_plants.py (#113, four heads)
  again    plants of the older lists whose text has changed since,
           planted in this head's text (the names say which they were)
  head2    `new` for fe6970c: held_across_start's plants in that head's
           text, and L1 to L9 in its rule (a block counts by its last
           known content before the next start)

A plant is a text of a file replaced by another. One whose text is not
found exactly once says NOT APPLIED. The file is restored after each
(try/finally), each result is appended to OUT as it comes, a plant whose
name OUT already holds is skipped, and the script refuses to start on a
checkout that `git status --short` does not call clean. Each test file
runs under a limit of its own; a file that does not end in it counts as
failed. The Python test files run for every plant, and the first that
fails ends the plant's run; tests/kernel/test_cut_record.sh runs for a
plant in cut-record.sh, tests/lib/test_skz_store.sh for one in the
benchmark's script.
"""
import importlib.util
import os
import subprocess
import sys

ROOT, OUT, SET = sys.argv[1], sys.argv[2], sys.argv[3]
ONLY = tuple(sys.argv[4:])
HERE = os.path.dirname(os.path.abspath(__file__))
J = 'tests/kernel/cut_daemon_check.py'
BENCH = 'tests/bench/topology-bench.sh'
PY_TESTS = ['tests/kernel/test_cut_ref.py', 'tests/kernel/test_cut_check.py', 'tests/kernel/test_cut_hold.py',
            'tests/kernel/test_cut_daemon_check.py', 'tests/kernel/test_cut_daemon_main.py']
SH_TESTS = {'tests/kernel/cut-record.sh': 'tests/kernel/test_cut_record.sh',
            'tests/bench/topology-bench.sh': 'tests/lib/test_skz_store.sh'}

B_IF = "            if lines and lines[-1]['event'] == 'barrier' and not skips:\n"
SKIPS = "            skips = nxt['pid'] not in acked and again is not None and again['sectors'] == str(n + 2)\n"
C_IF = ("            elif len(tail) == 2 and [ln['event'] for ln in tail] == ['barrier', 'cut'] \\\n"
        "                    and tail[0]['tag'] == tail[1]['tag'] and nxt['pid'] not in acked:\n")
C_SAME = ("                if again is not None and (again['sectors'], again['result']) == (tail[1]['sectors'],\n"
          "                                                                                 tail[1]['result']):\n")
C_CUT = "                    lines = lines[:-2]\n"
N_OF = "            n = int(counted[-1]) if counted and counted[-1].isdigit() else last\n"
RESET = "        if k and inst['pid'] in acked:\n            last = 0\n"
USE = "        instances, unstored = unstored_at_kill(instances, acked)\n"
WROTE = "        if wrote <= counted:\n            continue\n        end = starts[counted]['i']\n"
H_RANGE = "        for r in records[d['prefix'] + 1:end]:\n            if cut_ref.is_write(r):\n"
H_HIT = ("                    if b in held and (hs is None or hs[k] is None or hs[k] != held[b]):\n"
         "                        hit.add(b)\n        out.append((w['number'], counted + 1, wrote + 1, len(hit)))\n")
H_FILTER = "        across = [x for x in held_across_start(records, dcuts, written, blocks) if x[3]]\n"
H_PUT = "        put('held_across_start', 'INFO' if across else 'ERROR', '; '.join(\n"
H_ERR = "        errored = errored or not across\n"
A_OLD = "        if d.get('epoch', 0) == 0:\n            image.update(mine)\n            continue\n"
A_RET = "        return w['number'], len(mine), sum(1 for b, h in mine.items() if image.get(b, ZERO) != h)\n"
A_OK = "        ok = got is not None and got[2] > 0\n"
A_PUT = "        put('carried_across_failure', 'INFO' if ok else 'ERROR',\n"
A_ERR = "        errored = errored or not ok\n"
G_J = "        j = max(i for i in range(k + 1) if begun[i])\n"
G_LO = "        lo = bounds[j - 1]['i'] if j else -1\n"
G_HELD = "        if k and begun[k] and st.get('cuts_held') != '0':\n"
T_BOUND = "        if len(f[0]) > 20:\n"
T_NEXT = "            if nxt.isdigit() and len(nxt) <= 20 and rec['ns'] > int(nxt):\n"
T_HEAD = "TORN_HEAD = re.compile(r'([1-9]\\d*)(?:\\t(\\d*)(\\t.*)?)?', re.S)\n"
T_TORN = "    if not nxt.isdigit() or len(nxt) > 20:\n        return None\n"
MISS = "        if len(rbefore) == before + 1 and rbefore[-1]['prefix'] == last and after \\\n"

NEW = [
    # ---- unstored_at_kill (D1 of the review of #111) ----
    ('U1 a barrier with no cut line at a kill is kept (shape B as before the pull request)', J,
     B_IF, "            if False:\n"),
    ('U2 a cut line cut again by the next instance is kept (shape C as before)', J,
     C_SAME, "                if False:\n"),
    ('U3 the barrier is taken out even where the next instance counts on past one more (shape A excused)', J,
     B_IF, "            if lines and lines[-1]['event'] == 'barrier':\n"),
    ('U4 a next instance that acknowledged a failure can count past one more too', J,
     SKIPS, "            skips = again is not None and again['sectors'] == str(n + 2)\n"),
    ('U5 past one more is n + 1 (the next number itself)', J,
     SKIPS, SKIPS.replace("str(n + 2)", "str(n + 1)")),
    ('U6 shape C by the number alone (any group)', J,
     C_SAME, "                if again is not None and again['sectors'] == tail[1]['sectors']:\n"),
    ('U7 shape C by the group alone (any number)', J,
     C_SAME, "                if again is not None and again['result'] == tail[1]['result']:\n"),
    ('U8 shape C before a start that acknowledged a failure too', J,
     C_IF, C_IF.replace(" and nxt['pid'] not in acked", "")),
    ('U9 shape C takes out the cut line and leaves its barrier', J,
     C_CUT, "                    lines = lines[:-1]\n"),
    ('U10 shape C whatever flush the barrier before the cut line is of', J,
     C_IF, C_IF.replace("tail[0]['tag'] == tail[1]['tag'] and ", "")),
    ('U11 the count before the kill is not begun again at a start that acknowledged a failure', J,
     RESET, ""),
    ('U12 the count before the kill is the instances\' before it, never this one\'s own lines', J,
     N_OF, "            n = last\n"),
    ('U13 main judges the log as it stood (the instances without the lines are dropped)', J,
     USE, "        _, unstored = unstored_at_kill(instances, acked)\n"),
    ('U14 main does not say which starts acknowledged a failure', J,
     USE, "        instances, unstored = unstored_at_kill(instances)\n"),
    ('U15 shape C where the cut line ends the instance, whatever stands before it', J,
     C_IF, "            elif len(tail) == 2 and tail[1]['event'] == 'cut' and nxt['pid'] not in acked:\n"),
    ('U16 shape C looks at the next instance\'s last cut, not its first', J,
     "            again = next((ln for ln in nxt['lines'] if ln['event'] == 'cut'), None)\n",
     "            again = next((ln for ln in reversed(nxt['lines']) if ln['event'] == 'cut'), None)\n"),
    ('U17 the line of a shape C says what a shape B\'s says', J,
     "                      if n['shape'] == 'B' else\n", "                      if True else\n"),
    ('U18 the last instance\'s barrier at the end of the log is taken out too', J,
     "        if k + 1 < len(instances):\n            nxt = instances[k + 1]\n",
     "        if True:\n            nxt = instances[k + 1] if k + 1 < len(instances) else {'pid': '', 'lines': []}\n"),
    ('M1 cut_line_missing: the reference need not have one cut more before the kill', J,
     MISS, MISS.replace("len(rbefore) == before + 1 and ", "")),
    # ---- held_across_start (G1 of the review of #111) ----
    ('H1 a cut written by the instance that counted it counts too (<= to <)', J,
     WROTE, "        if wrote < counted or counted >= len(starts):\n            continue\n        end = starts[counted]['i']\n"),
    ('H2 the writes up to the last start before the cut\'s write count, not up to the first after its instant', J,
     WROTE, WROTE.replace("starts[counted]['i']", "starts[wrote - 1]['i']")),
    ('H3 the writes before the cut\'s instant count too', J,
     H_RANGE, H_RANGE.replace("records[d['prefix'] + 1:end]", "records[:end]")),
    ('H4 the writes after the next start count too', J,
     H_RANGE, H_RANGE.replace("records[d['prefix'] + 1:end]", "records[d['prefix'] + 1:]")),
    ('H5 a block written again with the content the cut holds counts', J,
     H_HIT, H_HIT.replace("if b in held and (hs is None or hs[k] is None or hs[k] != held[b]):", "if b in held:")),
    ('H6 only a block written again with the content the cut holds counts', J,
     H_HIT, H_HIT.replace("hs[k] != held[b]", "hs[k] == held[b]")),
    ('H7 a block that is not of the cut\'s map counts', J,
     H_HIT, H_HIT.replace("if b in held and (", "if (")),
    ('H8 a cut held across a start with no block from a copy meets the floor', J,
     H_FILTER, H_FILTER.replace(" if x[3]]", "]")),
    ('H9 the floor\'s line is INFO whatever it found', J,
     H_PUT, H_PUT.replace("'INFO' if across else 'ERROR'", "'INFO'")),
    ('H10 a take-over that shows nothing is no error of the rig (the exit)', J,
     H_ERR, ""),
    ('H11 every recording is asked for a cut held across a start', J,
     "    if name == 'takeover':\n        across =", "    if True:\n        across ="),
    ('H12 the instance that wrote is counted by the starts before the cut line, not before the write was asked', J,
     "        wrote = sum(1 for r in starts if r['ns'] < w['asked'])\n",
     "        wrote = sum(1 for r in starts if r['ns'] < d.get('ns', w['asked']))\n"),
    # ---- across_failure (G2 of the review of #113) ----
    ('A1 the old state\'s image is left empty (every block of the first cut that is not zeros differs)', J,
     A_OLD, "        if d.get('epoch', 0) == 0:\n            continue\n"),
    ('A2 the blocks that are the same are counted', J,
     A_RET, A_RET.replace("image.get(b, ZERO) != h", "image.get(b, ZERO) == h")),
    ('A3 a block the old image does not hold differs whatever it holds (zeros too)', J,
     A_RET, A_RET.replace("image.get(b, ZERO) != h", "image.get(b) != h")),
    ('A4 a first cut of the new state that differs in no block meets the floor', J,
     A_OK, "        ok = got is not None\n"),
    ('A5 the floor\'s line is INFO whatever it found', J,
     A_PUT, A_PUT.replace("'INFO' if ok else 'ERROR'", "'INFO'")),
    ('A6 a failure that carries nothing is no error of the rig (the exit)', J,
     A_ERR, ""),
    ('A7 no recording is asked for it', J,
     "    if any(begun[1:]):\n        got = across_failure", "    if False:\n        got = across_failure"),
    ('A8 the old image is the cuts of every state but the last written', J,
     A_OLD, "        if d.get('epoch', 0) == 0 or w is not written[-1]:\n            image.update(mine)\n            continue\n"),
    # ---- instance_lines (G1 and G3 of the review of #113) ----
    ('G1 the cuts before a start are counted from the log\'s first line (as before)', J,
     G_LO, "        lo = -1\n"),
    ('G2 the cuts before a start are counted from the first start that began a state', J,
     G_J, "        j = min(i for i in range(k + 1) if begun[i])\n"),
    ('G3 a start that acknowledged a failure is not asked what it holds', J,
     G_HELD, "        if False:\n"),
    ('G4 a status with no cuts_held passes at an acknowledged start', J,
     G_HELD, "        if k and begun[k] and st.get('cuts_held', '0') != '0':\n"),
    ('G5 every later start must hold nothing', J,
     G_HELD, "        if k and st.get('cuts_held') != '0':\n"),
    # ---- parse_log and torn_line (the next look at #113) ----
    ('T1 parse_log takes a time of any length', J, T_BOUND, "        if False:\n"),
    ('T2 parse_log refuses a time of more than 12 digits', J, T_BOUND, "        if len(f[0]) > 12:\n"),
    ('T3 parse_log refuses a time of 20 digits', J, T_BOUND, "        if len(f[0]) > 19:\n"),
    ('T4 the start check converts a next line\'s time of any length', J,
     T_NEXT, "            if nxt.isdigit() and rec['ns'] > int(nxt):\n"),
    ('T5 a torn line\'s time may begin with zeros again', J,
     T_HEAD, "TORN_HEAD = re.compile(r'(\\d+)(?:\\t(\\d*)(\\t.*)?)?', re.S)\n"),
    ('T6 torn_line takes a next line\'s time of any length', J,
     T_TORN, "    if not nxt.isdigit():\n        return None\n"),
    ('T7 torn_line refuses where the next line\'s time has more than 12 digits', J,
     T_TORN, T_TORN.replace("> 20", "> 12")),
]


SKIP2 = "        if len(m.group(1)) > len(nxt) or len(h.group(1)) > len(nxt):\n            continue\n"
F_CALL = "                if name == 'failed':\n                    rc = max(rc, failure_line(put, rdir))\n"
AGAIN = [
    ('2b K2 cut_line_missing: the next instance\'s number is not looked at', J,
     MISS + "                and after[0].get('local', after[0]['number']) == str(before + 2):\n",
     "        if len(rbefore) == before + 1 and rbefore[-1]['prefix'] == last and after:\n"),
    ('2b K5 main does not ask cut_line_missing: a lost line is a verdict', J,
     "            lost = cut_line_missing(dcuts, ref, records)\n", "            lost = None\n"),
    ('2c F6 main does not judge the failure', J,
     F_CALL, "                if False:\n                    rc = max(rc, failure_line(put, rdir))\n"),
    ('2c F7 main judges the failure and drops the verdict', J,
     F_CALL, "                if name == 'failed':\n                    failure_line(put, rdir)\n"),
    ('2c F8 main judges a failure in every recording that held cuts', J,
     F_CALL, "                if True:\n                    rc = max(rc, failure_line(put, rdir))\n"),
    ('2c I4 the acknowledged start\'s count is held to the cuts before it', J,
     "        before = 0 if begun[k] else sum(", "        before = sum("),
    ('2b B3 skz_release_line does not cut the last line', BENCH,
     " | sed 's/^ //; s/ $//' | cut -c1-300) || last=\"\"", " | sed 's/^ //; s/ $//') || last=\"\""),
    ('2b B4 skz_release_line does not squeeze the last line\'s blanks', BENCH,
     "2>/dev/null | tr -s ' \\t' ' ' | sed 's/^ //; s/ $//' | cut", "2>/dev/null | sed 's/^ //; s/ $//' | cut"),
    ('2b B4 (as #115 has it) skz_release_line leaves the blanks at the ends of the last line', BENCH,
     "tr -s ' \\t' ' ' | sed 's/^ //; s/ $//' | cut -c1-300", "tr -s ' \\t' ' ' | cut -c1-300"),
    ('2b B4 (as #115 has it) only the blank at the beginning is taken', BENCH,
     "sed 's/^ //; s/ $//'", "sed 's/^ //'"),
    ('2b B4 (as #115 has it) only the blank at the end is taken', BENCH,
     "sed 's/^ //; s/ $//'", "sed 's/ $//'"),
    ('2c B1 no bound on the digits (torn_line as 3fe4133 had it)', J,
     T_TORN, "    if not nxt.isdigit():\n        return None\n", SKIP2, ""),
    ('2c B2 the start\'s time is not held to the next line\'s digits', J,
     SKIP2, SKIP2.replace("len(m.group(1)) > len(nxt) or ", "")),
    ('2c B3 the torn line\'s own time is not held to the next line\'s digits', J,
     SKIP2, SKIP2.replace(" or len(h.group(1)) > len(nxt)", "")),
    ('2c B5 a start as long as the next line\'s time is skipped (> to >=)', J,
     SKIP2, SKIP2.replace("len(m.group(1)) > len(nxt)", "len(m.group(1)) >= len(nxt)")),
    ('2c B6 a torn line\'s time as long as the next line\'s is skipped (> to >=)', J,
     SKIP2, SKIP2.replace("len(h.group(1)) > len(nxt)", "len(h.group(1)) >= len(nxt)")),
    ('2c B10 a candidate with too long a time ends the search (continue to return None)', J,
     SKIP2, SKIP2.replace("continue", "return None")),
    ('2c P16 a start line later than the line after it is let through', J,
     T_NEXT, "            if False:\n"),
    ('2c P19 what is left must hold the whole time and its tab', J,
     T_HEAD, "TORN_HEAD = re.compile(r'([1-9]\\d*)\\t(\\d*)(\\t.*)?', re.S)\n"),
    ('2c P2 a page is 8192 bytes', J, "PAGE = 4096\n", "PAGE = 8192\n"),
    ('2c P2 (16384) a page is 16384 bytes', J, "PAGE = 4096\n", "PAGE = 16384\n"),
]

# fe6970c: held_across_start counts a block by its last known content.
H2_RANGE = "        for r in records[d['prefix'] + 1:end]:\n            zeroes = r.get('op') == 'write_zeroes'\n"
H2_SET = "                if b in held:\n                    home[b] = None if hs is None else hs[k]\n"
H2_SUM = "        hit = sum(1 for b, h in home.items() if h is not None and h != held[b])\n"
H2_ZERO = "            zeroes = r.get('op') == 'write_zeroes'\n"
H2_HS = "            hs = None if zeroes else r.get('hashes')\n"
HEAD2 = [p for p in NEW if not p[0].startswith(('H3 ', 'H4 ', 'H5 ', 'H6 ', 'H7 '))] + [
    ('H3 the writes before the cut\'s instant count too', J,
     H2_RANGE, H2_RANGE.replace("records[d['prefix'] + 1:end]", "records[:end]")),
    ('H4 the writes after the next start count too', J,
     H2_RANGE, H2_RANGE.replace("records[d['prefix'] + 1:end]", "records[d['prefix'] + 1:]")),
    ('H6 only a block whose last content is the cut\'s counts', J,
     H2_SUM, H2_SUM.replace("h != held[b]", "h == held[b]")),
    ('H7 a block that is not of the cut\'s map counts', J,
     H2_SET, "                if True:\n                    home[b] = None if hs is None else hs[k]\n",
     H2_SUM, H2_SUM.replace("h != held[b]", "h != held.get(b)")),
    ('L1 any write that differs counts, whatever was written after it (as at afd217a)', J,
     H2_SET, "                if b in held and not (hs is not None and hs[k] == held[b] and b in home):\n"
     "                    home[b] = None if hs is None else hs[k]\n"),
    ('L2 the first write to a block decides, not the last', J,
     H2_SET, "                if b in held:\n                    home.setdefault(b, None if hs is None else hs[k])\n"),
    ('L3 a block whose last content is not known counts', J,
     H2_SUM, H2_SUM.replace("h is not None and h != held[b]", "h is None or h != held[b]")),
    ('L4 a write-zeroes is not looked at (the write before it decides)', J,
     H2_ZERO, "            zeroes = False\n"),
    ('L5 a write-zeroes leaves zeros, which are known', J,
     H2_SET, "                if b in held:\n                    home[b] = (ZERO if zeroes else None) if hs is None else hs[k]\n"),
    ('L6 a write of no known content is passed over (the write before it decides)', J,
     H2_SET, "                if b in held and hs is not None and hs[k] is not None:\n                    home[b] = hs[k]\n"),
    ('L7 a block once not known stays so, whatever is written after', J,
     H2_SET, "                if b in held and home.get(b, 0) is not None:\n"
     "                    home[b] = None if hs is None else hs[k]\n"),
    ('L8 a write-zeroes is read as a write with hashes', J,
     H2_HS, "            hs = r.get('hashes')\n"),
    ('L9 no block counts', J,
     H2_SUM, "        hit = sum(1 for b, h in home.items() if h is not None and h != held[b] and False)\n"),
]

def load(name, path, argv):
    """An older review's plant script as a module; it reads sys.argv when imported."""
    saved = sys.argv
    sys.argv = argv
    try:
        sys.path.insert(0, os.path.dirname(path))
        spec = importlib.util.spec_from_file_location(name, path)
        mod = importlib.util.module_from_spec(spec)
        sys.modules[name] = mod
        spec.loader.exec_module(mod)
        return mod
    finally:
        sys.argv = saved


def plants_of(which):
    if which == 'new':
        return NEW
    if which == 'again':
        return AGAIN
    if which == 'head2':
        return HEAD2
    b, c = os.path.join(HERE, '..', 'm2.t3-2b'), os.path.join(HERE, '..', 'm2.t3-2c')
    if which == '2b':
        return load('p2b', os.path.join(b, 'plants.py'), ['x', ROOT]).PLANTS
    if which == '2b-next':
        return load('p2b_next', os.path.join(b, 'next_plants.py'), ['x', ROOT]).PLANTS
    if which == '2c':
        load('next3_plants', os.path.join(c, 'next3_plants.py'), ['x', ROOT, OUT])
        return sys.modules['plants'].PLANTS + sys.modules['next_plants'].NEW
    sys.exit(__doc__)


def run_tests(env, path):
    """-> the names of what failed, [] when every test passes."""
    for t in PY_TESTS:
        try:
            p = subprocess.run([sys.executable, '-B', t], cwd=ROOT, env=env, capture_output=True, text=True,
                               timeout=300)
        except subprocess.TimeoutExpired:
            return [f'{os.path.basename(t)} did not end in 300 s']
        failed = sorted({ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL: ', 'ERROR: '))})
        if p.returncode != 0 and not failed:
            failed = [f'{os.path.basename(t)} exit {p.returncode}']
        if failed:
            return failed
    sh = SH_TESTS.get(path)
    if sh and os.path.exists(os.path.join(ROOT, sh)):
        try:
            p = subprocess.run(['bash', sh], cwd=ROOT, env=env, capture_output=True, text=True, timeout=300)
        except subprocess.TimeoutExpired:
            return [f'{os.path.basename(sh)} did not end in 300 s']
        bad = [ln for ln in p.stdout.splitlines() if ln.startswith(('FAIL ', 'not ok', 'FAIL:'))]
        if bad or p.returncode != 0:
            return [f'{os.path.basename(sh)}: {ln[:80]}' for ln in bad] or [f'{os.path.basename(sh)} exit {p.returncode}']
    return []


def main():
    dirty = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True, timeout=120)
    if dirty.returncode != 0 or dirty.stdout.strip():
        sys.exit(f'{ROOT} is not a clean checkout: {dirty.stdout.strip() or dirty.stderr.strip()}')
    done = []
    if os.path.exists(OUT):
        with open(OUT) as f:
            done = f.read().splitlines()
    cache = {}
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
    with open(OUT, 'a') as out:
        def say(text):
            print(text, flush=True)
            out.write(text + '\n')
            out.flush()
            os.fsync(out.fileno())
        for name, path, *pairs in plants_of(SET):
            if ONLY and not name.startswith(ONLY):
                continue
            if any(ln.startswith(name + ': ') for ln in done):
                continue
            full = os.path.join(ROOT, path)
            if full not in cache:
                with open(full) as f:
                    cache[full] = f.read()
            orig = cache[full]
            counts = [orig.count(old) for old in pairs[0::2]]
            if any(c != 1 for c in counts) or any(old == new for old, new in zip(pairs[0::2], pairs[1::2])):
                say(f'{name}: NOT APPLIED (found {counts} times)')
                continue
            text = orig
            for old, new in zip(pairs[0::2], pairs[1::2]):
                text = text.replace(old, new)
            try:
                with open(full, 'w') as f:
                    f.write(text)
                failed = run_tests(env, path)
            finally:
                with open(full, 'w') as f:
                    f.write(orig)
            say(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(failed[:6])}' if failed else 'SURVIVED'))


if __name__ == '__main__':
    main()
