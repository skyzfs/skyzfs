#!/usr/bin/env python3
"""The Python inside topology-bench.sh compiles, and `summarize` runs.

    python3 tests/bench/test_embedded_python.py

The script runs its Python from here-documents (<<'PY' ... PY). Python
compiles a whole program before it runs a line, so one bad quote stops
the summary of every row, and nothing else runs that code before a
benchmark job ends.
"""
import json
import os
import re
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, 'topology-bench.sh')
HEREDOC = re.compile(r"<<\s*'PY'[^\n]*\n(.*?)\n\s*PY\n", re.S)


class Embedded(unittest.TestCase):
    def test_every_here_document_compiles(self):
        with open(SCRIPT) as f:
            text = f.read()
        docs = HEREDOC.findall(text)
        self.assertGreaterEqual(len(docs), 3)
        for k, doc in enumerate(docs):
            compile(doc, f'{SCRIPT} here-document {k + 1}', 'exec')

    def test_summarize_runs_on_an_empty_output(self):
        with tempfile.TemporaryDirectory() as d:
            p = subprocess.run(['bash', SCRIPT, 'summarize', d], capture_output=True, text=True, timeout=120)
        self.assertEqual(p.returncode, 0, p.stderr[-2000:])
        self.assertIn('## topology-bench', p.stdout)

    def row(self, d, looks):
        """Run the row here-document on looks {suffix: text} in d; its JSON."""
        with open(SCRIPT) as f:
            doc = next(x for x in HEREDOC.findall(f.read()) if 'sys.argv[12]' in x)
        prog = os.path.join(d, 'row.py')
        with open(prog, 'w') as f:
            f.write(doc)
        base = os.path.join(d, 'r')
        for suffix, text in looks.items():
            with open(base + suffix, 'w') as f:
                f.write(text)
        p = subprocess.run(['python3', '-I', prog, base, 'zfs-skyzfsd', 'tpcb', '60', 'none', '0',
                            base + '.nolog', base + '.nofsync', 'x', '8', '0', HERE],
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(p.returncode, 0, p.stderr[-2000:])
        return json.loads(p.stdout)

    def test_daemon_reclaim_ends_when_pgbench_returns(self):
        # The after-look comes after the trace's end, a sync and 2 s; the
        # daemon's cgroup is read for the window at the look right after
        # pgbench returned (.pressure), and from the after-look only when
        # that look has none.
        mb = 1 << 20
        def look(clock, xact, pgscan, at_max, file_mib):
            return (f'clock now {clock}\npg xact_commit {xact}\n'
                    f'mem skz.cgroup.stat.pgscan {pgscan * mb}\nmem skz.cgroup.events.max {at_max * mb}\n'
                    f'mem skz.cgroup.file {file_mib * mb}\nmem skz.VmRSS {30 * mb}\n')
        before = look(10**9, 0, 100, 5, 900)
        end = look(61 * 10**9, 10000, 1100, 50, 860)
        after = look(70 * 10**9, 10000, 1500, 70, 700)
        with tempfile.TemporaryDirectory() as d:
            diag = self.row(d, {'.before': before, '.pressure': end, '.after': after, '.pgbench': ''})['diag']
        m = diag['mem_mb']
        self.assertEqual(m['skz.cgroup.stat.pgscan.window'], 1000.0)
        self.assertEqual(m['skz.cgroup.events.max.window'], 45.0)
        self.assertEqual(m['skz.cgroup.file'], 860.0)
        self.assertEqual(diag['skz_cgroup_look'], 'pgbench returned')
        with tempfile.TemporaryDirectory() as d:
            diag = self.row(d, {'.before': before, '.pressure': 'clock now 61000000000\n', '.after': after,
                                '.pgbench': ''})['diag']
        m = diag['mem_mb']
        self.assertEqual(m['skz.cgroup.stat.pgscan.window'], 1400.0)
        self.assertEqual(m['skz.cgroup.file'], 700.0)
        self.assertEqual(diag['skz_cgroup_look'], 'after the sync')


if __name__ == '__main__':
    unittest.main()
