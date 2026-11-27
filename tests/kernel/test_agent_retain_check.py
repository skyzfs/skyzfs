#!/usr/bin/env python3
"""Tests of agent_retain_check.py: a course whose every case holds, built
in a temporary directory, and each pin made to fail by one change."""
import contextlib
import hashlib
import io
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import agent_retain_check as rc  # noqa: E402
from cut_check import RigError  # noqa: E402

POOL = rc.POOL
PID = 4242
KILLED = 'skyzfs-agent: test hook --kill-at: killed at {} {} ({})'
CUT = 'killed here'
STUCK_SAID = ('skyzfs-agent: report s1 not-retired: tried again at the next round: zfs destroy skzcut/c1@s1: '
              'cannot destroy snapshot skzcut/c1@s1: dataset is busy')


def commit_of(t):
    return hashlib.sha256(t.encode()).hexdigest()


def guid_of(s):
    return str(int(hashlib.sha256(s.encode()).hexdigest()[:12], 16))


def members(t):
    return [f'{POOL}@{t}'] + [f'{POOL}/{c}@{t}' for c in rc.CHILDREN]


def snaps_text(names, extra=(), less=()):
    rows = [(s, guid_of(s)) for t in names for s in members(t) if s not in less] + list(extra)
    return ''.join(f'{s}\t{g}\n' for s, g in rows)


def out_text(rounds):
    """[[item]] -> a run's stdout: each round's items, then its line. An
    item is ('snap', t), ('retired', t), ('abandoned', t, why),
    ('forgotten', t, why), or ('begun', t), which names the round line's
    begun= and prints nothing (by default the round's last snap, or -);
    a round whose last item is CUT was cut by a kill (no round line)."""
    out = []
    for n, items in enumerate(rounds, 1):
        begun = '-'
        for it in items:
            if it == CUT:
                continue
            if it[0] == 'begun':
                begun = it[1]
            elif it[0] == 'snap':
                begun = it[1]
                out.append(f'snap/{it[1]} {commit_of(it[1])}')
            elif it[0] == 'retired':
                out.append(f'retired {it[1]}')
            else:
                out.append(f'{it[0]} {it[1]} {it[2]}')
        if not items or items[-1] != CUT:
            out.append(f'round {n} ok took_ms=1200 late_ms=0 skipped=0 begun={begun} under_way=0 synced=0 bound=0')
    return ''.join(ln + '\n' for ln in out)


def model():
    """The course as the recorder leaves it when every case holds."""
    ks = [f'k{i}' for i in range(1, 8)]
    bs = [f'b{i}' for i in range(1, 14)]
    m = {}
    m['K'] = {
        'runs': {'publish': [[('snap', ks[0])], [('snap', ks[1])], [('snap', ks[2])]]
                 + [[('retired', ks[i - 3]), ('snap', ks[i])] for i in range(3, 7)]},
        'kept': ks[4:], 'retired': ks[:4], 'retires': [(t, 'retired') for t in ks[:4]],
        'extra_snaps': [(f'{POOL}/late@k1', '501'), (f'{POOL}/c2@k1', '301')],
        'files': {'others.tsv': f'late\t{POOL}/late@k1\t-\t501\nremade\t{POOL}/c2@k1\t300\t301\n',
                  'others.rounds': '1\n'},
    }
    m['S'] = {
        'runs': {'publish': [[('snap', 's1')], [('snap', 's2')], [('snap', 's3')],
                             [('retired', 's2'), ('snap', 's4')],
                             [('retired', 's1'), ('retired', 's3'), ('snap', 's5')],
                             [('retired', 's4'), ('snap', 's6')]]},
        'err': {'publish': STUCK_SAID + '\n'},
        'kept': ['s5', 's6'], 'retired': ['s1', 's2', 's3', 's4'],
        'retires': [('s1', 'retired'), ('s2', 'retired'), ('s3', 'retired'), ('s4', 'retired')],
        'files': {'held.rounds': '1\n', 'released.rounds': '4\n',
                  'stuck-dump.txt': 'volume ab\nenrolled skzcut pool_guid=1 leaf_guid=2\n'
                                    f'published s3 commit={commit_of("s3")} group=9 members=4 at_ms=3 retire=-\n'
                                    f'published s4 commit={commit_of("s4")} group=9 members=4 at_ms=3 retire=-\n'
                                    'stuck s1 members=4 at_ms=2\n',
                  'stuck-row.txt': 'ref\tnone\nsnaprow\tnone\n',
                  'stuck-snaps.txt': snaps_text(['s3', 's4'], [(f'{POOL}@s1', '11'), (f'{POOL}/c1@s1', '12')])},
    }
    for cid, x in (('X-retire', 'x'), ('X-retain', 'y'), ('X-destroy', 'z')):
        effect, side, n = rc.KILLS[cid].split(':')
        first = [[('snap', f'{x}1')], [('snap', f'{x}2')],
                 ([('retired', f'{x}1')] if cid == 'X-retain' else []) + [CUT]]
        less = (f'{POOL}/c1/g@{x}1',) if cid == 'X-destroy' else ()
        m[cid] = {
            'runs': {'publish': first,
                     'run2': [[('retired', f'{x}1'), ('snap', f'{x}3'), ('snap', f'{x}4')],
                              [('retired', f'{x}2'), ('retired', f'{x}3'), ('snap', f'{x}5')]]},
            'err': {'publish': KILLED.format(effect, side, n) + '\n'}, 'rc': {'publish': '137'},
            'kept': [f'{x}4', f'{x}5'], 'retired': [f'{x}1', f'{x}2', f'{x}3'],
            'retires': [(f'{x}1', a) for a in rc.X_WANT[cid][0]] + [(f'{x}2', 'retired'), (f'{x}3', 'retired')],
            'files': {'killed-snaps.txt': snaps_text([f'{x}1', f'{x}2'], less=less)},
        }
    m['B'] = {
        'runs': {'publish': [[('snap', t)] for t in bs[:10]],
                 'run2': [[('retired', t) for t in bs[:8]] + [('snap', bs[10])],
                          [('retired', t) for t in bs[8:10]] + [('snap', bs[11])],
                          [('retired', bs[10]), ('snap', bs[12])]]},
        'kept': bs[11:], 'retired': bs[:11], 'retires': [(t, 'retired') for t in bs[:11]],
    }
    m['A-old'] = {
        'runs': {'publish': [[('begun', 'a1')], [('abandoned', 'a1', 'too-old'), ('snap', 'a2')],
                             [('retired', 'a1'), ('snap', 'a3')], [('snap', 'a4')]]},
        'kept': ['a2', 'a3', 'a4'], 'retired': ['a1'], 'retires': [],
        'files': {'given-up.txt': 'volume ab\nabandoned a1 why=too-old at_ms=5 left=members:4\n'},
    }
    for cid, t, left in (('A-nothing', 'n', 'nothing'), ('A-members', 'm', 'members:4')):
        effect, side, n = rc.KILLS[cid].split(':')
        run = [[('snap', f'{t}2')], [('snap', f'{t}3')] + ([('retired', f'{t}1')] if cid == 'A-members' else []),
               [('snap', f'{t}4')]]
        m[cid] = {
            'runs': {'publish': [], 'run': run},
            'err': {'publish': KILLED.format(effect, side, n) + '\n'}, 'rc': {'publish': '137'},
            'kept': [f'{t}2', f'{t}3', f'{t}4'], 'retired': [f'{t}1'] if cid == 'A-members' else [],
            'gone': [f'{t}1'], 'retires': [],
            'files': {'names.txt': f'{t}1\n', 'giveup.out': f'abandoned {t}1 person\n', 'giveup.rc': '3\n',
                      'given-up.txt': f'volume ab\nabandoned {t}1 why=person at_ms=1 left={left}\n'},
        }
    for c in m.values():
        c.setdefault('files', {})
        c.setdefault('err', {})
    return m


