#!/usr/bin/env python3
"""#111 at 1f71211: plants in the commit's new logic of the judge
(tests/kernel/cut_daemon_check.py: first_commit, instance_lines), one at a
time, against its unit tests. The file is restored after each.

    next_plants.py CHECKOUT [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT = sys.argv[1]
ONLY = tuple(sys.argv[2:])
J = 'tests/kernel/cut_daemon_check.py'
TESTS = ['tests/kernel/test_cut_daemon_check.py', 'tests/kernel/test_cut_daemon_main.py']
ASK = "        if len(w) >= 3 and w[0] == '>' and w[2] == 'release':\n            asked.add(w[1])\n"
ANS = "        elif len(w) >= 3 and w[0] == '<' and w[1] in asked and w[2] == 'ok':\n            return int(f[0])\n"
LAX = "        if k and (committed is None or committed > inst['ns']):\n"
KEYS = "        for key, w in (('cut_state', want), ('intent_state', want_intent)):\n"

PLANTS = [
    # ---- first_commit ----
    ('F1 the time of the first release asked, answered or not', J,
     ASK, "        if len(w) >= 3 and w[0] == '>' and w[2] == 'release':\n            return int(f[0])\n"),
    ('F2 any command answered ok is a commit (a held, a write)', J,
     ANS, ANS.replace(" and w[1] in asked", "")),
    ('F3 a release refused is a commit', J,
     ANS, ANS.replace(" and w[2] == 'ok'", "")),
    ('F4 the last release answered, not the first', J,
     "    asked = set()\n    for ln in log.splitlines():\n", "    asked, last = set(), None\n    for ln in log.splitlines():\n",
     ANS, ANS.replace("            return int(f[0])\n", "            last = int(f[0])\n"),
     "    return None\n\n\ndef instance_lines", "    return last\n\n\ndef instance_lines"),
    ('F5 a write is asked for as a release is (its ok is a commit)', J,
     ASK, ASK.replace("w[2] == 'release'", "w[2] in ('release', 'write')")),
    ('F6 a line that is not a time and a text ends the reading', J,
     "        if len(f) != 2 or not f[0].isdigit():\n            continue\n", "        if len(f) != 2 or not f[0].isdigit():\n            break\n"),
    ('F7 no release answered is time 0 (a commit before every start)', J,
     "            return int(f[0])\n    return None\n\n\ndef instance_lines", "            return int(f[0])\n    return 0\n\n\ndef instance_lines"),
    # ---- instance_lines ----
    ('L1 every later start wants the map taken up (the rule before this commit)', J,
     LAX, "        if False:\n"),
    ('L2 every later start takes either, a commit before it or not', J,
     LAX, "        if k:\n"),
    ('L3 the first start takes either as well', J,
     LAX, "        if committed is None or committed > inst['ns']:\n"),
    ('L4 either only where a commit came after the start: with no commit at all the map must be taken up', J,
     LAX, "        if k and committed is not None and committed > inst['ns']:\n"),
    ('L5 either where the commit came before the start (the comparison turned round)', J,
     LAX, "        if k and (committed is None or committed < inst['ns']):\n"),
    ('L6 the commit is held to the first start\'s time, not this start\'s', J,
     LAX, "        if k and (committed is None or committed > instances[0]['ns']):\n"),
    ('L7 the commit is held to the start before this one', J,
     LAX, "        if k and (committed is None or committed > instances[k - 1]['ns']):\n"),
    ('L8 the cut state takes either too where the map does', J,
     KEYS, "        for key, w in (('cut_state', want_intent), ('intent_state', want_intent)):\n"),
    ('L9 the cuts line takes either too where the map does', J,
     "        if said != [want]:\n", "        if said != [want] and not (isinstance(want_intent, tuple) and said == ['begun']):\n"),
    ('L10 where either is taken, the map\'s state is not looked at (off, or nothing said, passes)', J,
     "            if st.get(key) not in (w if isinstance(w, tuple) else (w,)):\n",
     "            if not isinstance(w, tuple) and st.get(key) != w:\n"),
    ('L11 the line does not say that no cut was committed before the start', J,
     "            how += ', no cut committed before it'\n", "            pass\n"),
    ('L12 a begun map at a later start with a commit before it is no verdict (rc stays 0)', J,
     "        rc = 1 if bad else rc\n    return rc\n\n\ndef noflush_kstats", "        rc = 1 if bad and k == 0 else rc\n    return rc\n\n\ndef noflush_kstats"),
    ('L13 the first start\'s map is not looked at', J,
     KEYS, "        for key, w in (('cut_state', want),) + ((('intent_state', want_intent),) if k else ()):\n"),
]


def main():
    cache = {}
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
    for name, path, *pairs in PLANTS:
        if ONLY and not name.startswith(ONLY):
            continue
        full = os.path.join(ROOT, path)
        orig = cache.setdefault(full, open(full).read())
        counts = [orig.count(old) for old in pairs[0::2]]
        if any(c != 1 for c in counts) or any(old == new for old, new in zip(pairs[0::2], pairs[1::2])):
            print(f'{name}: NOT APPLIED (found {counts} times)', flush=True)
            continue
        text = orig
        for old, new in zip(pairs[0::2], pairs[1::2]):
            text = text.replace(old, new)
        try:
            with open(full, 'w') as f:
                f.write(text)
            failed = []
            for t in TESTS:
                try:
                    p = subprocess.run([sys.executable, '-B', t], cwd=ROOT, env=env, capture_output=True, text=True,
                                       timeout=300)
                except subprocess.TimeoutExpired:
                    failed.append(f'{os.path.basename(t)} did not end in 300 s')
                    continue
                failed += sorted({ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL: ', 'ERROR: '))})
                if p.returncode != 0 and not any(ln.startswith(('FAIL: ', 'ERROR: ')) for ln in p.stderr.splitlines()):
                    failed.append(f'{os.path.basename(t)} exit {p.returncode}')
            if failed:
                print(f'{name}: KILLED by {len(failed)}: {", ".join(failed[:6])}', flush=True)
            else:
                print(f'{name}: SURVIVED', flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)


if __name__ == '__main__':
    main()
