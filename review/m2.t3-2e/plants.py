#!/usr/bin/env python3
"""#116 at 49c5f62 (M2.T3 2e): plants, one at a time, in what the pull
request changed, against the unit tests of the changed files.

    plants.py CHECKOUT OUT [PREFIX ...]

A plant is a text of a file replaced by another. One whose text is not
found exactly once says NOT APPLIED. The file is restored after each
(try/finally), each result is appended to OUT as it comes, a plant whose
name OUT already holds is skipped, and the script refuses to start on a
checkout that `git status --short` does not call clean. Each test file
runs under a limit of its own; a file that does not end in it counts as
failed. The Python test files run for every plant, and the first that
fails ends the plant's run.

The older lists (review/m2.t3-2b, -2c and -2d) run on this head with
review/m2.t3-2d/plants.py; their results are the other plants-*.out
files here.
"""
import os
import subprocess
import sys

ROOT, OUT = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
J = 'tests/kernel/cut_daemon_check.py'
PY_TESTS = ['tests/kernel/test_cut_ref.py', 'tests/kernel/test_cut_check.py', 'tests/kernel/test_cut_hold.py',
            'tests/kernel/test_cut_daemon_check.py', 'tests/kernel/test_cut_daemon_main.py']
SH_TESTS = {'tests/kernel/cut-record.sh': 'tests/kernel/test_cut_record.sh'}

AGAIN_FOR = "            for later in instances[k + 1:]:\n"
ACK_BREAK = "                if later['pid'] in acked:\n                    break\n"
C_SAME = "                if again is not None and again['sectors'] == tail[1]['sectors']:\n"
M_IF = ("            elif lines and lines[-1]['event'] == 'applied' and k + 1 < len(instances) \\\n"
        "                    and instances[k + 1]['pid'] not in acked and reapplied(dict(inst, lines=lines), instances[k + 1]):\n")
M_CUT = ("'shape': 'M', 'number': None,\n                              'group': None})\n"
         "                lines = lines[:-1]\n")
RE_CUT = "        if ln['event'] == 'cut':\n            return False\n        if ln['event'] == 'recv':\n"
RE_MATCH = ("        elif ln['event'] == 'applied' and rng.get(ln['tag']) == (recv['op'], recv['sector'], recv['sectors']) \\\n"
            "                and ln['extra'][:1] == last['extra'][:1]:\n")
BEFORE = "                before = not zeroes and seq is not None and b in unplaced and seq < unplaced[b]\n"
S2_IF = "            if counted and max(counted) > max(gone, default=0) and not (held.isdigit() and int(held) >= 1):\n"
S2_GONE = "            gone = [n for t, n in released if t < inst['ns']]\n"
S2_K = "        if k and not begun[k]:\n            counted = "
S2_COUNTED = ("            counted = [int(d['number']) for d in dcuts if lo < d['prefix'] < bounds[k - 1]['i']\n"
              "                       and d['number'].isdigit()]\n")
REL_OK = ("        elif len(w) >= 3 and w[0] == '<' and w[1] in asked and w[2] == 'ok':\n"
          "            out.append((int(f[0]), asked.pop(w[1])))\n")
ACROSS = "        across = [x for x in held_across_start(records, dcuts, written, blocks) if x[3]]\n"