def write(d, m, log_extra=(), watch='1\tok head 3\n'):
    """The course's files in D from the model M."""
    log = [f'1000\t{PID}\tstart\t0\t-\t-\t-\t64']
    ns = 1000
    cases = []
    for cid in rc.CASES:
        if cid not in m:
            continue
        c = m[cid]
        cd = os.path.join(d, 'cases', cid)
        os.makedirs(os.path.join(cd, 'agent'), exist_ok=True)
        names = set()
        for lb, rounds in c['runs'].items():
            text = out_text(rounds)
            with open(os.path.join(cd, f'{lb}.out'), 'w') as f:
                f.write(text)
            with open(os.path.join(cd, f'{lb}.err'), 'w') as f:
                f.write(c.get('err', {}).get(lb, ''))
            with open(os.path.join(cd, f'{lb}.rc'), 'w') as f:
                f.write(c.get('rc', {}).get(lb, '0') + '\n')
            for ln in text.splitlines():
                w = ln.split(' ')
                if w[0].startswith('snap/'):
                    names.add(w[0][5:])
                elif w[0] == 'abandoned':
                    names.add(w[1])
        names |= set(c.get('gone', ()))
        files = dict(c.get('files', {}))
        files['printed.txt'] = ''.join(t + '\n' for t in sorted(names))
        for t in names:
            files.setdefault(f'row-{t}.txt', f'ref\t{commit_of(t)}\t1\nsnaprow\tpublished\t3\t40\t{commit_of(t)}\t{t}\t10\t'
                                         f'{"0" * 64}\nmanifest\t10\t{"0" * 64}\tmatches\n'
                             if t in c['kept'] else 'ref\tnone\nsnaprow\tnone\nmanifest\tnone\n')
        files.setdefault('snaps-after.txt', snaps_text(c['kept'], c.get('extra_snaps', ())))
        files.setdefault('record-after.txt', 'volume ab\nenrolled skzcut pool_guid=1 leaf_guid=2\n' + ''.join(
            f'published {t} commit={commit_of(t)} group=9 members=4 at_ms=3 retire=-\n' for t in c['kept']))
        files.setdefault('record-after.rc', '0\n')
        counts = {k: 0 for k in rc.STATUS_KEYS}
        for r in c['retires']:
            t, a = r[:2]
            counts[rc.ANSWER_KEY[a]] += 1
            ns += 10
            log.append(f'{ns}\t{PID}\tpublish\t-\tretire\t-\t-\t{t} {a} commit={r[2] if len(r) > 2 else commit_of(t)}')
        base = {'publish_retired': 7, 'publish_retire_gone': 2, 'publish_retire_other': 0}
        files.setdefault('status-before.txt', 'version 20\n' + ''.join(f'{k} {v}\n' for k, v in base.items()))
        files.setdefault('status-after.txt', 'version 20\n' + ''.join(
            f'{k} {base[k] + counts[k]}\n' for k in rc.STATUS_KEYS))
        files.setdefault('holds.txt', '')
        for name, text in files.items():
            if text is None:
                continue
            with open(os.path.join(cd, name), 'w') as f:
                f.write(text)
        cases.append(f'{cid}\tscen\t{rc.KILLS.get(cid, "-")}\t0\t-')
    log += list(log_extra)
    with open(os.path.join(d, 'requests.tsv'), 'w') as f:
        f.write(''.join(ln + '\n' for ln in log))
    with open(os.path.join(d, 'cases.tsv'), 'w') as f:
        f.write(''.join(ln + '\n' for ln in cases))
    with open(os.path.join(d, 'watch.log'), 'w') as f:
        f.write(watch)


def run(change=None, **kw):
    """-> (status, {key: verdict}) of the check on the model, changed."""
    m = model()
    if change:
        change(m)
    with tempfile.TemporaryDirectory() as d:
        write(d, m, **kw)
        with contextlib.redirect_stdout(io.StringIO()):
            st = rc.main(['aretain', d, os.path.join(d, 'requests.tsv')])
        with open(os.path.join(d, 'results.tsv')) as f:
            lines = [ln.split('\t') for ln in f.read().splitlines()]
    return st, {ln[0]: ln[1] for ln in lines}, {ln[0]: ln[2] for ln in lines}


