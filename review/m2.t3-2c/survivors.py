#!/usr/bin/env python3
"""#113 at 98024d5: the plants that the unit tests let through
(plants.out), each planted again and held against what the rig itself
has: the judge run on the recordings `failed` and `takeover` of a run's
artifact (would the run have noticed?) and on the hand-made logs of
scenarios.py (does a daemon's defect, or the other counting rule, come
out differently?). The file is restored after each; each result is
written to OUT as it comes.

    python3 -I survivors.py HERE CHECKOUT OUT ARTIFACT_DIR PLANT...

HERE is the directory of plants.py and scenarios.py; ARTIFACT_DIR is one
artifact (one image); PLANT is a plant's first word (E4, N4, ...).
"""
import os
import shutil
import subprocess
import sys
import tempfile

here, root, out_path, art = sys.argv[1:5]
names = sys.argv[5:]
sys.argv = ['plants.py', root, os.devnull]
sys.path.insert(0, here)
import plants  # noqa: E402

JUDGE = os.path.join(root, 'tests/kernel/cut_daemon_check.py')


def recorded():
    """-> {recording: (exit, lines)} of the judge on the artifact's recordings."""
    got = {}
    for name in ('failed', 'takeover'):
        tmp = tempfile.mkdtemp(prefix='rv-surv-')
        try:
            d = os.path.join(tmp, name)
            shutil.copytree(os.path.join(art, name), d)
            os.remove(os.path.join(d, 'results.tsv'))
            p = subprocess.run([sys.executable, '-I', '-B', JUDGE, name, d, os.path.join(d, 'requests.tsv'), '-',
                                str(512 << 20)], capture_output=True, text=True, timeout=1500)
            lines = ''
            if os.path.exists(os.path.join(d, 'results.tsv')):
                with open(os.path.join(d, 'results.tsv')) as f:
                    lines = ''.join(ln for ln in f if not ln.startswith('time\t'))
            got[name] = (p.returncode, lines)
        finally:
            shutil.rmtree(tmp)
    return got


def scenarios():
    """-> {scenario: exit} of scenarios.py on the artifact."""
    p = subprocess.run([sys.executable, '-I', '-B', os.path.join(here, 'scenarios.py'), root, art],
                       capture_output=True, text=True, timeout=3000)
    got = {}
    for ln in p.stdout.splitlines():
        if ln.startswith('  S') and ': exit ' in ln:
            got[ln.split()[0]] = ln.rsplit(': exit ', 1)[1]
    if p.returncode != 0:
        got['scenarios.py'] = f'ended with {p.returncode}: {p.stderr.strip().splitlines()[-1:]}'
    return got


def main():
    with open(out_path, 'a') as out:
        def say(text):
            print(text, flush=True)
            out.write(text + '\n')
            out.flush()
            os.fsync(out.fileno())
        base_rec, base_scen = recorded(), scenarios()
        say(f'{os.path.basename(art)}, the head as it is: failed exit {base_rec["failed"][0]}, takeover exit '
            f'{base_rec["takeover"][0]}; scenarios {" ".join(f"{k}={v}" for k, v in base_scen.items())}')
        for name, path, *pairs in plants.PLANTS:
            if name.split()[0] not in names:
                continue
            full = os.path.join(root, path)
            with open(full) as f:
                orig = f.read()
            text = orig
            for old, new in zip(pairs[0::2], pairs[1::2]):
                if text.count(old) != 1:
                    say(f'{name}: NOT APPLIED')
                    text = None
                    break
                text = text.replace(old, new)
            if text is None:
                continue
            try:
                with open(full, 'w') as f:
                    f.write(text)
                rec, scen = recorded(), scenarios()
            finally:
                with open(full, 'w') as f:
                    f.write(orig)
            said = []
            for r in ('failed', 'takeover'):
                if rec[r] == base_rec[r]:
                    continue
                a, b = base_rec[r][1].splitlines(), rec[r][1].splitlines()
                diff = [y for x, y in zip(a, b) if x != y] + b[len(a):]
                said.append(f'{r} as recorded: exit {rec[r][0]} (the head: {base_rec[r][0]}), '
                            f'{len(diff)} lines differ, the first: {(diff[0] if diff else "none")[:200]}')
            for k in sorted(set(base_scen) | set(scen)):
                if scen.get(k) != base_scen.get(k):
                    said.append(f'{k}: exit {scen.get(k)} (the head: {base_scen.get(k)})')
            say(f'{name}: ' + ('; '.join(said) if said else 'NOTHING differs: the recordings as recorded and '
                               'every scenario give the head\'s lines and exits'))


if __name__ == '__main__':
    main()