PLANTS = [
    # R1: window C at whatever group, the next cut line of the state in whichever later instance
    ('R1a window C only where the next cut line is of the same group (as before #116)', J, C_SAME,
     "                if again is not None and (again['sectors'], again['result']) == (tail[1]['sectors'],\n"
     "                                                                                 tail[1]['result']):\n"),
    ('R1b the next cut line is looked for in the next instance only', J, AGAIN_FOR,
     "            for later in instances[k + 1:k + 2]:\n"),
    ('R1c a start that acknowledged a failure does not end the search', J, ACK_BREAK, ''),
    ('R1d a start that acknowledged a failure is passed over, and the search goes on', J, ACK_BREAK,
     "                if later['pid'] in acked:\n                    continue\n"),
    ('R1e the last cut line of the later instance, not its first', J,
     "                again = next((ln for ln in later['lines'] if ln['event'] == 'cut'), None)\n",
     "                again = next((ln for ln in reversed(later['lines']) if ln['event'] == 'cut'), None)\n"),
    ('R1f window C whatever number the next cut line carries', J, C_SAME, "                if again is not None:\n"),
    ('R1g window C with no cut line after it', J, C_SAME, "                if True:\n"),
    ('R1h window C takes out the cut line and leaves the barrier', J, "                    lines = lines[:-2]\n",
     "                    lines = lines[:-1]\n"),
    ('R1i window B: the next cut one past, not two, keeps the barrier', J,
     "            skips = again is not None and again['sectors'] == str(n + 2)\n",
     "            skips = again is not None and again['sectors'] == str(n + 1)\n"),
    ('R1j window B: the barrier is never kept', J,
     "            skips = again is not None and again['sectors'] == str(n + 2)\n", "            skips = False\n"),
    # F2: window M
    ('M1 window M: whatever the request that ends the instance was', J,
     "    if recv is None or recv['op'] != 'write':\n        return False\n", "    if recv is None:\n        return False\n"),
    ('M2 window M: a cut line before the write comes again does not keep the line', J, RE_CUT,
     "        if ln['event'] == 'recv':\n"),
    ('M3 window M: any write applied by the next instance counts as the same', J, RE_MATCH,
     "        elif ln['event'] == 'applied' and rng.get(ln['tag']) is not None:\n"),
    ('M4 window M: the same range, whatever its content', J, RE_MATCH,
     "        elif ln['event'] == 'applied' and rng.get(ln['tag']) == (recv['op'], recv['sector'], recv['sectors']):\n"),
    ('M5 window M: the same content, wherever', J, RE_MATCH,
     "        elif ln['event'] == 'applied' and rng.get(ln['tag']) is not None \\\n"
     "                and ln['extra'][:1] == last['extra'][:1]:\n"),
    ('M6 window M before a start that acknowledged a failure too', J, M_IF,
     "            elif lines and lines[-1]['event'] == 'applied' and k + 1 < len(instances) \\\n"
     "                    and reapplied(dict(inst, lines=lines), instances[k + 1]):\n"),
    ('M7 window M: the applied line is not taken out', J, M_CUT,
     "'shape': 'M', 'number': None,\n                              'group': None})\n"),
    ('M8 window M: never', J, M_IF, "            elif False:\n"),
    ('M9 window M: an instance that ends on any line of the write', J, M_IF,
     "            elif lines and lines[-1]['event'] in ('applied', 'commit', 'label', 'recv') and k + 1 < len(instances) \\\n"
     "                    and instances[k + 1]['pid'] not in acked and reapplied(dict(inst, lines=lines), instances[k + 1]):\n"),
    ('M10 window M: the write applied again after the next instance cut counts too (the cut line ends nothing)', J,
     "        if ln['event'] == 'cut':\n            return False\n        if ln['event'] == 'recv':\n",
     "        if ln['event'] == 'cut':\n            continue\n        if ln['event'] == 'recv':\n"),
    ('M11 the line counts a killed write among the flushes', J,
     "    flushes = [n for n in notes if n['shape'] != 'M']\n", "    flushes = list(notes)\n"),
    ('M12 the line does not say how many writes', J, "if writes else '')", "if False else '')"),
    ('M13 the line calls a killed write a cut that was cut again', J,
     "                      if n['shape'] == 'M' else\n", "                      if n['shape'] == 'm' else\n"),
    ('M14 window M is looked at before window B', J,
     "            if lines and lines[-1]['event'] == 'barrier' and not skips:\n",
     "            if lines and lines[-1]['event'] == 'barrier' and not skips and False:\n"),
    # R2: held_across_start and a write-zeroes
    ('R2a a write received before a write-zeroes and applied after it in the log is known', J, BEFORE,
     "                before = False\n"),
    ('R2b the write received after the write-zeroes is the one not known', J, BEFORE,
     "                before = not zeroes and seq is not None and b in unplaced and seq > unplaced[b]\n"),
    ('R2c every write after a write-zeroes that left the block unplaced is not known', J, BEFORE,
     "                before = not zeroes and b in unplaced\n"),
    ('R2d every block of the write-zeroes is unplaced, not its ambiguous set', J,
     "                for b in r.get('ambiguous', ()):\n                    unplaced[b] = seq\n",
     "                for b in range(first, (r['offset'] + r['bytes'] - 1) // BLOCK + 1):\n                    unplaced[b] = seq\n"),
    # S2: cuts_held at a start that takes the state up
    ('S2a cuts_held 0 will do', J, S2_IF,
     "            if counted and max(counted) > max(gone, default=0) and not (held.isdigit() and int(held) >= 0):\n"),
    ('S2b held is wanted though the newest cut was released', J, S2_IF,
     "            if counted and max(counted) >= max(gone, default=0) and not (held.isdigit() and int(held) >= 1):\n"),
    ('S2c a release answered after the start counts as before it', J, S2_GONE,
     "            gone = [n for t, n in released]\n"),
    ('S2d a status with no cuts_held line will do', J, S2_IF,
     "            if counted and max(counted) > max(gone, default=0) and held != '' and not (held.isdigit() and int(held) >= 1):\n"),
    ('S2e only at a take-over, not at a start that created the device', J, S2_K,
     "        if k and not begun[k] and bounds[k - 1]['taken_over']:\n            counted = "),
    ('S2f only at a start that created the device', J, S2_K,
     "        if k and not begun[k] and not bounds[k - 1]['taken_over']:\n            counted = "),
    ('S2g the cuts counted before the start, of whatever state', J, S2_COUNTED,
     "            counted = [int(d['number']) for d in dcuts if d['prefix'] < bounds[k - 1]['i']\n"
     "                       and d['number'].isdigit()]\n"),
    ('S2h a release the daemon refused counts as answered', J, REL_OK,
     "        elif len(w) >= 3 and w[0] == '<' and w[1] in asked:\n"
     "            out.append((int(f[0]), asked.pop(w[1])))\n"),
    ('S2i no release is read', J, "    released = releases(hlog)\n", "    released = []\n"),
    ('S2j the rule is never asked', J, S2_K, "        if False:\n            counted = "),
    ('S2k any release before the start will do, of whatever cut', J, S2_IF,
     "            if counted and not gone and not (held.isdigit() and int(held) >= 1):\n"),
    ('S2l the PASS line does not show cuts_held', J, ", cuts_held {st.get('cuts_held')!r}; the intent map named", "; the intent map named"),
    # G2, G3: the floors
    ('G2a a cut written across the start with no block from a copy meets the floor', J, ACROSS,
     "        across = [x for x in held_across_start(records, dcuts, written, blocks)]\n"),
    ('G3a held_across_start under its floor is not the rig\'s error', J,
     "        errored = errored or not across\n", "        errored = errored or False\n"),
    ('G3b carried_across_failure under its floor is not the rig\'s error', J,
     "        errored = errored or not ok\n", "        errored = errored or False\n"),
    ('G3c main: an ERROR line with no verdict ends 0', J, "    if errored and rc == 0:\n        rc = 2\n",
     "    if False:\n        rc = 2\n"),
    ('G3d main: an ERROR line with a verdict ends 2', J, "    if errored and rc == 0:\n        rc = 2\n",
     "    if errored:\n        rc = 2\n"),
    ('G3e main drops held_lines\' ERROR (the whole recording)', J,
     "                rc, errored = max(rc, v), errored or e\n        elif len(instances) > 1:\n",
     "                rc, errored = max(rc, v), errored\n        elif len(instances) > 1:\n"),
    ('G3f main drops held_lines\' ERROR (a holder that ended with a verdict)', J,
     "                rc, errored = max(rc, v), errored or e\n            else:\n",
     "                rc, errored = max(rc, v), errored\n            else:\n"),
    ('G3g held_across_start under its floor reads INFO', J, "        put('held_across_start', 'INFO' if across else 'ERROR', '; '.join(\n",
     "        put('held_across_start', 'INFO', '; '.join(\n"),
    ('G3h carried_across_failure under its floor reads INFO', J,
     "        put('carried_across_failure', 'INFO' if ok else 'ERROR',\n", "        put('carried_across_failure', 'INFO',\n"),
    ('G3i carried_across_failure: a cut with no block that differs meets the floor', J,
     "        ok = got is not None and got[2] > 0\n", "        ok = got is not None\n"),
]


