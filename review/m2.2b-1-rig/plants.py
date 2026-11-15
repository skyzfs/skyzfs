#!/usr/bin/env python3
"""#102 at 345f17e: plants in g.4's file judge (`gj_files_judged`, tests/kernel/daemon-rig.sh),
one at a time, against the judge's tests (tests/kernel/test_daemon_check.py). The file is
restored after each.

    plants.py CHECKOUT
"""
import os
import subprocess
import sys

ROOT = sys.argv[1]
RIG = 'tests/kernel/daemon-rig.sh'
ARM = ('            "$GJ_START_TOKEN")\n'
       '                [[ " $1 " == *" $f "* ]] || [[ $out == *"$f removed"* ]] || '
       'out+="the start token $f removed; " ;;\n')
PLANTS = [
    ('P1 the token has no rule of its own', ARM, ''),
    ('P2 the token may go as well', ARM, '            "$GJ_START_TOKEN") ;;\n'),
    ('P3 the token is allowed by a pattern, not by its name',
     ARM, ARM.replace('"$GJ_START_TOKEN")', 'volume.start*)')),
    ('P4 the two lists are taken the other way round',
     ARM, ARM.replace('[[ " $1 " ==', '[[ " $2 " ==')),
    ('P5 the token is allowed beside the status file, with no rule of its own',
     ARM, '', "GJ_MAY_CHANGE='^status$'", "GJ_MAY_CHANGE='^(status|volume.start)$'"),
    ('P6 a token that only appeared is named as removed',
     ARM, ARM.replace('[[ " $1 " == *" $f "* ]]', '[[ " $1 " == *" $f "* && " $2 " == *" $f "* ]]')),
    ('P7 the token\'s rule comes before the image\'s and takes every name',
     '            home.img|volume) [[', '            "$GJ_START_TOKEN"|*) ;;\n            home.img|volume) [['),
]


def main():
    path = os.path.join(ROOT, RIG)
    orig = open(path).read()
    try:
        for name, *pairs in PLANTS:
            text = orig
            counts = [text.count(old) for old in pairs[0::2]]
            if any(c != 1 for c in counts):
                print(f'{name}: NOT APPLIED (found {counts} times)', flush=True)
                continue
            for old, new in zip(pairs[0::2], pairs[1::2]):
                text = text.replace(old, new)
            with open(path, 'w') as f:
                f.write(text)
            p = subprocess.run([sys.executable, 'tests/kernel/test_daemon_check.py'], cwd=ROOT,
                               capture_output=True, text=True)
            failed = sorted({ln.split()[1] for ln in p.stderr.splitlines()
                             if ln.startswith(('FAIL: ', 'ERROR: '))})
            if p.returncode == 0:
                print(f'{name}: SURVIVED', flush=True)
            else:
                print(f'{name}: KILLED by {len(failed)}: {", ".join(failed)}', flush=True)
            with open(path, 'w') as f:
                f.write(orig)
    finally:
        with open(path, 'w') as f:
            f.write(orig)


if __name__ == '__main__':
    main()
