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
import sys
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
        and the summary has a line for it in the uploader's table: every
        counter's change over the window (each different at the two ends,
        so that a figure read at one end shows), the cuts held at both
        ends, the state at the end and how long the first upload took
        before the windows (its .skzinfo line); a window whose daemon
        changed says so; a row without uploads has neither."""
        devs = 'dev pool 0 0 0 0 0 0 0 0\ndev origin 0 0 0 0 0 0 0 0\n'
        before = (devs + 'skz pid 7\nskz cuts 10\nskz uploaded 3\nskz uploaded_whole 1\nskz upload_blocks 900\n'
                  'skz cuts_replaced 6\nskz cuts_abandoned 1\nskz upload_abandoned_at_mismatch 1\n'
                  'skz upload_mismatches 1\nskz upload_errors 1\nskz cuts_held 1\nskz upload_cuts_lost 2\n'
                  'skz upload_zero_blocks 100\nskz upload_cut 9\nskz upload_state idle\nskz lease waiting\n'
                  'skz cut_holes held\nskz cut_hole_chunks 7\n')
        after = (devs + 'skz pid 7\nskz cuts 40\nskz uploaded 9\nskz uploaded_whole 1\nskz upload_blocks 1500\n'
                 'skz cuts_replaced 30\nskz cuts_abandoned 4\nskz upload_abandoned_at_mismatch 2\n'
                 'skz upload_mismatches 2\nskz upload_errors 3\nskz cuts_held 2\nskz upload_cuts_lost 5\n'
                 'skz upload_zero_blocks 8292\nskz upload_cut 39\nskz upload_state putting\nskz lease held\n'
                 'skz cut_holes dropped\nskz cut_hole_chunks 0\n')
        with tempfile.TemporaryDirectory() as d:
            with open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w') as f:
                f.write('serve_args \nrelease_every 0\nupload http://127.0.0.1:9000 tb-1-1 tb-17\nbuild abc\n'
                        'first_upload not committed by the window\'s end (watched from the import)\n'
                        'first_upload after 40 s (uploaded 1, cuts lost 0)\n')
            r = self.row(d, {'.before': before, '.after': after, '.pgbench': 'tps = 812.5 (without initial connection time)\n'})
            # the last line of a key is the one kept: the watcher's commit after its first word
            self.assertEqual(r['skyzfsd']['info']['first_upload'], 'after 40 s (uploaded 1, cuts lost 0)')
            self.assertEqual(r['skyzfsd']['upload'], {'endpoint': 'http://127.0.0.1:9000', 'bucket': 'tb-1-1', 'volume': 'tb-17'})
            self.assertTrue(r['skyzfsd']['serve_args'].endswith('(uploads to tb-17)'), r['skyzfsd']['serve_args'])
            self.assertNotIn('from_load', r['skyzfsd']['upload'])
            two = self.row(d, {'.before': before, '.after': after.replace('skz pid 7', 'skz pid 8'), '.pgbench': ''})
            self.assertIs(two['skyzfsd']['pid_same'], False)
            two['workload'] = 'select'
            # review of #128, L1: a status with no pid at one end is not a
            # second daemon, though pid_same is False; one that failed to
            # be read leaves its clock line alone (review of #132, D2)
            unread = self.row(d, {'.before': before, '.after': devs + 'skz read_at_ns 2000\n', '.pgbench': ''})
            self.assertIs(unread['skyzfsd']['pid_same'], False)
            unread['workload'] = 'update'
            # Review of #132, D2: a status read at the end that shows the
            # daemon gone is no serving daemon, not a status not read.
            gone = self.row(d, {'.before': before, '.after': devs + 'skz read_at_ns 2000\n'
                                'skz counters left_by_a_daemon_that_is_gone,_pid_7\n', '.pgbench': ''})
            gone['workload'] = 'gone'
            # The lead's decision on #132: a row with no upload at its
            # window's start is a seed row, counted apart from the median.
            seed = self.row(d, {'.before': before.replace('skz uploaded 3\n', 'skz uploaded 0\n'), '.after': after,
                                '.pgbench': ''})
            seed['workload'] = 'seed'
            out = os.path.join(d, 'out', '1')
            os.makedirs(out)
            for name, row in (('tpcb', r), ('select', two), ('update', unread), ('gone', gone), ('seed', seed)):
                with open(os.path.join(out, f'{name}-zfs-skyzfsd.json'), 'w') as f:
                    json.dump(row, f)
            p = subprocess.run(['bash', SCRIPT, 'summarize', os.path.join(d, 'out')], capture_output=True, text=True,
                               timeout=120)
            self.assertEqual(p.returncode, 0, p.stderr[-2000:])
            table = p.stdout.split('What the uploader did in the window', 1)[1].split('\n\n')[1].splitlines()
            self.assertEqual([ln for ln in table if ln.startswith('| tpcb | ')],
                             ['| tpcb | zfs-skyzfsd [x] | 812 | 30 | 6 (0) | 600 | 24 | 3 (1) | 1 | 2 | 3 | 8192 | '
                              'dropped (0) | 1 → 2 | 39 / 40 | putting | held | after 40 s (uploaded 1, cuts lost 0) |'],
                             p.stdout[-3000:])
            self.assertEqual([ln.split(' | ')[1] for ln in table if ln.startswith('| select | ')],
                             ['zfs-skyzfsd [x] (TWO DAEMONS in the window)'])
            self.assertEqual([ln.split(' | ')[1] for ln in table if ln.startswith('| update | ')],
                             ['zfs-skyzfsd [x] (its status not read at an end of the window)'])
            self.assertEqual([ln.split(' | ')[1] for ln in table if ln.startswith('| gone | ')],
                             ['zfs-skyzfsd [x] (NO SERVING DAEMON at an end of the window)'])
            status = p.stdout.split('one server throughout', 1)[1].split('\n\n')[0].splitlines()
            self.assertEqual(sorted(ln.split(' | ')[13] for ln in status if 'zfs-skyzfsd' in ln),
                             ['NO: the pid changed', 'no serving daemon at an end', 'status not read', 'yes', 'yes'],
                             p.stdout[-3000:])
            self.assertIn("\nRows for design §9's median (the steady state): 0 with a verdict (0 at least, 0 below), "
                          "4 with none.\n", p.stdout)
            self.assertIn("Seed rows, not in the median (0 with a verdict (0 at least, 0 below), 1 with none): seed "
                          "zfs-skyzfsd [x] (row 1): no upload had committed at its window's start.", p.stdout)
            self.assertIn('is not one of design §9\'s median', p.stdout)
            # a row whose object storage was frozen until the workload began
            with open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w') as f:
                f.write('serve_args \nrelease_every 0\nupload http://127.0.0.1:9000 tb-1-1 tb-18\nbuild abc\n'
                        'upload_from_load 1\n')
            r = self.row(d, {'.before': before, '.after': after, '.pgbench': ''})
            self.assertEqual(r['skyzfsd']['upload'].get('from_load'), 'workload')
            self.assertTrue(r['skyzfsd']['serve_args'].endswith(
                "(uploads to tb-18) (object storage frozen from the daemon's start to the workload's)"),
                r['skyzfsd']['serve_args'])
            # =window: frozen until the first window began, the seed in it
            with open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w') as f:
                f.write('serve_args \nrelease_every 0\nupload http://127.0.0.1:9000 tb-1-1 tb-18\nbuild abc\n'
                        'upload_from_load window\n')
            r = self.row(d, {'.before': before, '.after': after, '.pgbench': ''})
            self.assertEqual(r['skyzfsd']['upload'].get('from_load'), 'window')
            self.assertTrue(r['skyzfsd']['serve_args'].endswith(
                "(uploads to tb-18) (object storage frozen from the daemon's start to its first window's: "
                "the seed in that window)"), r['skyzfsd']['serve_args'])
            # Only the window that let MinIO go is marked (Codex on #132).
            self.assertIs(r['skyzfsd']['released_here'], False)
            with open(os.path.join(d, 'r.released'), 'w') as f:
                f.write('1795069506\n')
            r = self.row(d, {'.before': before, '.after': after, '.pgbench': ''})
            self.assertIs(r['skyzfsd']['released_here'], True)
            os.unlink(os.path.join(d, 'r.released'))
            # anything else in the record is not from the load
            with open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w') as f:
                f.write('serve_args \nrelease_every 0\nupload http://127.0.0.1:9000 tb-1-1 tb-18\nbuild abc\n'
                        'upload_from_load \n')
            r = self.row(d, {'.before': before, '.after': after, '.pgbench': ''})
            self.assertNotIn('from_load', r['skyzfsd']['upload'])
            self.assertTrue(r['skyzfsd']['serve_args'].endswith('(uploads to tb-18)'), r['skyzfsd']['serve_args'])
            with open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w') as f:
                f.write('serve_args \nrelease_every 0\nupload http://127.0.0.1:9000 tb-1-1 tb-18\nbuild abc\n'
                        'upload_from_load 1\n')
            # a row whose status was not read at the window's end (Codex
            # on #128): it still says it uploads, and gets no verdict
            r = self.row(d, {'.before': before, '.after': devs, '.pgbench': 'tps = 950 (without initial connection time)\n'})
            self.assertEqual(r['skyzfsd']['upload']['volume'], 'tb-18')
            self.assertEqual(r['skyzfsd']['upload'].get('from_load'), 'workload')
            self.assertEqual(r['skyzfsd']['after'], {})
            self.assertIsNone(r['skyzfsd']['pid_same'])
            self.assertIn("THE DAEMON'S STATUS WAS NOT READ at the window's end", r['skyzfsd']['serve_args'])
            sys.path.insert(0, HERE)
            import bracket
            r['_seq'], r['machine'] = 2, 'm'
            model = lambda seq: {'_seq': seq, 'topology': 'zfs-slog', 'workload': 'tpcb', 'tps': 1000.0, 'machine': 'm',
                                 'origin_delay_ms': 0}
            v = bracket.bracketed(r, [model(1), r, model(3)])['verdict']
            self.assertTrue(v.startswith("NO VERDICT: the row says it uploads, but its daemon's status was not read, or showed no "
                                         "serving daemon, at the window's end"), v)
            # as a failed status read really leaves the .after (review on
            # #128): its clock line alone
            r2 = self.row(d, {'.before': before, '.after': devs + 'skz read_at_ns 2000\n',
                              '.pgbench': 'tps = 950 (without initial connection time)\n'})
            self.assertEqual(r2['skyzfsd']['upload']['volume'], 'tb-18')
            r2['_seq'], r2['machine'] = 2, 'm'
            v = bracket.bracketed(r2, [model(1), r2, model(3)])['verdict']
            self.assertTrue(v.startswith("NO VERDICT: the row says it uploads, but its daemon's status was not read, or showed no "
                                         "serving daemon, at the window's end"), v)
            out = os.path.join(d, 'out2', '1')
            os.makedirs(out)
            with open(os.path.join(out, 'tpcb-zfs-skyzfsd.json'), 'w') as f:
                json.dump(r, f)
            p = subprocess.run(['bash', SCRIPT, 'summarize', os.path.join(d, 'out2')], capture_output=True, text=True,
                               timeout=120)
            self.assertEqual(p.returncode, 0, p.stderr[-2000:])
            # a row with no status at either end and no uploads has no skyzfsd part
            with open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w') as f:
                f.write('serve_args \nrelease_every 0\nupload none\nbuild abc\n')
            self.assertNotIn('skyzfsd', self.row(d, {'.before': before, '.after': devs, '.pgbench': ''}))
            # a row without uploads
            with open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w') as f:
                f.write('serve_args \nrelease_every 0\nupload none\nbuild abc\n')
            r = self.row(d, {'.before': before, '.after': after, '.pgbench': ''})
            self.assertNotIn('upload', r['skyzfsd'])
            self.assertNotIn('uploads to', r['skyzfsd']['serve_args'])

    def test_the_intent_maps_table(self):
        """The intent map's syncs over the window (M2.3) and the header's
        own (status version 16, --intent-header-sync): a row from a
        daemon with the header's keys (its end values other than its
        start's), one with no header sync in the window (n/a, not a
        division by 0), one from an older daemon (n/a), one whose daemon
        keeps no map (version 16, intent_state off: left out) and one
        from before the map's keys (left out)."""
        # a status read shows the daemon's pid (review of #128, L1)
        devs = 'dev pool 0 0 0 0 0 0 0 0\ndev origin 0 0 0 0 0 0 0 0\nskz pid 7\n'
        old = 'skz intent_state taken_up\nskz intent_syncs {}\nskz intent_sync_ns {}\nskz intent_sync_max_ns {}\nskz intent_waits {}\n'
        new = old + ('skz intent_header_sync {}\nskz intent_header_syncs {}\nskz intent_header_sync_ns {}\n'
                     'skz intent_header_sync_max_ns {}\nskz intent_named {}\nskz intent_regions 8192\n')
        cases = {'on': (devs + new.format(10, 10_000_000, 2_000_000, 2, 'off', 4, 4_000_000, 900_000, 8192),
                        devs + new.format(110, 260_000_000, 3_000_000, 7, 'on', 54, 54_000_000, 1_500_000, 12)),
                 'nohdr': (devs + new.format(10, 10_000_000, 2_000_000, 2, 'off', 0, 0, 0, 30),
                           devs + new.format(30, 50_000_000, 3_000_000, 2, 'off', 0, 0, 0, 20)),
                 'old': (devs + old.format(10, 10_000_000, 3_000_000, 2), devs + old.format(30, 50_000_000, 3_000_000, 2)),
                 'nomap': (devs + new.format(0, 0, 0, 0, 'off', 0, 0, 0, 0).replace('taken_up', 'off'),
                           devs + new.format(0, 0, 0, 0, 'off', 0, 0, 0, 0).replace('taken_up', 'off')),
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
                '| nohdr | zfs-skyzfsd [x] | 701 | off | 20 | 2.00 | 3.00 | 0 | n/a | 0.00 | 0 | 20 / 8192 |',
                '| old | zfs-skyzfsd [x] | 702 | n/a | 20 | 2.00 | 3.00 | n/a | n/a | n/a | 0 | n/a / n/a |']), text[1][:3000])

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