# The plants of review/m2.t3-2d's lists whose text #116 changed, planted
# in this head's text (the names say which they were).
C_IF = ("            elif len(tail) == 2 and [ln['event'] for ln in tail] == ['barrier', 'cut'] \\\n"
        "                    and tail[0]['tag'] == tail[1]['tag']:\n")
H_SET = "                if b in held:\n                    home[b] = None if hs is None or before else hs[k]\n"
H_SUM = "        hit = sum(1 for b, h in home.items() if h is not None and h != held[b])\n"
LOG_F = "    for ln in log.splitlines():\n        f = ln.split('\\t')\n        if len(f) != 2 or not f[0].isdigit():\n            continue\n"
PLANTS += [
    ('again U2 a cut line cut again by a later instance is kept (window C never)', J, C_SAME, "                if False:\n"),
    ('again U7 window C by the group alone (any number)', J, C_SAME,
     "                if again is not None and again['result'] == tail[1]['result']:\n"),
    ('again U10 window C whatever flush the barrier before the cut line is of', J, C_IF,
     C_IF.replace(" \\\n                    and tail[0]['tag'] == tail[1]['tag']", "")),
    ('again U15 window C where the cut line ends the instance, whatever stands before it', J, C_IF,
     "            elif len(tail) == 2 and tail[1]['event'] == 'cut':\n"),
    ('again U18 the last instance\'s barrier at the end of the log is taken out too', J,
     "        if k + 1 < len(instances):\n            tail = lines[-2:]\n", "        if True:\n            tail = lines[-2:]\n"),
    ('again H7 a block that is not of the cut\'s map counts', J,
     H_SET, "                if True:\n                    home[b] = None if hs is None or before else hs[k]\n",
     H_SUM, H_SUM.replace("h != held[b]", "h != held.get(b)")),
    ('again L1 any write that differs counts, whatever was written after it', J, H_SET,
     "                if b in held and not (hs is not None and hs[k] == held[b] and b in home):\n"
     "                    home[b] = None if hs is None or before else hs[k]\n"),
    ('again L2 the first write to a block decides, not the last', J, H_SET,
     "                if b in held:\n                    home.setdefault(b, None if hs is None or before else hs[k])\n"),
    ('again L5 a write-zeroes leaves zeros, which are known', J, H_SET,
     "                if b in held:\n"
     "                    home[b] = (ZERO if zeroes else None) if hs is None or before else hs[k]\n"),
    ('again L6 a write of no known content is passed over (the write before it decides)', J, H_SET,
     "                if b in held and hs is not None and hs[k] is not None and not before:\n"
     "                    home[b] = hs[k]\n"),
    ('again L7 a block once not known stays so, whatever is written after', J, H_SET,
     "                if b in held and home.get(b, 0) is not None:\n"
     "                    home[b] = None if hs is None or before else hs[k]\n"),
    ('again L9 no block counts', J, H_SUM,
     "        hit = sum(1 for b, h in home.items() if h is not None and h != held[b] and False)\n"),
    ('again O4 first_commit: a line that is not a time and a text is the rig\'s error', J,
     "    asked = set()\n" + LOG_F,
     "    asked = set()\n" + LOG_F.replace("            continue\n", "            raise RigError('holder.log: not a time and a text')\n")),
    ('again O4 releases: a line that is not a time and a text is the rig\'s error', J,
     "    asked, out = {}, []\n" + LOG_F,
     "    asked, out = {}, []\n" + LOG_F.replace("            continue\n", "            raise RigError('holder.log: not a time and a text')\n")),
]


