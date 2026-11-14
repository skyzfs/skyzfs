"""Plants for #80 at 0486339: each applied alone, then both no-kernel tests run."""
import subprocess
W = '/home/user/skyzfs-pr74'
RIG = 'tests/kernel/control-rig.sh'
COND = ('    if [[ $sent == "$want"* ]] && [[ $said == refused:* ]] && [ "$ended" = yes ] \\\n'
        '        && [[ $served == "ended by the refused commit"* ]] && [ "$rd" = waiting ] && [ "$state" = waiting ]; then')
def cond(drop):
    return COND.replace(drop, 'true', 1)
WR = '            [ "$dev" = "$RC_NEW" ] && [ "$img" = "$RC_NEW" ] || r=FAIL'
PL = [
 ('R2 the driver\'s answer is not checked', RIG, COND, cond('[[ $said == refused:* ]]')),
 ('R4 the program\'s last line is not checked', RIG, COND, cond('[[ $served == "ended by the refused commit"* ]]')),
 ('R1b the sent line is not checked (any kind passes)', RIG, COND, cond('[[ $sent == "$want"* ]]')),
 ('W1 the write is not read back through the device', RIG, WR, '            [ "$img" = "$RC_NEW" ] || r=FAIL'),
 ('W2 the write is not read back from the image', RIG, WR, '            [ "$dev" = "$RC_NEW" ] || r=FAIL'),
 ('W3 the write is not read back at all', RIG, WR, '            true'),
 ('W4 a writer that failed passes', RIG, '    else\n        [[ $(cat "$RC_RD.rc") == "exit 0 "* ]]\n    fi', '    else\n        true\n    fi'),
 ('W5 the flush client does not fsync', RIG, '"conv=fsync,nocreat,notrunc"', '"conv=nocreat,notrunc"'),
 ('W6 the write course refuses a read', RIG, 'what=$kind; [ "$kind" = flush ] || what="$kind:$RC_AT"', 'what=$kind; [ "$kind" = flush ] || what="read:$RC_AT"'),
 ('W7 the flush pin is the read pin', RIG, '        *) pin=refused_flush_completes_after_take_over ;;', '        *) pin=refused_read_issued_again ;;'),
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
    f2 = [l[:100] for l in o2.splitlines() if l.startswith('FAIL')]
    print(f'{name}: table rc {rc1} {f1}; refused rc {rc2} {f2}', flush=True)
print(run(['git', 'status', '--short'])[1] or 'clean')
