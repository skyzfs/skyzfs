#!/usr/bin/env python3
"""Plants on 9a87140's raw() (the review of #139). Each plant edits a copy
of tests/kernel (never the worktree), runs the pull request's
test_publish_agent.Raw there, and says KILLED (a test failed), SURVIVED,
or NOT APPLIED (the text is not there exactly once).
Usage: fix_plants.py KERNEL_DIR SCRATCH_DIR
"""
import os
import shutil
import subprocess
import sys

PLANTS = [
    ('X1', 'a write that fails with EPIPE is the rig\'s error again (only ECONNRESET kept)',
     'except (BrokenPipeError, ConnectionResetError) as e:\n            err = e',
     'except ConnectionResetError as e:\n            err = e'),
    ('X2', 'a reset with no answer read is not raised (returns an empty answer)',
     '        if not got and err is not None:\n            raise err\n', ''),
    ('X3', 'a reset on the read forgets its error (breaks only)',
     '                err = err or e\n                break', '                break'),
    ('X4', 'the fix reverted on the read (a reset after the answer raises)',
     '            except ConnectionResetError as e:\n                err = err or e\n                break',
     '            except ConnectionResetError as e:\n                raise'),
    ('X5', 'any OSError on the write is kept, not only EPIPE and ECONNRESET',
     'except (BrokenPipeError, ConnectionResetError) as e:\n            err = e',
     'except OSError as e:\n            err = e'),
]


def main():
    kdir, scratch = sys.argv[1], sys.argv[2]
    src = open(os.path.join(kdir, 'publish_agent.py')).read()
    for pid, what, old, new in PLANTS:
        n = src.count(old)
        if n != 1:
            print(f'{pid}: NOT APPLIED ({n} matches): {what}')
            continue
        d = os.path.join(scratch, f'plant-{pid}')
        shutil.rmtree(d, ignore_errors=True)
        shutil.copytree(kdir, d, ignore=shutil.ignore_patterns('__pycache__'))
        with open(os.path.join(d, 'publish_agent.py'), 'w') as f:
            f.write(src.replace(old, new))
        env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
        p = subprocess.run([sys.executable, '-m', 'unittest', 'test_publish_agent.Raw'], cwd=d, env=env,
                           capture_output=True, text=True, timeout=300)
        failed = [ln.split(' ', 1)[1].split(' (')[0] for ln in p.stderr.splitlines()
                  if ln.startswith(('FAIL:', 'ERROR:'))]
        print(f'{pid}: {"KILLED" if p.returncode else "SURVIVED"} {failed}: {what}')
        shutil.rmtree(d, ignore_errors=True)


if __name__ == '__main__':
    main()