class Whole(unittest.TestCase):
    def test_every_pin_holds(self):
        st, v, d = run()
        bad = {k: (x, d[k]) for k, x in v.items() if x not in ('ok', 'INFO', 'CAUGHT')}
        self.assertEqual(bad, {})
        self.assertEqual(st, 0)
        for key in ('pin_K_retired', 'pin_K_keep', 'pin_K_retired_in_round', 'pin_K_others_kept', 'pin_K_request_log',
                    'ctl_K_keep_3', 'ctl_K_left', 'pin_S_stuck', 'pin_S_children_first', 'pin_S_said',
                    'pin_S_others_go_on', 'pin_S_released', 'pin_S_request_log', 'pin_X-retire_said',
                    'pin_X-retire_request_log', 'pin_X-retire_nothing_destroyed', 'pin_X-retain_said',
                    'pin_X-destroy_first_member', 'pin_B_kept_60', 'pin_B_retired', 'pin_B_steps', 'obs_B_rounds',
                    'pin_A-old_left', 'pin_A-old_retired', 'pin_A-nothing_left', 'pin_A-nothing_dropped',
                    'pin_A-members_left', 'pin_A-members_retired', 'pin_watcher_reads'):
            self.assertIn(key, v)
        for cid in rc.CASES:
            for p in ('record_reads', 'retired_gone', 'kept', 'holds', 'status'):
                self.assertEqual(v[f'pin_{cid}_{p}'], 'ok', (cid, p))
        self.assertEqual(v['ctl_K_keep_3'], 'CAUGHT')
        self.assertEqual(v['ctl_K_left'], 'CAUGHT')

    def test_agent_check_hands_it_on(self):
        m = model()
        with tempfile.TemporaryDirectory() as d:
            write(d, m)
            p = subprocess.run([sys.executable, '-I', os.path.join(HERE, 'agent_check.py'), 'aretain', d,
                                os.path.join(d, 'requests.tsv'), '-', '1024'], capture_output=True, text=True)
            self.assertEqual(p.returncode, 0, p.stdout[-2000:] + p.stderr[-2000:])
            with open(os.path.join(d, 'results.tsv')) as f:
                self.assertIn('pin_K_retired\tok', f.read())


def fails(test, change, key, want='FAILED', status=1, **kw):
    st, v, d = run(change, **kw)
    test.assertEqual(v.get(key), want, d.get(key))
    test.assertEqual(st, status, {k: x for k, x in v.items() if x not in ('ok', 'INFO', 'CAUGHT')})
    return v, d


def set_round(m, cid, lb, n, items):
    m[cid]['runs'][lb][n - 1] = items


