#!/usr/bin/env python3
"""skyzfs/skyzfs#11 at beb53a3: inputs with which tests/kernel/daemon_check.py is green
although nothing, or not everything, was judged. Run from tests/kernel of a checkout.
Each line prints the judge's exit status; 0 is green."""
import contextlib, io, os, tempfile
import daemon_check as d

def run(lines, checks):
    t = tempfile.mkdtemp()
    with open(os.path.join(t, 'results.tsv'), 'w') as f:
        f.write(''.join(f'{k}\t{v}\n' for k, v in lines))
    with contextlib.redirect_stdout(io.StringIO()):
        return d.main([t, '--checks', checks])

base = [('impl', 'reference: x'), ('setup', 'ok')]
e = [(n, (v[4] if len(v) > 4 else v[1]) + ': x') for n, v in d.EXPECTED.items() if v[0] == 'e'] + [('obs_e_kernel_log', 'x')]
print('no check ran, --checks "abcde" (the workflow accepts it):', run(base, 'abcde'))
print('no check ran, --checks " ":', run(base, ' '))
print('check e as the table has it:', run(base + e, 'e'))
print('... plus a pin the table does not know, FAIL:', run(base + e + [('pin_new_thing', 'FAIL: x')], 'e'))
print('... plus a control the table does not know, PASS:', run(base + e + [('ctl_x', 'PASS: x')], 'e'))
print('... one pin written FAIL and then PASS:', run(base + [('pin_held_request_completes', 'FAIL: x')] + e, 'e'))
