#!/usr/bin/env python3
"""#115 at afd217a: the scenarios of the review of #113 on the recording
`failed` (../m2.t3-2c/scenarios.py, S0 to S9: the first cut after the
failure lacking what the intent map named, the counting rules, the state
taken up at an acknowledged start), run through this head's judge, with
the new lines shown as well.

    python3 -I scenarios_failed.py CHECKOUT ARTIFACT_DIR...
"""
import os
import sys

here = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(here, '..', 'm2.t3-2c', 'scenarios.py')) as f:
    src = f.read()
old = "'failure', 'rig_error')"
assert src.count(old) == 1
src = src.replace(old, "'failure', 'carried_across_failure', 'obs_unstored_at_kill', 'rig_error')")
old = "(lines[k][0] not in ('PASS', 'INFO') or k in ('cut_numbers', 'status'))"
assert src.count(old) == 1
src = src.replace(old, "(lines[k][0] not in ('PASS', 'INFO') or k in ('cut_numbers', 'status', "
                  "'carried_across_failure'))")
exec(compile(src, 'scenarios.py', 'exec'))