class Common(unittest.TestCase):
    def test_a_retired_name_left_on_the_pool(self):
        def ch(m):
            m['K']['extra_snaps'].append((f'{POOL}/c1@k2', '77'))
        _, d = fails(self, ch, 'pin_K_retired_gone')
        self.assertIn('skzcut/c1@k2', d['pin_K_retired_gone'])

    def test_a_retired_name_with_a_ref(self):
        def ch(m):
            m['B']['files'] = {'row-b3.txt': f'ref\t{commit_of("b3")}\t1\nsnaprow\tnone\n'}
        fails(self, ch, 'pin_B_retired_gone')

    def test_a_retired_name_with_a_row(self):
        def ch(m):
            m['S']['files']['row-s2.txt'] = (f'ref\tnone\nsnaprow\tpublished\t3\t40\t{commit_of("s2")}\ts2\t10\t'
                                             f'{"0" * 64}\n')
        fails(self, ch, 'pin_S_retired_gone')

    def test_a_retired_name_in_the_record(self):
        def ch(m):
            m['X-retain']['files']['record-after.txt'] = (
                f'published y4 commit={commit_of("y4")} group=9 members=4 at_ms=3 retire=-\n'
                f'published y2 commit={commit_of("y2")} group=9 members=4 at_ms=3 retire=members\n')
        fails(self, ch, 'pin_X-retain_retired_gone')

    def test_a_kept_name_marked(self):
        def ch(m):
            m['B']['files']['record-after.txt'] = f'published b13 commit={commit_of("b13")} group=9 members=4 retire=ref\n'
        fails(self, ch, 'pin_B_kept')

    def test_a_kept_name_on_another_commit(self):
        def ch(m):
            m['A-old']['files']['row-a3.txt'] = f'ref\t{commit_of("zz")}\t1\nsnaprow\tnone\n'
        fails(self, ch, 'pin_A-old_kept')

    def test_a_kept_name_with_no_pool_snapshot(self):
        def ch(m):
            m['A-nothing']['files']['snaps-after.txt'] = snaps_text(['n2', 'n3'])
        fails(self, ch, 'pin_A-nothing_kept')

    def test_a_kept_name_not_in_the_record(self):
        def ch(m):
            m['A-members']['files']['record-after.txt'] = f'published m4 commit={commit_of("m4")} retire=-\n'
        fails(self, ch, 'pin_A-members_kept')

    def test_a_hold_left(self):
        def ch(m):
            m['K']['files']['holds.txt'] = f'{POOL}@k7\tskyzfs-publish\tThu Nov 27 10:00 2026\n'
        fails(self, ch, 'pin_K_holds')

    def test_a_hold_of_another_tag_is_not_the_agents(self):
        def ch(m):
            m['K']['files']['holds.txt'] = f'{POOL}@k7\tskz-rig\tThu Nov 27 10:00 2026\n'
        st, v, _ = run(ch)
        self.assertEqual((st, v['pin_K_holds']), (0, 'ok'))

    def test_the_record_unread(self):
        def ch(m):
            m['B']['files']['record-after.rc'] = '6\n'
        fails(self, ch, 'pin_B_record_reads')

    def test_the_status_counts_one_retire_less(self):
        def ch(m):
            m['K']['files']['status-after.txt'] = ('publish_retired 10\npublish_retire_gone 2\n'
                                                   'publish_retire_other 0\n')
        _, d = fails(self, ch, 'pin_K_status')
        self.assertIn('publish_retired counted 3 over the case, the request log has 4', d['pin_K_status'])

    def test_the_status_counts_a_gone(self):
        def ch(m):
            m['X-retire']['files']['status-after.txt'] = ('publish_retired 10\npublish_retire_gone 2\n'
                                                          'publish_retire_other 0\n')
        _, d = fails(self, ch, 'pin_X-retire_status')
        self.assertIn('publish_retire_gone counted 0', d['pin_X-retire_status'])

    def test_a_retire_on_another_commit(self):
        st, v, d = run(None, log_extra=[f'5000\t{PID}\tpublish\t-\tretire\t-\t-\tb13 refused other '
                                        f'commit={commit_of("q")} found={commit_of("b13")}'])
        self.assertEqual(v['pin_B_status'], 'FAILED')
        self.assertIn('names commit', d['pin_B_status'])
        self.assertEqual(st, 1)

    def test_a_status_from_before_the_counts(self):
        def ch(m):
            m['S']['files']['status-before.txt'] = 'version 19\n'
        fails(self, ch, 'premise_S_status', 'ERROR', 2)

    def test_the_daemon_served_again(self):
        def ch(m):
            m['S']['files']['recovered.txt'] = 'the daemon served again\n'
        fails(self, ch, 'premise_S_status', 'ERROR', 2)

    def test_a_retired_name_left_under_a_rig_snapshots_name_with_another_guid(self):
        def ch(m):
            m['K']['extra_snaps'] = [(f'{POOL}/late@k1', '502'), (f'{POOL}/c2@k1', '301')]
        _, d = fails(self, ch, 'pin_K_retired_gone')
        self.assertIn('skzcut/late@k1', d['pin_K_retired_gone'])

    def test_a_kept_name_on_another_commit_in_the_record(self):
        def ch(m):
            m['K']['files']['record-after.txt'] = (
                f'published k6 commit={commit_of("k6")} group=9 members=4 at_ms=3 retire=-\n'
                f'published k7 commit={commit_of("zz")} group=9 members=4 at_ms=3 retire=-\n')
        _, d = fails(self, ch, 'pin_K_kept')
        self.assertIn('the record has k7 on commit', d['pin_K_kept'])

    def test_the_control_missed(self):
        j = rc.Judge('x', out=io.StringIO())
        j.control('ctl_x', [], 'what')
        self.assertEqual((j.lines, j.status()), (['ctl_x\tMISSED\t[check] what: the judge found nothing'], 1))
        j = rc.Judge('x', out=io.StringIO())
        j.control('ctl_x', ['a'], 'what')
        self.assertEqual((j.lines, j.status()), (['ctl_x\tCAUGHT\t[check] what: a'], 0))

    def test_rounds_counted_on_across_runs(self):
        with tempfile.TemporaryDirectory() as d:
            write(d, model())
            k = rc.read_case(rc.Case(d, 'X-retire'), [])
        self.assertEqual([(ln['run'], ln['kind'], ln['round']) for ln in k['lines'] if ln['kind'] != 'round'],
                         [('publish', 'snap', 1), ('publish', 'snap', 2), ('run2', 'retired', 3),
                          ('run2', 'snap', 3), ('run2', 'snap', 3), ('run2', 'retired', 4), ('run2', 'retired', 4),
                          ('run2', 'snap', 4)])


