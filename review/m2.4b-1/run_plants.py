#!/usr/bin/env python3
"""Run plants of plants.py one at a time, a file of result per plant in plants/, so
that a run cut short loses one plant at most. A plant that has its file is skipped.

    run_plants.py CHECKOUT TARGET_DIR ID [ID ...]

An ID of the form S:ID runs the scenarios (scenarios.py) against that plant instead,
into gaps_ID.out; S: alone runs them on the head, into scenarios.out.
"""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT, TARGET = sys.argv[1], sys.argv[2]


def clean():
    status = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True)
    return status.stdout.strip() == ''


for ident in sys.argv[3:]:
    if ident.startswith('S:'):
        plant = ident[2:]
        out = os.path.join(HERE, f'gaps_{plant}.out' if plant else 'scenarios.out')
        cmd = [sys.executable, os.path.join(HERE, 'scenarios.py'), ROOT, TARGET] + ([plant + ' '] if plant else [])
    else:
        out = os.path.join(HERE, 'plants', ident + '.out')
        cmd = [sys.executable, os.path.join(HERE, 'plants.py'), ROOT, TARGET, ident + ' ']
    if os.path.exists(out):
        print(f'{ident}: has its result', flush=True)
        continue
    if not clean():
        print(f'{ident}: the checkout is not clean, stopping', flush=True)
        break
    p = subprocess.run(cmd, capture_output=True, text=True)
    text = p.stdout + p.stderr
    with open(out + '.part', 'w') as f:
        f.write(text)
    os.rename(out + '.part', out)
    print(f'{ident}: ' + (text.splitlines() or ['(nothing)'])[0][:200], flush=True)
