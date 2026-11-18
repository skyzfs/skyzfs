#!/usr/bin/env python3
"""#119 at 32d8d67: the head's judge on the recordings of the kernel runs
made so far (all on #118's e42ea6a, under older heads of the rig).

    rejudge.py CHECKOUT RUN_DIR [RUN_DIR ...]

Each recording's directory is copied (the judge appends to results.tsv)
and judged as cut-record.sh calls the check; only the lines that are not
INFO, and of the controls only those that differ, are printed.
"""
import os
import shutil
import subprocess
import sys
import tempfile

ROOT = sys.argv[1]
JUDGE = os.path.join(ROOT, 'tests', 'kernel', 'cut_daemon_check.py')
for run in sys.argv[2:]:
    print(f'== {os.path.basename(os.path.normpath(run))}')
    for name in sorted(os.listdir(run)):
        src = os.path.join(run, name)
        if not (os.path.isdir(src) and os.path.exists(os.path.join(src, 'requests.tsv'))
                and os.path.exists(os.path.join(src, 'status.txt'))):
            continue
        tmp = tempfile.mkdtemp(prefix='rejudge-')
        try:
            d = os.path.join(tmp, name)
            shutil.copytree(src, d)
            os.remove(os.path.join(d, 'results.tsv'))
            p = subprocess.run([sys.executable, '-I', JUDGE, name, d, os.path.join(d, 'requests.tsv'), '-',
                                str(512 << 20)], capture_output=True, text=True, timeout=1200)
            print(f'{name}: exit {p.returncode}')
            with open(os.path.join(d, 'results.tsv')) as f:
                for ln in f:
                    key, verdict, detail = ln.rstrip('\n').split('\t', 2)
                    if key.startswith('upload_') or verdict in ('FAIL', 'ERROR') or 'DIFFERS' in detail:
                        print(f'  {key}\t{verdict}\t{detail[:420]}')
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