class Keep(unittest.TestCase):
    def test_retired_a_round_early(self):
        def ch(m):
            set_round(m, 'K', 'publish', 2, [('snap', 'k2'), ('retired', 'k1')])
            set_round(m, 'K', 'publish', 3, [('snap', 'k3')])
        fails(self, ch, 'pin_K_keep')

    def test_retired_a_round_late(self):
        def ch(m):
            set_round(m, 'K', 'publish', 4, [('snap', 'k4')])
            set_round(m, 'K', 'publish', 5, [('retired', 'k1'), ('retired', 'k2'), ('snap', 'k5')])
        _, d = fails(self, ch, 'pin_K_retired_in_round')
        self.assertIn('k1 said retired in round 5, not in round 4', d['pin_K_retired_in_round'])

    def test_late_where_a_report_names_it(self):
        def ch(m):
            set_round(m, 'K', 'publish', 4, [('snap', 'k4')])
            set_round(m, 'K', 'publish', 5, [('retired', 'k1'), ('retired', 'k2'), ('snap', 'k5')])
            m['K']['err'] = {'publish': 'skyzfs-agent: report k1 not-retired: tried again at the next round: '
                                        'retire not answered: timed out\n'}
        st, v, _ = run(ch)
        self.assertEqual((v['pin_K_retired_in_round'], st), ('ok', 0))

    def test_retired_out_of_order(self):
        def ch(m):
            set_round(m, 'K', 'publish', 4, [('snap', 'k4')])
            set_round(m, 'K', 'publish', 5, [('snap', 'k5'), ('retired', 'k3'), ('retired', 'k2')])
        fails(self, ch, 'pin_K_retired')

    def test_one_not_retired(self):
        def ch(m):
            set_round(m, 'K', 'publish', 7, [('snap', 'k7')])
            m['K']['retired'] = m['K']['retired'][:4]
            m['K']['retires'] = m['K']['retires'][:4]
            m['K']['files']['row-k5.txt'] = f'ref\t{commit_of("k5")}\t1\nsnaprow\tnone\n'
        fails(self, ch, 'pin_K_retired')

    def test_the_record_keeps_three(self):
        def ch(m):
            m['K']['files']['record-after.txt'] = ''.join(
                f'published {t} commit={commit_of(t)} retire=-\n' for t in ('k6', 'k7'))
            m['K']['files']['record-after.txt'] += 'abandoned k0 why=person at_ms=1 left=nothing\n'
        fails(self, ch, 'pin_K_retired')

    def test_the_remade_snapshot_destroyed(self):
        def ch(m):
            m['K']['extra_snaps'] = [(f'{POOL}/late@k1', '501')]
        _, d = fails(self, ch, 'pin_K_others_kept')
        self.assertIn('skzcut/c2@k1', d['pin_K_others_kept'])

    def test_the_late_snapshot_destroyed(self):
        def ch(m):
            m['K']['extra_snaps'] = [(f'{POOL}/c2@k1', '301')]
        fails(self, ch, 'pin_K_others_kept')

    def test_the_others_made_late(self):
        def ch(m):
            m['K']['files']['others.rounds'] = '2\n'
        fails(self, ch, 'premise_K_others', 'ERROR', 2)

    def test_the_remade_snapshot_has_its_old_guid(self):
        def ch(m):
            m['K']['files']['others.tsv'] = f'late\t{POOL}/late@k1\t-\t501\nremade\t{POOL}/c2@k1\t301\t301\n'
        fails(self, ch, 'premise_K_others', 'ERROR', 2)

    def test_too_few_names(self):
        def ch(m):
            m['K']['runs']['publish'] = [[('snap', 'k1')], [('snap', 'k2')], [('snap', 'k3'), ('retired', 'k1')]]
        fails(self, ch, 'premise_K_names', 'ERROR', 2)

    def test_a_retire_line_twice(self):
        def ch(m):
            m['K']['retires'] = [('k1', 'retired')] + m['K']['retires']
            m['K']['files']['status-after.txt'] = ('publish_retired 13\npublish_retire_gone 2\n'
                                                   'publish_retire_other 0\n')
        fails(self, ch, 'pin_K_request_log')

    def test_a_retire_of_a_kept_name(self):
        def ch(m):
            m['K']['retires'].append(('k7', 'gone'))
        fails(self, ch, 'pin_K_request_log')

    def test_a_retire_line_on_another_commit(self):
        def ch(m):
            m['K']['retires'][2] = ('k3', 'retired', commit_of('zz'))
        _, d = fails(self, ch, 'pin_K_request_log')
        self.assertIn('the retire of k3 (retired) names commit', d['pin_K_request_log'])

    def test_retire_lines_out_of_order(self):
        def ch(m):
            r = m['K']['retires']
            r[0], r[1] = r[1], r[0]
        fails(self, ch, 'pin_K_request_log')

    def test_retired_in_the_round_that_published_the_keepth_after_it(self):
        def ch(m):
            set_round(m, 'K', 'publish', 3, [('retired', 'k1'), ('snap', 'k3')])
            set_round(m, 'K', 'publish', 4, [('snap', 'k4')])
        _, d = fails(self, ch, 'pin_K_keep')
        self.assertIn('k1 said retired in round 3, not after k3 was published (round 3)', d['pin_K_keep'])

    def test_the_newest_two_alone_kept_after_the_last_round(self):
        def ch(m):
            set_round(m, 'K', 'publish', 7, [('retired', 'k4'), ('retired', 'k5'), ('snap', 'k7')])
            m['K']['kept'], m['K']['retired'] = ['k6', 'k7'], ['k1', 'k2', 'k3', 'k4', 'k5']
            m['K']['retires'].append(('k5', 'retired'))
        v, d = fails(self, ch, 'pin_K_retired')
        self.assertEqual(v['pin_K_keep'], 'FAILED')

    def test_due_retired(self):
        lines = rc.read_out(out_text([[('snap', 'a')], [('snap', 'b')], [('snap', 'c'), ('snap', 'd')], [('snap', 'e')],
                                      [CUT]]))
        self.assertEqual((rc.due_retired(lines, 1), rc.due_retired(lines, 3), rc.due_retired(lines, 9)),
                         (['a', 'b', 'c'], ['a'], []))
        self.assertEqual(rc.due_retired([], 1), [])

    def test_judge_keep(self):
        lines = rc.read_out(out_text(model()['K']['runs']['publish']))
        self.assertEqual(rc.judge_keep(lines, 2), [])
        self.assertEqual(len(rc.judge_keep(lines, 3)), 4)
        self.assertEqual(len(rc.judge_keep(lines, 1)), 0)


