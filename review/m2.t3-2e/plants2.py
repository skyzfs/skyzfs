#!/usr/bin/env python3
"""#116 at 8e20e51 (the next look): plants in what the commit changed, one
at a time, by plants.py's runner (the same rules: a text replaced once,
the file restored after each, each result appended to OUT as it comes).

    plants2.py CHECKOUT OUT [PREFIX ...]

plants.py's own 69 run on this head as they are (plants-new-8e20e51.out);
those of them whose text the commit changed are planted here in the new
text, under the old names with `again`.
"""
import importlib.util
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location('plants', os.path.join(HERE, 'plants.py'))
p = importlib.util.module_from_spec(spec)
spec.loader.exec_module(p)
J = p.J
T = 'tests/kernel/test_cut_daemon_main.py'

CUT = ("        if ln['event'] == 'cut':\n            if not again or ln['tag'] in early:\n"
       "                return False\n            return True\n")
CUT_IF = "            if not again or ln['tag'] in early:\n"
EARLY = "            if ln['op'] == 'flush' and not again:\n"
DISCARD = "            else:\n                early.discard(ln['tag'])\n"
MATCH = ("        elif not again and ln['event'] == 'applied' \\\n"
         "                and rng.get(ln['tag']) == (recv['op'], recv['sector'], recv['sectors']) \\\n"
         "                and ln['extra'][:1] == last['extra'][:1]:\n")
END = "            again = True\n    return again\n"
COUNTED = ("            counted = [int(d.get('local', d['number'])) for d in dcuts if lo < d['prefix'] < bounds[k - 1]['i']\n"
           "                       and d.get('local', d['number']).isdigit()]\n")
GONE = "            gone = [n for t, n in released if since < t < inst['ns']]\n"
SINCE = "            since = instances[j]['ns'] if j else -1\n"
PATCH = "        with mock.patch.object(cdc, 'held_lines', held(cdc.held_lines)), mock.patch.object(sys, 'argv', argv), \\\n"

