#!/usr/bin/env python3
"""Show-cases of the next look at #159 (head 1a40bfc), built with
test_agent_check's own fixtures. Usage: python3 -I showcases.py WORKTREE"""
import os, sys
sys.path.insert(0, os.path.abspath(os.path.join(sys.argv[1], 'tests', 'kernel')))
os.chdir(os.path.join(sys.argv[1], 'tests', 'kernel'))
import test_agent_check as tc
from test_agent_check import right, trace, upto, commit_of, run_course, not_ok

NONE = 'ref\tnone\nsnaprow\tnone\nmanifest\tnone\n'
KILL = 'skyzfs-agent: test hook --kill-at: killed at {} {} (1)\n'


def show(title, c, keys):
    rc, lines = run_course(course=c)
    print(f'== {title}: exit {rc}')
    for k in keys:
        print(f'   {k}: {lines.get(k, ("(none)", ""))[0]}  {lines.get(k, ("", ""))[1][:150]}')
    bad = {k: v[0] for k, v in not_ok(lines).items() if k not in keys}
    if bad:
        print(f'   other lines not ok: {bad}')


ASKED = upto('asked', 'after')


def destroy_case(retry_name, retry_commit, nodeb_has_t):
    """A destroy case: T k1 given up (destroyed) at the settle; the retry
    publishes RETRY_NAME; node B has k1 at the check or not."""
    c = right()
    c['cases'].append(('destroy', 'destroy', '-', '3', 'k1', {
        'trace.tsv': trace('k1', *ASKED), 'publish.out': 'abandoned k1 destroyed\n', 'destroy.log': '',
        'snaprow.txt': NONE, 'retry.out': f'snap/{retry_name} {retry_commit}\n', 'retry.rc': '0\n'}))
    c['pub'].append(('destroy', retry_name, 95, 509, retry_commit))
    c['rows'][retry_name] = (10, 96)
    if nodeb_has_t and retry_name != 'k1':
        c['pub'].append(('destroy', 'k1', 94, 510, '-'))
        c['rows']['k1'] = (11, 97)
    return c


K = ['pin_destroy_stays_absent', 'obs_destroy_name_reused', 'pin_destroy_outcome', 'pin_destroy_criterion']
show('SC1 G7 as planted at #158: the withdrawn k1 published by a later cut; the retry took k2', destroy_case('k2', commit_of('k2'), True), K)
show('SC1b the same, k1 not published (right)', destroy_case('k2', commit_of('k2'), False), K)
show('SC2 an agent that takes the given-up name k1 again; node B has the retry\'s k1', destroy_case('k1', commit_of('k1'), False), K)
show('SC2b the same; node B\'s k1 is not the retry\'s (an old request landed)', destroy_case('k1', 'e' * 64, False), K)


def moved_case(kill_e, kill_s, pts, settle, retry, rows, pub):
    c = right()
    c['cases'].append(('A-x', 'plain', f'{kill_e}:{kill_s}:1', '137', 'k1', {
        'trace.tsv': trace('k1', *pts), 'publish.err': KILL.format(kill_e, kill_s),
        'settle.out': settle, **({'snaprow.txt': NONE} if 'snap/k1 ' not in settle else {}),
        **({'retry.out': retry, 'retry.rc': '0\n'} if retry is not None else {})}))
    c['pub'] += pub
    c['rows'].update(rows)
    return c


TAKE = upto('intent', 'after')[:-1] + [('intent', 'after'), ('intent', 'before'), ('intent', 'after'),
                                        ('take', 'before'), ('take', 'after')]
K = ['pin_A-x_outcome', 'obs_A-x_outcome', 'pin_A-x_moved', 'obs_A-x_name_reused', 'pin_A-x_stays_absent',
     'pin_A-x_criterion']
show('SC3 kill at take:after; the settle publishes k1-1, the retry k1 in the same second (a2 at b99f21a): right',
     moved_case('take', 'after', TAKE, f'snap/k1-1 {commit_of("k1-1")}\n', f'snap/k1 {commit_of("k1")}\n',
                {'k1-1': (10, 92), 'k1': (11, 93)},
                [('A-x', 'k1-1', 91, 511, commit_of('k1-1')), ('A-x', 'k1', 92, 512, commit_of('k1'))]), K)
show('SC3b the same; node B\'s k1 is not the retry\'s',
     moved_case('take', 'after', TAKE, f'snap/k1-1 {commit_of("k1-1")}\n', f'snap/k1 {"e" * 64}\n',
                {'k1-1': (10, 92), 'k1': (11, 93)},
                [('A-x', 'k1-1', 91, 511, commit_of('k1-1')), ('A-x', 'k1', 92, 512, 'e' * 64)]), K)
show('SC4 kill at take:after; the settle publishes k1-1 and k1-2 (a double publish of one take)',
     moved_case('take', 'after', TAKE, f'snap/k1-1 {commit_of("k1-1")}\nsnap/k1-2 {commit_of("k1-2")}\n',
                f'snap/k2 {commit_of("k2")}\n',
                {'k1-1': (10, 92), 'k1-2': (11, 93), 'k2': (12, 94)},
                [('A-x', 'k1-1', 91, 511, commit_of('k1-1')), ('A-x', 'k1-2', 92, 512, commit_of('k1-2')),
                 ('A-x', 'k2', 93, 513, commit_of('k2'))]), K)
DESTROY = TAKE + [('failed', 'before'), ('failed', 'after'), ('destroy', 'before'), ('destroy', 'after')]
show('SC5 kill at destroy:after; the settle takes k1 again under its name and publishes it (the engine, '
     'a_crash_around_the_destroy_takes_again)',
     moved_case('destroy', 'after', DESTROY, f'snap/k1 {commit_of("k1")}\n', None,
                {'k1': (10, 92)}, [('A-x', 'k1', 91, 511, commit_of('k1'))]), K)
show('SC5b kill at destroy:before; the settle destroys the take and publishes k1-1: right',
     moved_case('destroy', 'before', DESTROY[:-1], f'snap/k1-1 {commit_of("k1-1")}\n', f'snap/k2 {commit_of("k2")}\n',
                {'k1-1': (10, 92), 'k2': (11, 93)},
                [('A-x', 'k1-1', 91, 511, commit_of('k1-1')), ('A-x', 'k2', 92, 512, commit_of('k2'))]), K)