class Stuck(unittest.TestCase):
    def test_not_stuck(self):
        def ch(m):
            m['S']['files']['stuck-dump.txt'] = f'published s1 commit={commit_of("s1")} retire=members\n'
        fails(self, ch, 'pin_S_stuck')

    def test_neither_stuck_nor_published(self):
        def ch(m):
            m['S']['files']['stuck-dump.txt'] = 'volume ab\n'
        fails(self, ch, 'pin_S_stuck')

    def test_the_ref_kept_while_stuck(self):
        def ch(m):
            m['S']['files']['stuck-row.txt'] = f'ref\t{commit_of("s1")}\t1\nsnaprow\tnone\n'
        fails(self, ch, 'pin_S_stuck')

    def test_the_grandchild_left(self):
        def ch(m):
            m['S']['files']['stuck-snaps.txt'] += f'{POOL}/c1/g@s1\t13\n'
        fails(self, ch, 'pin_S_children_first')

    def test_the_root_destroyed_first(self):
        def ch(m):
            m['S']['files']['stuck-snaps.txt'] = f'{POOL}/c1@s1\t12\n'
        fails(self, ch, 'pin_S_children_first')

    def test_said_twice(self):
        def ch(m):
            m['S']['err']['publish'] = STUCK_SAID + '\n' + STUCK_SAID + '\n'
        fails(self, ch, 'pin_S_said')

    def test_said_again_for_another_reason(self):
        def ch(m):
            m['S']['err']['publish'] = STUCK_SAID + '\n' + STUCK_SAID.replace('busy', 'gone') + '\n'
        st, v, _ = run(ch)
        self.assertEqual((v['pin_S_said'], st), ('ok', 0))

    def test_not_said(self):
        def ch(m):
            m['S']['err']['publish'] = ''
        fails(self, ch, 'pin_S_said')

    def test_said_of_the_root(self):
        def ch(m):
            m['S']['err']['publish'] = STUCK_SAID.replace('zfs destroy skzcut/c1@s1', 'zfs destroy skzcut@s1') + '\n'
        fails(self, ch, 'pin_S_said')

    def test_the_others_wait(self):
        def ch(m):
            set_round(m, 'S', 'publish', 4, [('snap', 's4')])
            set_round(m, 'S', 'publish', 5, [('retired', 's1'), ('retired', 's2'), ('retired', 's3'), ('snap', 's5')])
        fails(self, ch, 'pin_S_others_go_on')

    def test_retired_while_held(self):
        def ch(m):
            set_round(m, 'S', 'publish', 3, [('snap', 's3'), ('retired', 's2'), ('retired', 's1')])
            set_round(m, 'S', 'publish', 4, [('snap', 's4'), ('retired', 's3')])
        fails(self, ch, 'pin_S_released')

    def test_retired_three_rounds_after(self):
        def ch(m):
            set_round(m, 'S', 'publish', 4, [('snap', 's4'), ('retired', 's3')])
            set_round(m, 'S', 'publish', 6, [('snap', 's6'), ('retired', 's5'), ('retired', 's1')])
        fails(self, ch, 'pin_S_released')

    def test_retired_twice(self):
        def ch(m):
            set_round(m, 'S', 'publish', 6, [('retired', 's4'), ('retired', 's1'), ('snap', 's6')])
        fails(self, ch, 'pin_S_released')

    def test_asked_again_while_stuck(self):
        def ch(m):
            m['S']['retires'].insert(1, ('s1', 'gone'))
        fails(self, ch, 'pin_S_request_log')

    def test_held_late(self):
        def ch(m):
            m['S']['files']['held.rounds'] = '2\n'
        fails(self, ch, 'premise_S_held', 'ERROR', 2)

    def test_released_early(self):
        def ch(m):
            m['S']['files']['released.rounds'] = '2\n'
        fails(self, ch, 'premise_S_held', 'ERROR', 2)


class Kill(unittest.TestCase):
    def test_not_killed(self):
        def ch(m):
            m['X-retire']['err']['publish'] = KILLED.format('retire', 'before', 1) + '\n'
        fails(self, ch, 'premise_X-retire_killed', 'ERROR', 2)

    def test_retain_said_once(self):
        def ch(m):
            set_round(m, 'X-retain', 'run2', 1, [('snap', 'y3'), ('retired', 'y2')])
        _, d = fails(self, ch, 'pin_X-retain_said')
        self.assertIn('1 times by the killed run and 0 by run2, not 1 and 1', d['pin_X-retain_said'])

    def test_retire_said_twice(self):
        def ch(m):
            m['X-retire']['runs']['publish'][1] = [('snap', 'x2'), ('retired', 'x1'), CUT]
        fails(self, ch, 'pin_X-retire_said')

    def test_retire_not_asked_again(self):
        def ch(m):
            m['X-retire']['retires'] = [('x1', 'retired'), ('x2', 'retired'), ('x3', 'retired')]
        fails(self, ch, 'pin_X-retire_request_log')

    def test_destroy_asked_again(self):
        def ch(m):
            m['X-destroy']['retires'].insert(1, ('z1', 'gone'))
        fails(self, ch, 'pin_X-destroy_request_log')

    def test_retire_destroyed_before_the_kill(self):
        def ch(m):
            m['X-retire']['files']['killed-snaps.txt'] = snaps_text(['x1'], less=(f'{POOL}/c2@x1',))
        fails(self, ch, 'pin_X-retire_nothing_destroyed')

    def test_destroy_destroyed_another(self):
        def ch(m):
            m['X-destroy']['files']['killed-snaps.txt'] = snaps_text(['z1'], less=(f'{POOL}@z1',))
        fails(self, ch, 'pin_X-destroy_first_member')

    def test_destroy_destroyed_none(self):
        def ch(m):
            m['X-destroy']['files']['killed-snaps.txt'] = snaps_text(['z1'])
        fails(self, ch, 'pin_X-destroy_first_member')

    def test_x1_left_after_run2(self):
        def ch(m):
            m['X-destroy']['extra_snaps'] = [(f'{POOL}@z1', '9')]
        fails(self, ch, 'pin_X-destroy_retired_gone')


