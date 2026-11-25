#!/usr/bin/env python3
"""The review of #139 (M2.T7): publish_check.main() driven end to end with
no kernel and no object store, so that its premises, its controls and its
wiring (the parts the pull request's unit tests do not reach) are tested.

Node B (nodeb_check.NodeB) is stood in for by a fake that answers
`snap-row` from a table and runs the real `compare` for the files stage;
the request log is given as parsed instances. Each case is a world (what
a course left and what node B finds) and the exit main() must give, and
the lines that must read so.

Usage: PUBLISH_CHECK_DIR=<worktree>/tests/kernel python3 -I drive_publish_check.py [-v]
"""
import hashlib
import io
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

KDIR = os.environ.get('PUBLISH_CHECK_DIR', '')
if not KDIR:
    sys.exit('PUBLISH_CHECK_DIR is not set')
sys.path.insert(0, KDIR)
import cut_daemon_check  # noqa: E402
import nodeb_check  # noqa: E402
import publish_check as pc  # noqa: E402

A = 'a' * 32
S = 'b' * 32


def commit(k):
    return f'{k:064x}'


def sums(files):
    return ''.join(f'{hashlib.sha256(b).hexdigest()}  {fn}\n' for fn, b in sorted(files.items())).encode()


class World:
    """What a course left, and what node B finds, for one course."""

    def __init__(self, course):
        self.course = course
        self.cuts = [(n, 10 + n, True) for n in range(1, 120)]   # number, group, itself
        self.ends = {}
        self.requests = {}       # T -> (G, A, S, C, txg)
        self.rows = {}           # T -> (commit, state, number, group) or None
        self.refs = {}           # T -> commit or None
        self.manifests = {}      # T -> bytes the agent sent
        self.node_files = {}     # T -> {file: bytes} node B finds in the snapshot
        self.manifest_state = {}  # T -> 'matches' | 'differs' | None (no object)
        self.fetched = {}        # T -> bytes node B fetched (defaults to the manifest)
        self.files = {}          # extra files in DIR
        self.stage_fail = {}     # T -> stage that fails on the published snapshot
        getattr(self, course)()

    def publish(self, t, label, k, after, g, txg, number):
        files = {f'{t}-file': f'content of {t}'.encode(), 'common': b'x' * 100}
        self.manifests[t] = sums(files)
        self.node_files[t] = dict(files)
        self.requests[t] = (g, A, S, after, txg)
        h = commit(k)
        self.refs[t] = h
        group = next(c[1] for c in self.cuts if c[0] == number)
        self.rows[t] = (h, S, number, group)
        self.manifest_state[t] = 'matches'
        self.ends[label] = f'done commit={h}'

    def pplain(self):
        self.publish('t1', 't1', 1, 3, 20, 20, 10)
        self.ends.update({'t1-again': 'accepted', 't1-again-end': self.ends['t1'], 't1-other': 'accepted',
                          't1-other-end': 'refused taken snap/t1 records another publish'})
        self.publish('t2', 't2', 2, 12, 25, 25, 15)
        self.ends.update({'att': 'refused attachment x', 'state': 'refused state x'})
        self.publish('t4', 't4', 4, 20, 51, 31, 41)     # cuts 21..40 (groups 31..50) below G
        self.ends.update({'t5': 'expired', 't7': 'pending after 3 s', 't7-other': 'refused asked x',
                          't7-withdraw': 'withdrawn', 't7-again': 'accepted'})
        self.publish('t7', 't7-end', 7, 45, 56, 56, 46)
        for t in ('t3', 't5'):
            self.refs[t], self.rows[t] = None, None

    def prewind(self):
        # The abandoned history's cuts are counted up to C = 30, with
        # groups up to 40; the new history's G is 27.
        self.publish('r1', 'r1', 9, 30, 27, 27, 31)
        self.cuts = [(n, 10 + n, True) for n in range(1, 31)] + [(n, n - 4, n >= 31) for n in range(31, 60)]
        self.rows['r1'] = (self.rows['r1'][0], S, 31, 27)
        self.files['rewind-status.txt'] = 'seconds 1\ncuts_held 2\n'

    def pkill(self):
        self.ends.update({'tw': 'pending after 3 s', 'tw-withdraw': 'withdrawn', 'tk': 'pending after 3 s',
                          'tw-after': 'unknown', 'tk-after': 'unknown', 'tk-again': 'accepted',
                          'tw-last': 'unknown'})
        self.publish('tk', 'tk-end', 5, 6, 45, 15, 35)
        self.requests['tw'] = (40, A, S, 5, 10)
        self.manifests['tw'] = sums({'w': b'w'})
        self.refs['tw'], self.rows['tw'] = None, None

    def kill(self, t, landed):
        self.publish(t, f'{t}-end', 3, 5, 20, 20, 10)
        self.ends.update({t: 'none (exit 2)', f'{t}-after': 'unknown', f'{t}-again': 'accepted'})
        h, st, n, g = self.rows[t]
        self.files[f'{t}-between.txt'] = (f'ref\t{h}\t1\nsnaprow\t{st}\t{n}\t{g}\t{h}\t01ARZ3NDEKTSV4RRFFQ69G5FAV\t'
                                          f'1\t{"d" * 64}\n' if landed else 'ref\tnone\nsnaprow\tnone\n')
        self.files['kill.tsv'] = 'killed_rc\t137\n'

    def pkillman(self):
        self.kill('m1', False)

    def pkillpub(self):
        self.kill('p1', True)

    def pchannel(self):
        self.ends.update({'nu': 'refused no-upload x', 'version': 'refused version x',
                          'malformed': 'refused malformed x', 'name': 'refused name x', 'peer': 'refused peer',
                          'cuts': f'cuts attachment={A} state={S} count=3 group=12 opens=itself'})
        self.files['channel-status.txt'] = 'control uploads\ncontrol_peer 1\ncontrol_malformed 2\n'


