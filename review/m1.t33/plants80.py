"""Plants for #80 at 0264071: each applied alone, then both no-kernel tests run."""
import subprocess
W = '/home/user/skyzfs-pr74'
RIG = 'tests/kernel/control-rig.sh'
CC = 'tests/kernel/control_check.py'
COND = ('    if [[ $sent == "sent for a read at byte $RC_AT on tag "* ]] && [[ $said == refused:* ]] && [ "$ended" = yes ] \\\n'
        '        && [[ $served == "ended by the refused commit"* ]] && [ "$rd" = waiting ] && [ "$state" = waiting ]; then')
def cond(drop):
    return COND.replace(drop, 'true', 1)
PL = [
 ('R1 the byte of the sent line is not checked', RIG, COND, cond('[[ $sent == "sent for a read at byte $RC_AT on tag "* ]]')),
 ('R2 the driver\'s answer is not checked', RIG, COND, cond('[[ $said == refused:* ]]')),
 ('R3 the program\'s own end is not checked', RIG, COND, cond('[ "$ended" = yes ]')),
 ('R4 the served line is not checked', RIG, COND, cond('[[ $served == "ended by the refused commit"* ]]')),
 ('R5 the reader is not checked', RIG, COND, cond('[ "$rd" = waiting ]')),
 ('R6 the device\'s state is not checked', RIG, COND, cond('[ "$state" = waiting ]')),
 ('R7 rc_reader ignores the data', RIG, '    [[ $(cat "$RC_RD.rc") == "exit 0 "* ]] && [ "$got" = "$RC_WANT" ]', '    [[ $(cat "$RC_RD.rc") == "exit 0 "* ]]'),
 ('R8 rc_reader ignores the exit', RIG, '    [[ $(cat "$RC_RD.rc") == "exit 0 "* ]] && [ "$got" = "$RC_WANT" ]', '    [ "$got" = "$RC_WANT" ]'),
 ('R9 the table lacks the abort pin', CC, "    'pin_commands_aborted_at_a_stop': (", "    'pin_commands_aborted_at_a_stopX': ("),
 ('R10 the add\'s answer is not checked', RIG, '    if [ "$ANSWER" != added ] && [[ $ANSWER != added\\ * ]]; then RC_WHY="the add under $n answered: ${ANSWER:-nothing}"; return; fi\n', ''),
]
def run(cmd):
    r = subprocess.run(cmd, cwd=W, capture_output=True, text=True)
    return r.returncode, r.stdout + r.stderr
for name, f, old, new in PL:
    path = f'{W}/{f}'
    s = open(path).read()
    n = s.count(old)
    if n != 1:
        print(f'{name}: PLANT NOT APPLIED ({n})', flush=True); continue
    open(path, 'w').write(s.replace(old, new))
    try:
        rc1, o1 = run(['python3', '-I', 'tests/kernel/test_control_check.py'])
        rc2, o2 = run(['bash', 'tests/kernel/test_control_refused.sh'])
    finally:
        open(path, 'w').write(s)
    f1 = sorted({l.split()[1] for l in o1.splitlines() if l.startswith(('FAIL:', 'ERROR:'))})
    f2 = [l[:90] for l in o2.splitlines() if l.startswith('FAIL')]
    print(f'{name}: table rc {rc1} {f1}; refused rc {rc2} {f2}', flush=True)
print(run(['git', 'status', '--short'])[1] or 'clean')