# review/m2.t3-2b/next_plants.py's F4, F6 and F7, whose text first_commit
# now shares with releases: planted in first_commit.
FC_HEAD = "    asked = set()\n    for ln in log.splitlines():\n        f = ln.split('\\t')\n        if len(f) != 2 or not f[0].isdigit():\n            continue\n"
FC_ANS = "            return int(f[0])\n    return None\n\n\ndef releases"
PLANTS += [
    ('again F4 first_commit: the last release answered, not the first', J,
     "    asked = set()\n    for ln in log.splitlines():\n", "    asked, last = set(), None\n    for ln in log.splitlines():\n",
     FC_ANS, "            last = int(f[0])\n    return last\n\n\ndef releases"),
    ('again F6 first_commit: a line that is not a time and a text ends the reading', J,
     FC_HEAD, FC_HEAD.replace("            continue\n", "            break\n")),
    ('again F7 first_commit: no release answered is time 0 (a commit before every start)', J,
     FC_ANS, "            return int(f[0])\n    return 0\n\n\ndef releases"),
    ('again F6 releases: a line that is not a time and a text ends the reading', J,
     "    asked, out = {}, []\n" + LOG_F, "    asked, out = {}, []\n" + LOG_F.replace("            continue\n", "            break\n")),
    ('again S2m releases: the release asked first is the one answered, whatever tag the answer carries', J,
     "            out.append((int(f[0]), asked.pop(w[1])))\n",
     "            out.append((int(f[0]), asked.pop(sorted(asked)[0])))\n"),
]


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
        for name, path, *pairs in PLANTS:
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