class FakeNodeB:
    world = None

    def __init__(self, rdir, logdev):
        self.dir = os.path.join(rdir, 'nodeb')
        os.makedirs(self.dir, exist_ok=True)

    def tool_run(self, args):
        w = self.world
        assert args[0] == 'snap-row', args
        t = args[args.index('--snapshot') + 1]
        out = args[args.index('--out') + 1] if '--out' in args else None
        lines = [f'ref\t{w.refs[t]}\t2' if w.refs.get(t) else 'ref\tnone']
        row = w.rows.get(t)
        if row is None:
            lines.append('snaprow\tnone')
        else:
            h, st, n, g = row
            m = w.manifests.get(t, b'')
            lines.append(f'snaprow\t{st}\t{n}\t{g}\t{h}\t01ARZ3NDEKTSV4RRFFQ69G5FAV\t{len(m)}\t{"d" * 64}')
            if out:
                ms = w.manifest_state.get(t, 'matches')
                if ms is None:
                    lines.append('manifest\tnone')
                else:
                    got = w.fetched.get(t, m)
                    with open(out, 'wb') as f:
                        f.write(got)
                    lines.append(f'manifest\t{len(got)}\t{"d" * 64}\t{ms}')
        return 0, '\n'.join(lines) + '\n'

    def verify(self, vol, ref, snap, want, damaged):
        w = self.world
        seen = nodeb_check.Seen()
        if w.refs.get(snap) is None:
            seen.put('refs', False, f'no ref {ref}')
            return seen
        for st in ('refs', 'content', 'import', 'snapshot', 'mount'):
            seen.put(st, w.stage_fail.get(snap) != st, st)
        got = {fn: hashlib.sha256(b).hexdigest() for fn, b in w.node_files[snap].items()}
        seen.put('files', *nodeb_check.compare(want, got))
        seen.put('copies', w.stage_fail.get(snap) != 'copies', '10 blocks, 20 copies')
        return seen


def run(world):
    """main() on the world -> (exit, {key: (verdict, detail)})."""
    with tempfile.TemporaryDirectory() as d:
        rdir = os.path.join(d, 'rec')
        os.makedirs(rdir)
        with open(os.path.join(rdir, 'ends.tsv'), 'w') as f:
            f.write(''.join(f'{k}\t{v}\n' for k, v in world.ends.items()))
        with open(os.path.join(rdir, 'published.tsv'), 'w') as f:
            for t, (g, a, s, c, txg) in world.requests.items():
                mp = os.path.join(rdir, f'manifest-{t}')
                with open(mp, 'wb') as m:
                    m.write(world.manifests[t])
                f.write(f'{t}\t{g}\t{a}\t{s}\t{c}\t{mp}\t{txg}\n')
        for fn, text in world.files.items():
            with open(os.path.join(rdir, fn), 'w') as f:
                f.write(text)
        log = os.path.join(rdir, 'requests.tsv')
        with open(log, 'w') as f:
            f.write('x\n')
        inst = [{'pid': 1, 'device': 0, 'lines': [
            {'event': 'cut', 'sector': 'itself' if it else 'rewind', 'sectors': str(n), 'result': str(g), 'line': n}
            for n, g, it in world.cuts]}]
        FakeNodeB.world = world
        saved = (pc.nodeb_check.NodeB, pc.cut_daemon_check.parse_log, sys.argv)
        pc.nodeb_check.NodeB = FakeNodeB
        pc.cut_daemon_check.parse_log = lambda text, torn=None: inst
        sys.argv = ['publish_check.py', world.course, rdir, log, '-', '1']
        code = None
        try:
            with redirect_stdout(io.StringIO()):
                pc.main()
        except SystemExit as e:
            code = e.code
        finally:
            pc.nodeb_check.NodeB, pc.cut_daemon_check.parse_log, sys.argv = saved
        lines = {}
        with open(os.path.join(rdir, 'results.tsv')) as f:
            for ln in f:
                k, v, d_ = ln.rstrip('\n').split('\t')
                lines[k] = (v, d_)
        return code, lines