p.PLANTS = [
    # D1: reapplied
    ('D1a the rule of 49c5f62: applied again before any cut line', J,
     CUT, "        if ln['event'] == 'cut':\n            return False\n", END, "            return True\n    return False\n"),
    ('D1b any cut line keeps the line, a later flush\'s too', J, CUT_IF, "            if True:\n"),
    ('D1c a cut after the second applied line never keeps the line, an early flush\'s neither', J, CUT_IF,
     "            if not again:\n"),
    ('D1d no cut line keeps the line', J, CUT, ''),
    ('D1e a flush received after the second applied line is early too', J, EARLY, "            if ln['op'] == 'flush':\n"),
    ('D1f a tag used again stays early', J, DISCARD, ''),
    ('D1g no write applied again and no cut: the line is kept whatever', J,
     "    return again\n\n\ndef unstored_at_kill", "    return False\n\n\ndef unstored_at_kill"),
    ('D1h the line is taken out though the write was not applied again', J,
     "    return again\n\n\ndef unstored_at_kill", "    return True\n\n\ndef unstored_at_kill"),
    ('D1i the tag alone decides at a cut line (no `not again`)', J, CUT_IF, "            if ln['tag'] in early:\n"),
    ('D1j a later flush\'s cut does not end the reading', J,
     "                return False\n            return True\n", "                return False\n            continue\n"),
    ('D1k every request received before the second applied line is early, not only a flush', J, EARLY,
     "            if not again:\n"),
    ('D1m a cut before the write comes again takes the line out', J, CUT_IF,
     "            if again and ln['tag'] in early:\n"),
    ('D1n the early flushes are forgotten at the second applied line', J, "            again = True\n    return again\n",
     "            again = True\n            early.clear()\n    return again\n"),
    ('D1o a tag is discarded at every receipt (no flush is early)', J, EARLY, "            if False:\n"),
    ('again M3 any write applied by the next instance counts as the same', J, MATCH,
     "        elif not again and ln['event'] == 'applied' and rng.get(ln['tag']) is not None:\n"),
    ('again M4 the same range, whatever its content', J, MATCH,
     "        elif not again and ln['event'] == 'applied' \\\n"
     "                and rng.get(ln['tag']) == (recv['op'], recv['sector'], recv['sectors']):\n"),
    ('again M5 the same content, wherever', J, MATCH,
     "        elif not again and ln['event'] == 'applied' and rng.get(ln['tag']) is not None \\\n"
     "                and ln['extra'][:1] == last['extra'][:1]:\n"),
    # D2: the state's own numbers and releases
    ('D2a the check\'s running numbers (as at 49c5f62)', J, COUNTED,
     "            counted = [int(d['number']) for d in dcuts if lo < d['prefix'] < bounds[k - 1]['i']\n"
     "                       and d['number'].isdigit()]\n"),
    ('D2b every release before the start counts, an older state\'s too', J, GONE,
     "            gone = [n for t, n in released if t < inst['ns']]\n"),
    ('D2c a release answered after the start counts (again S2c)', J, GONE,
     "            gone = [n for t, n in released if since < t]\n"),
    ('D2d only the releases since the start before this one', J, SINCE,
     "            since = instances[k - 1]['ns'] if k > 1 else -1\n"),
    ('D2e the releases since the second start, whichever began the state', J, SINCE,
     "            since = instances[1]['ns'] if j else -1\n"),
    ('D2f the cuts counted before the start, of whatever state (again S2g)', J, COUNTED,
     "            counted = [int(d.get('local', d['number'])) for d in dcuts if d['prefix'] < bounds[k - 1]['i']\n"
     "                       and d.get('local', d['number']).isdigit()]\n"),
    ('D2g a release at the very time of the start that began the state counts', J, GONE,
     "            gone = [n for t, n in released if since <= t < inst['ns']]\n"),
    # the test helper's in-process main: what the new test rests on
    ('T1 the helper does not replace held_lines', T, PATCH,
     "        with mock.patch.object(cdc, 'held_lines', cdc.held_lines), mock.patch.object(sys, 'argv', argv), \\\n"),
    ('T2 the helper takes any end of main as exit 2', T, "        rc = code[0]\n", "        rc = 2\n"),
    ('T3 the helper takes any end of main as exit 0', T, "        rc = code[0]\n", "        rc = 0\n"),
    ('T4 main ends 2 whatever the flag when held_lines is called (after the instances)', J,
     "                rc, errored = max(rc, v), errored or e\n        elif len(instances) > 1:\n",
     "                rc, errored = max(rc, v), True\n        elif len(instances) > 1:\n"),
    ('T5 main ends 2 whatever the flag when held_lines is called (after a verdict)', J,
     "                rc, errored = max(rc, v), errored or e\n            else:\n",
     "                rc, errored = max(rc, v), True\n            else:\n"),
    ('T6 main: the flag of the call after the instances is the first call\'s only', J,
     "                rc, errored = max(rc, v), errored or e\n        elif len(instances) > 1:\n",
     "                rc, errored = max(rc, v), errored and e\n        elif len(instances) > 1:\n"),
    ('T7 main: an ERROR flag ends 1', J, "    if errored and rc == 0:\n        rc = 2\n",
     "    if errored and rc == 0:\n        rc = 1\n"),
]

S2K = "        if k and not begun[k]:\n            # The state's own numbers"
p.PLANTS += [
    ('again S2e only at a take-over, not at a start that created the device', J, S2K,
     "        if k and not begun[k] and bounds[k - 1]['taken_over']:\n            # The state's own numbers"),
    ('again S2f only at a start that created the device', J, S2K,
     "        if k and not begun[k] and not bounds[k - 1]['taken_over']:\n            # The state's own numbers"),
    ('again S2j the rule is never asked', J, S2K, "        if False:\n            # The state's own numbers"),
]

if __name__ == '__main__':
    p.main()
