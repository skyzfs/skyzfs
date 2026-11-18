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

    def row(self, d, looks, env=None):
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
                           capture_output=True, text=True, timeout=60,
                           env={**os.environ, **(env or {})})
        self.assertEqual(p.returncode, 0, p.stderr[-2000:])
        return json.loads(p.stdout)

    def test_a_row_that_uploads(self):
        """TB_SKZ_UPLOAD (M2.4c): the row names the volume it uploads to,
        and the summary has a line for it in the uploader's table, with
        the counters over the window, the state at its end and how long
        the first upload took before the windows (its .skzinfo line); a
        row without uploads has neither."""
        devs = 'dev pool 0 0 0 0 0 0 0 0\ndev origin 0 0 0 0 0 0 0 0\n'
        before = (devs + 'skz cuts 10\nskz uploaded 3\nskz uploaded_whole 1\nskz upload_blocks 900\nskz cuts_replaced 6\n'
                  'skz cuts_abandoned 0\nskz upload_mismatches 0\nskz upload_errors 0\nskz cuts_held 1\n'
                  'skz upload_cuts_lost 2\nskz upload_zero_blocks 100\n')
        after = (devs + 'skz cuts 40\nskz uploaded 9\nskz uploaded_whole 1\nskz upload_blocks 1500\nskz cuts_replaced 30\n'
                 'skz cuts_abandoned 0\nskz upload_mismatches 0\nskz upload_errors 0\nskz cuts_held 2\n'
                 'skz upload_cut 39\nskz upload_state putting\nskz lease held\nskz upload_cuts_lost 5\n'
                 'skz upload_zero_blocks 8292\nskz cut_holes dropped\nskz cut_hole_chunks 0\n')
        with tempfile.TemporaryDirectory() as d:
            with open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w') as f:
                f.write('serve_args \nrelease_every 0\nupload http://127.0.0.1:9000 tb-1-1 tb-17\nbuild abc\n'
                        'first_upload after 40 s (uploaded 1, cuts lost 0)\n')
            r = self.row(d, {'.before': before, '.after': after, '.pgbench': 'tps = 812.5 (without initial connection time)\n'})
            self.assertEqual(r['skyzfsd']['upload'], {'endpoint': 'http://127.0.0.1:9000', 'bucket': 'tb-1-1', 'volume': 'tb-17'})
            self.assertTrue(r['skyzfsd']['serve_args'].endswith('(uploads to tb-17)'), r['skyzfsd']['serve_args'])
            out = os.path.join(d, 'out', '1')
            os.makedirs(out)
            with open(os.path.join(out, 'tpcb-zfs-skyzfsd.json'), 'w') as f:
                json.dump(r, f)
            p = subprocess.run(['bash', SCRIPT, 'summarize', os.path.join(d, 'out')], capture_output=True, text=True,
                               timeout=120)
            self.assertEqual(p.returncode, 0, p.stderr[-2000:])
            rows = [ln for ln in p.stdout.splitlines() if ln.startswith('| tpcb | ') and '(whole)' not in ln
                    and ' / 40 |' in ln]
            self.assertEqual(rows, ['| tpcb | zfs-skyzfsd [x] | 812 | 30 | 6 (0) | 600 | 24 | 0 | 0 | 0 | 3 | 8192 | dropped (0) | 2 | 39 / 40 | '
                                    'putting | held | after 40 s (uploaded 1, cuts lost 0) |'], p.stdout[-3000:])
            # a row without uploads
            with open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w') as f:
                f.write('serve_args \nrelease_every 0\nupload none\nbuild abc\n')
            r = self.row(d, {'.before': before, '.after': after, '.pgbench': ''})
            self.assertNotIn('upload', r['skyzfsd'])
            self.assertNotIn('uploads to', r['skyzfsd']['serve_args'])

    def test_the_intent_maps_table(self):
        """The intent map's syncs over the window (M2.3) and the header's
        own (status version 16, --intent-header-sync): a row from a
        daemon with the header's keys, one from an older daemon (n/a),
        and one whose daemon keeps no map (left out)."""
        devs = 'dev pool 0 0 0 0 0 0 0 0\ndev origin 0 0 0 0 0 0 0 0\n'
        old = 'skz intent_state taken_up\nskz intent_syncs {}\nskz intent_sync_ns {}\nskz intent_sync_max_ns 3000000\nskz intent_waits {}\n'
        new = old + ('skz intent_header_sync on\nskz intent_header_syncs {}\nskz intent_header_sync_ns {}\n'
                     'skz intent_header_sync_max_ns 1500000\nskz intent_named 12\nskz intent_regions 8192\n')
        cases = {'on': (devs + new.format(10, 10_000_000, 2, 4, 4_000_000), devs + new.format(110, 260_000_000, 7, 54, 54_000_000)),
                 'old': (devs + old.format(10, 10_000_000, 2), devs + old.format(30, 50_000_000, 2)),
                 'none': (devs + 'skz cuts 1\n', devs + 'skz cuts 2\n')}
        with tempfile.TemporaryDirectory() as d:
            with open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w') as f:
                f.write('serve_args --hold-cuts\nrelease_every 0\nbuild abc\n')
            out = os.path.join(d, 'out', '1')
            os.makedirs(out)
            for i, (name, (b, a)) in enumerate(cases.items()):
                r = self.row(d, {'.before': b, '.after': a, '.pgbench': f'tps = {700 + i} (without initial connection time)\n'})
                r['workload'] = name
                with open(os.path.join(out, f'{name}-zfs-skyzfsd.json'), 'w') as f:
                    json.dump(r, f)
            p = subprocess.run(['bash', SCRIPT, 'summarize', os.path.join(d, 'out')], capture_output=True, text=True,
                               timeout=120)
            self.assertEqual(p.returncode, 0, p.stderr[-2000:])
            text = p.stdout.split('The intent map in the window', 1)
            self.assertEqual(len(text), 2, p.stdout[-3000:])
            table = text[1].split('\n\n')[1].splitlines()
            rows = [ln for ln in table if 'zfs-skyzfsd' in ln]
            self.assertEqual(sorted(rows), sorted([
                '| on | zfs-skyzfsd [x] | 700 | on | 100 | 2.50 | 3.00 | 50 | 1.00 | 1.50 | 5 | 12 / 8192 |',
                '| old | zfs-skyzfsd [x] | 701 | n/a | 20 | 2.00 | 3.00 | n/a | n/a | n/a | 0 | n/a / n/a |']), text[1][:3000])

    def test_a_row_says_what_its_releaser_did(self):
        """TB_SKZ_RELEASE (M2.T4): the row's serve arguments give the
        releases in the window, or that the releaser ended, from its
        line in the two readings; a row without one has neither."""
        def rel(d, before, after, every='1'):
            with open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w') as f:
                f.write(f'serve_args --hold-cuts\nrelease_every {every}\nbuild abc+hooks\n')
            looks = {'.before': f'skz writes 1\n{before}', '.after': f'skz writes 2\n{after}', '.pgbench': ''}
            return self.row(d, looks)['skyzfsd']
        with tempfile.TemporaryDirectory() as d:
            k = rel(d, 'skzrel 36 1 1\t36\t37\n', 'skzrel 59 1 1\t59\t59\n')
            self.assertEqual(k['release']['in_window'], 23)
            self.assertNotIn('ended', k['release'])
            self.assertEqual(k['serve_args'], '--hold-cuts (cuts released every 1: 23 in the window)')
            # ended in the window: its last line says why
            k = rel(d, 'skzrel 36 1 1\t36\t37\n', 'skzrel 40 0 # ERROR HolderError: release 41 refused\n')
            self.assertEqual(k['release']['ended'], '# ERROR HolderError: release 41 refused')
            self.assertIn('THE RELEASER ENDED, # ERROR HolderError: release 41 refused', k['serve_args'])
            # ended before it: no releases at all
            k = rel(d, 'skzrel 0 0 # ERROR HolderError: no reply\n', 'skzrel 0 0 # ERROR HolderError: no reply\n')
            self.assertEqual(k['release']['in_window'], 0)
            self.assertIn('THE RELEASER ENDED', k['serve_args'])
            # no line from it at all
            k = rel(d, '', '')
            self.assertEqual(k['release']['ended'], 'no line from the releaser')
            # no releaser
            k = rel(d, '', '', every='0')
            self.assertNotIn('release', k)
            self.assertEqual(k['serve_args'], '--hold-cuts')

    def test_a_row_after_a_failed_row_says_so(self):
        # The workflow sets TB_EARLIER_FAILED on the rows of a sequence
        # after one that failed (M1.T24).
        with tempfile.TemporaryDirectory() as d:
            r = self.row(d, {'.before': 'clock now 1\n', '.after': 'clock now 2\n', '.pgbench': ''},
                         {'TB_EARLIER_FAILED': '2 5'})
        self.assertEqual(r['earlier_failed'], [2, 5])
        with tempfile.TemporaryDirectory() as d:
            r = self.row(d, {'.before': 'clock now 1\n', '.after': 'clock now 2\n', '.pgbench': ''},
                         {'TB_EARLIER_FAILED': ''})
        self.assertNotIn('earlier_failed', r)

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