class AsIs(unittest.TestCase):
    """Each course as the daemon should leave it: exit 0, the controls caught."""

    def test_each_course_holds(self):
        for course in pc.COURSES:
            code, lines = run(World(course))
            self.assertEqual(code, 0, (course, {k: v for k, v in lines.items() if v[0] not in ('ok', 'INFO')}))

    def test_the_controls_are_caught_where_they_are_put(self):
        code, lines = run(World('pplain'))
        self.assertEqual(lines['ctl_below'][0], 'CAUGHT')
        self.assertIn('below G', lines['ctl_below'][1])
        self.assertEqual(lines['ctl_commit'][0], 'CAUGHT')
        self.assertEqual(lines['ctl_files'][0], 'CAUGHT_AT_FILES')
        self.assertIn('premise_t4_cuts_below_g', lines)


def wrong(course, edit):
    w = World(course)
    edit(w)
    return run(w)


class TheDaemonWrong(unittest.TestCase):
    """A daemon that does what it must not: exit 1, on the line that says so."""

    def verdict(self, course, edit, key):
        code, lines = wrong(course, edit)
        self.assertEqual(code, 1, lines)
        self.assertEqual(lines[key][0], 'FAILED', (key, lines.get(key)))

    def test_t4_published_by_a_cut_below_g(self):
        def e(w):
            h = w.rows['t4'][0]
            w.rows['t4'] = (h, S, 40, 50)
        self.verdict('pplain', e, 'pin_t4_row')

    def test_t1_ref_on_another_commit(self):
        self.verdict('pplain', lambda w: w.refs.update(t1=commit(99)), 'pin_t1_row')

    def test_t1_row_names_another_commit(self):
        self.verdict('pplain', lambda w: w.rows.update(t1=(commit(99), S, 10, 20)), 'pin_t1_row')

    def test_t1_row_of_another_state(self):
        self.verdict('pplain', lambda w: w.rows.update(t1=(commit(1), 'f' * 32, 10, 20)), 'pin_t1_row')

    def test_t1_cut_counted_at_c(self):
        self.verdict('pplain', lambda w: w.rows.update(t1=(commit(1), S, 3, 13)), 'pin_t1_row')

    def test_t1_cut_that_opens_only_with_a_rewind(self):
        def e(w):
            w.cuts = [(n, g, n != 10) for n, g, _ in w.cuts]
        self.verdict('pplain', e, 'pin_t1_cut')

    def test_t1_row_group_not_the_cut_lines(self):
        self.verdict('pplain', lambda w: w.rows.update(t1=(commit(1), S, 10, 21)), 'pin_t1_cut')

    def test_t1_manifest_differs_from_the_row(self):
        self.verdict('pplain', lambda w: w.manifest_state.update(t1='differs'), 'pin_t1_manifest')

    def test_t1_manifest_not_the_bytes_sent(self):
        self.verdict('pplain', lambda w: w.fetched.update(t1=w.manifests['t1'].replace(b'0', b'1', 1)),
                     'pin_t1_manifest')

    def test_t1_no_manifest_object(self):
        self.verdict('pplain', lambda w: w.manifest_state.update(t1=None), 'pin_t1_manifest')

    def test_t1_snapshot_files_differ(self):
        self.verdict('pplain', lambda w: w.node_files['t1'].update(common=b'y'), 'pin_t1_nodeb')

    def test_t1_snapshot_missing(self):
        self.verdict('pplain', lambda w: w.stage_fail.update(t1='snapshot'), 'pin_t1_nodeb')

    def test_t1_asked_again_done_on_another_commit(self):
        self.verdict('pplain', lambda w: w.ends.update({'t1-again-end': f'done commit={commit(98)}'}),
                     'pin_asking_twice_is_once')

    def test_t1_other_manifest_done(self):
        self.verdict('pplain', lambda w: w.ends.update({'t1-other-end': f'done commit={commit(1)}'}),
                     'pin_answer_t1-other-end')

    def test_t5_there(self):
        self.verdict('pplain', lambda w: w.refs.update(t5=commit(5)), 'pin_t5_absent')

    def test_t3_row_there(self):
        self.verdict('pplain', lambda w: w.rows.update(t3=(commit(3), S, 10, 20)), 'pin_t3_absent')

    def test_tw_there(self):
        self.verdict('pkill', lambda w: w.refs.update(tw=commit(8)), 'pin_tw_absent')

    def test_tw_done_after_the_kill(self):
        self.verdict('pkill', lambda w: w.ends.update({'tw-after': f'done commit={commit(8)}'}),
                     'pin_answer_tw-after')

    def test_tk_known_to_the_next_instance(self):
        self.verdict('pkill', lambda w: w.ends.update({'tk-after': 'pending'}), 'pin_answer_tk-after')

    def test_m1_there_between(self):
        def e(w):
            w.files['m1-between.txt'] = f'ref\t{commit(3)}\t1\nsnaprow\tnone\n'
        self.verdict('pkillman', e, 'pin_m1_not_before_the_transaction')

    def test_r1_by_an_abandoned_cut(self):
        self.verdict('prewind', lambda w: w.rows.update(r1=(commit(9), S, 20, 30)), 'pin_r1_row')

    def test_r1_by_a_cut_that_opens_only_with_a_rewind(self):
        def e(w):
            w.cuts = [(n, g, it if n != 31 else False) for n, g, it in w.cuts]
        self.verdict('prewind', e, 'pin_r1_cut')

    def test_channel_counts(self):
        self.verdict('pchannel', lambda w: w.files.update({'channel-status.txt': 'control_peer 0\n'
                                                                                  'control_malformed 2\n'}),
                     'pin_status_control_peer')