class Backlog(unittest.TestCase):
    def test_nine_in_a_round(self):
        def ch(m):
            bs = [f'b{i}' for i in range(1, 14)]
            set_round(m, 'B', 'run2', 1, [('snap', 'b11')] + [('retired', t) for t in bs[:9]])
            set_round(m, 'B', 'run2', 2, [('snap', 'b12')] + [('retired', t) for t in bs[9:11]])
        _, d = fails(self, ch, 'pin_B_steps')
        self.assertIn('round 1 of run2 said 9 retired', d['pin_B_steps'])

    def test_nine_in_a_round_with_a_report(self):
        def ch(m):
            bs = [f'b{i}' for i in range(1, 14)]
            set_round(m, 'B', 'run2', 1, [('snap', 'b11')] + [('retired', t) for t in bs[:9]])
            set_round(m, 'B', 'run2', 2, [('snap', 'b12')] + [('retired', t) for t in bs[9:11]])
            m['B']['err'] = {'run2': 'skyzfs-agent: report b2 not-retired: tried again at the next round: x\n'}
        v, d = fails(self, ch, 'pin_B_steps')
        self.assertEqual(v['premise_B_backlog'], 'ERROR')

    def test_seven_in_round_one(self):
        def ch(m):
            bs = [f'b{i}' for i in range(1, 14)]
            set_round(m, 'B', 'run2', 1, [('snap', 'b11')] + [('retired', t) for t in bs[:7]])
            set_round(m, 'B', 'run2', 2, [('snap', 'b12')] + [('retired', t) for t in bs[7:11]])
        _, d = fails(self, ch, 'pin_B_steps')
        self.assertIn('round 1 of run2 said 7 retired, not 8', d['pin_B_steps'])

    def test_a_report_makes_round_one_unjudged(self):
        def ch(m):
            m['B']['err'] = {'run2': 'skyzfs-agent: report b2 not-retired: tried again at the next round: x\n'}
        fails(self, ch, 'premise_B_backlog', 'ERROR', 2)

    def test_retired_at_keep_60(self):
        def ch(m):
            set_round(m, 'B', 'publish', 10, [('snap', 'b10'), ('retired', 'b1')])
            set_round(m, 'B', 'run2', 1, [('snap', 'b11')] + [('retired', f'b{i}') for i in range(2, 10)])
            set_round(m, 'B', 'run2', 2, [('snap', 'b12'), ('retired', 'b10'), ('retired', 'b11')])
        fails(self, ch, 'pin_B_kept_60')

    def test_one_kept_too_many(self):
        def ch(m):
            set_round(m, 'B', 'run2', 3, [('snap', 'b13')])
            m['B']['kept'] = ['b12', 'b13']
            m['B']['retired'] = m['B']['retired'][:11]
            m['B']['retires'] = m['B']['retires'][:11]
        fails(self, ch, 'pin_B_retired')

    def test_too_few_names(self):
        def ch(m):
            m['B']['runs']['publish'] = m['B']['runs']['publish'][:9]
        fails(self, ch, 'premise_B_names', 'ERROR', 2)


class Abandoned(unittest.TestCase):
    def test_too_old_left_nothing(self):
        def ch(m):
            m['A-old']['files']['given-up.txt'] = 'abandoned a1 why=too-old at_ms=5 left=nothing\n'
        fails(self, ch, 'pin_A-old_left')

    def test_too_old_left_unknown(self):
        def ch(m):
            m['A-old']['files']['given-up.txt'] = 'abandoned a1 why=too-old at_ms=5 left=unknown\n'
        fails(self, ch, 'pin_A-old_left')

    def test_too_old_retired_in_the_round_that_gave_it_up(self):
        def ch(m):
            set_round(m, 'A-old', 'publish', 2, [('abandoned', 'a1', 'too-old'), ('retired', 'a1'), ('snap', 'a2')])
            set_round(m, 'A-old', 'publish', 3, [('snap', 'a3')])
        fails(self, ch, 'pin_A-old_retired')

    def test_too_old_forgotten(self):
        def ch(m):
            set_round(m, 'A-old', 'publish', 3, [('snap', 'a3'), ('forgotten', 'a1', '(why)')])
            m['A-old']['retired'] = []
        fails(self, ch, 'pin_A-old_retired')

    def test_given_up_not_the_one_round_one_began(self):
        def ch(m):
            set_round(m, 'A-old', 'publish', 1, [('begun', 'a0')])
        fails(self, ch, 'premise_A-old_given_up', 'ERROR', 2)

    def test_given_up_in_round_one(self):
        def ch(m):
            set_round(m, 'A-old', 'publish', 1, [('abandoned', 'a1', 'too-old'), ('begun', 'a1')])
            set_round(m, 'A-old', 'publish', 2, [('snap', 'a2')])
        st, v, _ = run(ch)
        self.assertEqual((v['premise_A-old_given_up'], v['pin_A-old_retired'], st), ('ok', 'ok', 0))

    def test_too_old_forgotten_as_well(self):
        def ch(m):
            set_round(m, 'A-old', 'publish', 4, [('snap', 'a4'), ('forgotten', 'a1', '(x)')])
        fails(self, ch, 'pin_A-old_retired')

    def test_not_too_old(self):
        def ch(m):
            set_round(m, 'A-old', 'publish', 1, [('abandoned', 'a1', 'no-progress')])
        fails(self, ch, 'premise_A-old_given_up', 'ERROR', 2)

    def test_nothing_said(self):
        def ch(m):
            set_round(m, 'A-nothing', 'run', 2, [('snap', 'n3'), ('retired', 'n1')])
        fails(self, ch, 'pin_A-nothing_dropped')

    def test_nothing_forgotten(self):
        def ch(m):
            set_round(m, 'A-nothing', 'run', 2, [('snap', 'n3'), ('forgotten', 'n1', '(x)')])
        fails(self, ch, 'pin_A-nothing_dropped')

    def test_nothing_kept(self):
        def ch(m):
            m['A-nothing']['files']['record-after.txt'] = ''.join(
                f'published {t} commit={commit_of(t)} retire=-\n' for t in ('n2', 'n3', 'n4'))
            m['A-nothing']['files']['record-after.txt'] += 'abandoned n1 why=person at_ms=1 left=nothing\n'
        fails(self, ch, 'pin_A-nothing_dropped')

    def test_nothing_left_members(self):
        def ch(m):
            m['A-nothing']['files']['given-up.txt'] = 'abandoned n1 why=person at_ms=1 left=members:4\n'
        fails(self, ch, 'pin_A-nothing_left')

    def test_members_left_nothing(self):
        def ch(m):
            m['A-members']['files']['given-up.txt'] = 'abandoned m1 why=person at_ms=1 left=nothing\n'
        fails(self, ch, 'pin_A-members_left')

    def test_members_given_up_by_another(self):
        def ch(m):
            m['A-members']['files']['given-up.txt'] = 'abandoned m1 why=too-old at_ms=1 left=members:4\n'
        fails(self, ch, 'pin_A-members_left')

    def test_members_not_retired(self):
        def ch(m):
            set_round(m, 'A-members', 'run', 2, [('snap', 'm3')])
            m['A-members']['retired'] = []
        fails(self, ch, 'pin_A-members_retired')

    def test_members_forgotten_as_well(self):
        def ch(m):
            set_round(m, 'A-members', 'run', 3, [('snap', 'm4'), ('forgotten', 'm1', '(x)')])
        fails(self, ch, 'pin_A-members_retired')

    def test_members_left_on_the_pool(self):
        def ch(m):
            m['A-members']['extra_snaps'] = [(f'{POOL}/c1@m1', '8')]
        fails(self, ch, 'pin_A-members_retired_gone')

    def test_no_name_before_the_kill(self):
        def ch(m):
            m['A-members']['files']['names.txt'] = '-\n'
        fails(self, ch, 'premise_A-members_name', 'ERROR', 2)


