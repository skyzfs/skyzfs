"""Show-cases against agent_check.py at bca1a27, built with
test_agent_check's own fixtures. Usage: python3 -I showcases159.py WORKTREE/tests/kernel"""
import sys
sys.path.insert(0, sys.argv[1])
import test_agent_check as t  # noqa: E402


def show(title, c, keys, **kw):
    rc, lines = t.run_course(course=c, **kw)
    print(f'--- {title}: exit {rc}')
    for k in keys:
        print('   ', k, lines.get(k))
    return rc, lines


# 1. Run 45772672638, a1, case A-intent-before-1: killed at intent:before:1
#    (no snapshot taken, T only named); node B has no snap/T after the
#    settle; the retry, in the same second, takes and publishes a snapshot
#    of the same name. A right run.
c = t.right()
c['cases'].append(('A-intent-before-1', 'plain', 'intent:before:1', '137', 'x7', {
    'trace.tsv': t.trace('x7', ('enrol', 'before'), ('enrol', 'after'), ('intent', 'before')),
    'publish.err': 'skyzfs-agent: test hook --kill-at: killed at intent before (1)\n',
    'settle.out': '', 'settle.rc': '0\n',
    'snaprow.txt': 'ref\tnone\nsnaprow\tnone\n',
    'retry.out': f'snap/x7 {t.commit_of("x7")}\n', 'retry.rc': '0\n'}))
c['pub'].append(('A-intent-before-1', 'x7', 95, 507, t.commit_of('x7')))
c['rows']['x7'] = (10, 96)
show('the retry published T\'s name (a right run)', c,
     ['obs_A-intent-before-1_outcome', 'pin_A-intent-before-1_criterion', 'pin_A-intent-before-1_stays_absent',
      'pin_x7_row', 'pin_x7_nodeb'])
