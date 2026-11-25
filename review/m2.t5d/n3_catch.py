"""n3_catch.py WORKTREE: N3's catch in the tests. The stand-in log of
test_publish_check_main.py writes '-' in the G and C columns of every
`asked` line (as before the pull request); how many tests fail? The file
is put back in a finally. Run with python3 -I."""
import os
import subprocess
import sys

wt = sys.argv[1]
path = os.path.join(wt, 'tests/kernel/test_publish_check_main.py')
old = "                g, c = ln[3:5] if len(ln) == 5 else ('-', '-')\n"
new = "                g, c = ('-', '-')\n"
orig = open(path).read()
assert orig.count(old) == 1
try:
    open(path, 'w').write(orig.replace(old, new))
    p = subprocess.run(['python3', '-I', 'tests/kernel/test_publish_check_main.py'], cwd=wt,
                       capture_output=True, text=True, timeout=600)
    tail = [ln for ln in p.stderr.splitlines() if ln.startswith(('FAIL:', 'ERROR:', 'Ran ', 'FAILED', 'OK'))]
    print('exit', p.returncode)
    print('\n'.join(tail))
finally:
    open(path, 'w').write(orig)
print('clean' if not subprocess.run(['git', 'status', '--porcelain'], cwd=wt, capture_output=True, text=True).stdout else 'DIRTY')
