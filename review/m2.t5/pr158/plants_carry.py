import subprocess, sys, shutil
wt = sys.argv[1]
f = wt + '/tests/kernel/agent_check.py'
orig = open(f).read()
plants = [
 ('R01 number >= C', "if row['number'] <= c:", "if row['number'] < c:"),
 ('R02 group > G', "if row['group'] < g:", "if row['group'] <= g:"),
 ('R03 no judge_cut', "return bad + publish_check.judge_cut(cuts, row)", "return bad"),
 ('R04 first ask not last', "                last = (ln['ns'], int(ln['sector']), int(ln['sectors']))", "                last = last or (ln['ns'], int(ln['sector']), int(ln['sectors']))"),
 ('R05 G/C swapped', "last = (ln['ns'], int(ln['sector']), int(ln['sectors']))", "last = (ln['ns'], int(ln['sectors']), int(ln['sector']))"),
 ('R06 any result', "and ln['result'] == f'{name} accepted'", "and ln['result'].startswith(name)"),
 ('R07 older <=', "older = (row['number'], row['group']) < kcut", "older = (row['number'], row['group']) < (0, 0)"),
 ('R08 swap pin on put too', "if hook == 'carry-swap:1':", "if hook in ('carry-swap:1', 'carry-put:1'):"),
 ('R09 no row passes', "holds(f'pin_{cid}_carried_by_the_last_request', [f'no row of {t} checked at node B'], 'server', '')", "holds(f'pin_{cid}_carried_by_the_last_request', [], 'server', '')"),
 ('R10 premise always ok', "[] if ask else [f'no ask of {t} accepted in the request log']", "[]"),
 ('R11 first_carrier itself ignored', "if n > c and gr >= g and itself)", "if n > c and gr >= g)"),
]
for name, a, b in plants:
    n = orig.count(a)
    if n != 1:
        print(name, 'NOT APPLIED', n); continue
    open(f, 'w').write(orig.replace(a, b))
    try:
        r = subprocess.run(['python3', '-I', 'tests/kernel/test_agent_check.py'], cwd=wt, capture_output=True, timeout=600)
        print(name, 'KILLED' if r.returncode else 'SURVIVED')
    finally:
        open(f, 'w').write(orig)
