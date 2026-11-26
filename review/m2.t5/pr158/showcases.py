"""Show-cases against agent_check.py, built with test_agent_check's own
fixtures: each prints the exit code and the lines that matter."""
import sys
sys.path.insert(0, sys.argv[1])
import test_agent_check as t  # noqa: E402

def show(title, c, keys):
    rc, lines = t.run_course(course=c)
    print(f'--- {title}: exit {rc}')
    for k in keys:
        print('   ', k, lines.get(k))

# 1. A plain case killed at a later reach whose run panicked (101) before
#    it: INFO, judged as no kill; the settle published.
c = t.right()
c['cases'].append(('A-look-before-2', 'plain', 'look:before:2', '101', 'p1', {
    'trace.tsv': t.trace('p1', *t.upto('look', 'before')),
    'publish.err': "thread 'main' panicked at src/engine.rs:1: called unwrap on Err\n",
    'settle.out': f'snap/p1 {t.commit_of("p1")}\n'}))
c['pub'].append(('A-look-before-2', 'p1', 95, 509, '-'))
c['rows']['p1'] = (12, 96)
show('later reach not made, the agent panicked (101)', c,
     ['obs_A-look-before-2_not_reached', 'pin_A-look-before-2_outcome', 'pin_A-look-before-2_criterion'])

# 2. restart, no kill, the agent's own run ended 1 (error) and the settle
#    published: the scenario says "the agent goes on by itself".
c = t.right()
c['cases'].append(('d1', 'restart', '-', '1', 'a6', {
    'trace.tsv': t.trace('a6', *t.upto('asked', 'after')), 'between.txt': 'ref\tnone\nsnaprow\tnone\n',
    'publish.err': 'skyzfs-agent: the daemon: connection refused\n',
    'settle.out': f'snap/a6 {t.commit_of("a6")}\n'}))
c['pub'].append(('d1', 'a6', 90, 507, '-'))
c['rows']['a6'] = (11, 91)
show('restart: first run ended 1, the settle published', c,
     ['premise_d1_pending_at_the_loss', 'pin_d1_outcome', 'pin_d1_criterion'])

# 3. A record that does not read after the case (dump exits 6): clean.
c = t.with_case('trace', **{'record-after.txt': 'skyzfs-agent: /x/agent: the record is corrupt: bad checksum\n'})
show('record corrupt after the case', c, ['pin_trace_clean'])

# 4. both: nothing shows the daemon was killed (no premise).
c = t.right()
c['cases'].append(('b-ask-after-1', 'both', 'ask:after:1', '137', 'a6', {
    'trace.tsv': t.trace('a6', *t.upto('ask', 'after')),
    'publish.err': 'skyzfs-agent: test hook --kill-at: killed at ask after (1)\n',
    'settle.out': f'snap/a6 {t.commit_of("a6")}\n'}))
c['pub'].append(('b-ask-after-1', 'a6', 90, 507, '-'))
c['rows']['a6'] = (11, 91)
rc, lines = t.run_course(course=c)
print('--- both, no trace of a daemon kill: exit', rc, sorted(k for k in lines if 'b-ask-after-1' in k))

# 5. The retry printed a name node B does not have, T absent.
c = t.with_case('unmarked', **{'retry.out': f'snap/zz {t.commit_of("zz")}\n'})
c['pub'] = [p for p in c['pub'] if p[1] != 'a3']
del c['rows']['a3']
show('retry printed a name node B lacks', c, ['pin_unmarked_criterion'])
