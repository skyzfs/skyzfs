"""Show-cases against agent_check.py at c580c72, built with
test_agent_check's own fixtures: each prints the exit code and the lines
that matter. Usage: python3 -I showcases2.py WORKTREE/tests/kernel"""
import sys
sys.path.insert(0, sys.argv[1])
import test_agent_check as t  # noqa: E402


def show(title, c, keys, **kw):
    rc, lines = t.run_course(course=c, **kw)
    print(f'--- {title}: exit {rc}')
    for k in keys:
        print('   ', k, lines.get(k))
    return rc, lines


# 1. D1: a destroy case whose T node B has and node A does not (the
#    recorder's published-gone.tsv), with the agent's own run saying it
#    abandoned T.
c = t.right()
c['cases'].append(('destroy', 'destroy', '-', '3', 'd1', {
    'trace.tsv': t.trace('d1', *t.upto('asked', 'after')), 'destroy.log': '',
    'publish.out': 'abandoned d1 destroyed\n'}))
c['gone'] = 'destroy\td1\n'
show('D1: destroy, T published and gone', c,
     ['pin_published_snapshots_exist', 'pin_destroy_first_run', 'pin_destroy_outcome', 'pin_destroy_criterion'])

# 2. The same, and a premise of another case that does not hold (G3).
show('D1 with a premise ERROR elsewhere', c,
     ['pin_published_snapshots_exist', 'premise_A-asked-after-1_killed'],
     change=t.case_file('A-asked-after-1', 'publish.err', ''))

# 3. The settle's status is judged nowhere: a plain case killed at
#    asked:after:1, the settle panicked (101) and printed nothing, node B
#    has no a2, the retry published a9.
c = t.right()
c['cases'][1] = ('A-asked-after-1', 'plain', 'asked:after:1', '137', 'a2', {
    'trace.tsv': t.trace('a2', *t.upto('asked', 'after')),
    'publish.err': 'skyzfs-agent: test hook --kill-at: killed at asked after (1)\n',
    'settle.out': '', 'settle.rc': '101\n',
    'settle.err': "thread 'main' panicked at crates/skyzfs-agent/src/engine.rs:1:1\n",
    'retry.out': f'snap/a9 {t.commit_of("a9")}\n', 'retry.rc': '0\n'})
c['pub'] = [p for p in c['pub'] if p[1] != 'a2'] + [('A-asked-after-1', 'a9', 62, 509, t.commit_of('a9'))]
c['rows'] = {k: v for k, v in c['rows'].items() if k != 'a2'}
c['rows']['a9'] = (6, 63)
show('settle panicked (101), the retry published', c,
     ['obs_A-asked-after-1_outcome', 'pin_A-asked-after-1_criterion', 'pin_A-asked-after-1_clean',
      'case_A-asked-after-1'])

# 4. The same with the settle unsettled at its limit (7).
c['cases'][1][5]['settle.rc'] = '7\n'
c['cases'][1][5]['settle.out'] = 'unsettled a2\n'
show('settle unsettled (7), the retry published', c, ['pin_A-asked-after-1_criterion', 'case_A-asked-after-1'])

# 5. G3, a missed control with a premise ERROR: ends 1 (no test has it).
show('missed ctl_files and a premise ERROR', t.right(), ['ctl_files', 'premise_A-asked-after-1_killed'],
     miss_files=True, change=t.case_file('A-asked-after-1', 'publish.err', ''))

# 6. G3, a per-case error of the rig hides every later case: the second
#    case has no holds.txt (RigError), the gone case left a hold of the
#    tag and a destroyed snapshot published by it.
c = t.right()
c['cases'][3] = c['cases'][3][:5] + (dict(c['cases'][3][5], **{
    'holds.txt': f'{t.POOL}@g1\tskyzfs-publish\tThu\n'}),)
rc, lines = show('a later case hidden by an earlier case\'s RigError', c,
                 ['rig_error', 'pin_gone_clean', 'pin_gone_outcome'],
                 change=t.remove('cases/A-asked-after-1/holds.txt'))
print('    lines judged:', sorted(k for k in lines if k.startswith('pin_')))
rc, lines = show('the same, no RigError', c, ['pin_gone_clean'])