class APremiseThatDoesNotHold(unittest.TestCase):
    """The course did not make its case: the rig's error, exit 2."""

    def error(self, course, edit, key):
        code, lines = wrong(course, edit)
        self.assertEqual(code, 2, lines)
        self.assertEqual(lines[key][0], 'ERROR', (key, lines.get(key)))

    def test_no_cut_below_g_after_c(self):
        def e(w):
            g, a, s, c, txg = w.requests['t4']
            w.requests['t4'] = (g, a, s, 40, txg)
            w.rows['t4'] = (w.rows['t4'][0], S, 41, 51)
        self.error('pplain', e, 'premise_t4_cuts_below_g')

    def test_nothing_held_at_the_rewind(self):
        self.error('prewind', lambda w: w.files.update({'rewind-status.txt': 'cuts_held 0\n'}),
                   'premise_held_at_rewind')

    def test_no_abandoned_cut_at_g(self):
        def e(w):
            w.cuts = [(n, min(g, 26) if n <= 30 else g, it) for n, g, it in w.cuts]
        self.error('prewind', e, 'premise_abandoned_cuts_at_g')

    def test_no_cut_past_tws_g(self):
        def e(w):
            w.cuts = [c for c in w.cuts if c[1] < 40]
            w.rows['tk'] = (w.rows['tk'][0], S, 29, 39)
            w.requests['tk'] = (39, A, S, 6, 15)
        self.error('pkill', e, 'premise_tw_cuts_past_g')

    def test_p1_not_landed(self):
        self.error('pkillpub', lambda w: w.files.update({'p1-between.txt': 'ref\tnone\nsnaprow\tnone\n'}),
                   'premise_p1_landed')


class Gaps(unittest.TestCase):
    """What the check does not see (the review's findings): each passes."""

    def test_p1_found_on_another_commit_than_landed(self):
        # The next instance answers done on a commit other than the one
        # that landed before the kill (it made the publish again).
        def e(w):
            w.refs['p1'] = commit(77)
            w.rows['p1'] = (commit(77), S, 12, 22)
            w.ends['p1-end'] = f'done commit={commit(77)}'
        code, lines = wrong('pkillpub', e)
        self.assertEqual(code, 0, lines)

    def test_m1_daemon_died_otherwise(self):
        # The daemon did not die at the hook (exit 3, no manifest landed):
        # the course reads the same.
        code, lines = wrong('pkillman', lambda w: w.files.update({'kill.tsv': 'killed_rc\t3\n'}))
        self.assertEqual(code, 0, lines)

    def test_prewind_held_cuts_not_of_the_abandoned_history(self):
        # cuts_held counts any cut: two cuts of the shared or new history
        # pass the premise.
        code, lines = wrong('prewind', lambda w: w.files.update({'rewind-status.txt': 'cuts_held 1\n'}))
        self.assertEqual(code, 0, lines)

    def test_published_by_a_later_cut_than_the_first(self):
        # t1 published by cut 12 where cut 10 already qualified.
        code, lines = wrong('pplain', lambda w: w.rows.update(t1=(commit(1), S, 12, 22)))
        self.assertEqual(code, 0, lines)


if __name__ == '__main__':
    unittest.main()