class Course(unittest.TestCase):
    def test_a_case_with_the_rigs_error(self):
        def ch(m):
            m['S']['files']['rig-error.txt'] = 'round 1 published no name\n'
        st, v, _ = run(ch)
        self.assertEqual(v['premise_S_rig'], 'ERROR')
        self.assertNotIn('pin_S_stuck', v)
        self.assertEqual(v['pin_K_retired'], 'ok')
        self.assertEqual(st, 2)

    def test_a_verdict_of_the_recorder(self):
        def ch(m):
            m['B']['files']['verdicts.tsv'] = 'rounds\tthe run printed 2 round lines, not 10\n'
        fails(self, ch, 'pin_B_rounds')

    def test_a_verdict_beats_an_error(self):
        def ch(m):
            m['B']['files']['verdicts.tsv'] = 'rounds\tthe run printed 2 round lines, not 10\n'
            m['S']['files']['rig-error.txt'] = 'x\n'
        fails(self, ch, 'pin_B_rounds')

    def test_the_watcher_saw_a_verdict(self):
        fails(self, None, 'pin_watcher_reads', watch='1\tVERDICT head unreadable\n')

    def test_the_watcher_wrote_nothing(self):
        fails(self, None, 'premise_watcher', 'ERROR', 2, watch='')

    def test_a_case_not_reached(self):
        def ch(m):
            del m['A-members']
        st, v, _ = run(ch)
        self.assertEqual((v['case_A-members'], st), ('INFO', 0))

    def test_a_file_missing(self):
        def ch(m):
            m['K']['files']['snaps-after.txt'] = None
        fails(self, ch, 'premise_K_files', 'ERROR', 2)

    def test_not_the_course(self):
        with tempfile.TemporaryDirectory() as d:
            with contextlib.redirect_stdout(io.StringIO()):
                st = rc.main(['afail', d, os.path.join(d, 'requests.tsv')])
        self.assertEqual(st, 2)


class Reading(unittest.TestCase):
    def test_lines_belong_to_the_round_after_them(self):
        lines = rc.read_out('snap/t1 ab\nround 1 ok took_ms=5 late_ms=0 skipped=0\nretired t0\nsnap/t2 cd\n'
                            'round 2 ok took_ms=7 late_ms=0 skipped=0\nforgotten t9 (why so)\n')
        self.assertEqual([(ln['kind'], ln['name'], ln['round']) for ln in lines],
                         [('snap', 't1', 1), ('round', None, 1), ('retired', 't0', 2), ('snap', 't2', 2),
                          ('round', None, 2), ('forgotten', 't9', 3)])
        self.assertEqual(lines[-1]['why'], '(why so)')
        self.assertEqual(lines[4]['took_ms'], 7)

    def test_dump(self):
        d = rc.read_dump('volume ab\npublished t1 commit=ab group=3 members=4 at_ms=1 retire=-\n'
                         'abandoned t2 why=person at_ms=2 left=members:4\nstuck t3 members=4 at_ms=3\n')
        self.assertEqual(d['order'], ['t1'])
        self.assertEqual(d['abandoned']['t2']['left'], 'members:4')
        self.assertEqual(d['stuck']['t3']['members'], '4')
        for bad in ('published t1 commit\n', 'stuck t3 a=1\nstuck t3 a=2\n', 'published\n'):
            with self.assertRaises(RigError):
                rc.read_dump(bad)

    def test_snaps(self):
        self.assertEqual(rc.read_snaps('skzcut@t\t12\n'), {'skzcut@t': '12'})
        for bad in ('skzcut@t 12\n', 'skzcut\t12\n', 'skzcut@t\tx\n'):
            with self.assertRaises(RigError):
                rc.read_snaps(bad)
        self.assertEqual(rc.at({'a@t': '1', 'a/b@t': '2', 'a@tt': '3', 'a@t-1': '4'}, 't'), ['a/b@t', 'a@t'])

    def test_killed(self):
        err = 'x\nskyzfs-agent: test hook --kill-at: killed at destroy-member after (1)\n'
        self.assertTrue(rc.killed(err, 'destroy-member:after:1'))
        self.assertFalse(rc.killed(err, 'destroy-member:after:2'))
        self.assertFalse(rc.killed(err, 'destroy-member:before:1'))

    def test_reports(self):
        self.assertEqual(rc.reports('skyzfs-agent: report t not-retired: tried again at the next round: a b\n'
                                    'skyzfs-agent: report u no-guids: x\n'), [('t', 'a b')])

    def test_the_tables(self):
        self.assertEqual(rc.STEPS_MAX, 8)
        self.assertEqual(rc.CASES, ('K', 'S', 'X-retire', 'X-retain', 'X-destroy', 'B', 'A-old', 'A-nothing',
                                    'A-members'))
        self.assertEqual(rc.X_WANT, {'X-retire': (['retired', 'gone'], 0, 1), 'X-retain': (['retired'], 1, 1),
                                     'X-destroy': (['retired'], 0, 1)})
        src = open(os.path.join(HERE, 'agent-record.sh')).read()
        for cid in rc.CASES:
            self.assertIn(f'rcase {cid} ', src)
        for cid, kill in rc.KILLS.items():
            self.assertIn(f'rcase {cid} ', src)
            self.assertIn(f' {kill}\n', src)


if __name__ == '__main__':
    unittest.main()
