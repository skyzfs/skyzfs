import os, shutil, subprocess, sys, tempfile
SRC = sys.argv[1]  # tests/kernel dir of the worktree
TARGET = 'agent_retain_check.py'
M = [
 ('due_retired counts the last round', "if ln['round'] < last]", "if ln['round'] <= last]"),
 ('due_retired keeps one more', "seen[:max(0, len(seen) - keep)]", "seen[:max(0, len(seen) - keep - 1)]"),
 ('judge_keep < not <=', "elif ln['round'] <= when[names[i + keep]]:", "elif ln['round'] < when[names[i + keep]]:"),
 ('judge_keep fewer-after branch dropped', "if i + keep >= len(names):\n            bad.append", "if False:\n            bad.append"),
 ('judge_in_round +2', "due = when[names[names.index(t) + keep]] + 1", "due = when[names[names.index(t) + keep]] + 2"),
 ('judge_in_round no report exemption', "if t not in when or t in reported or", "if t not in when or"),
 ('judge_gone allowed ignored', "ok = set(allowed)", "ok = set()"),
 ('judge_gone allowed by name only', "left = [s for s in at(snaps, t) if (s, snaps[s]) not in ok]", "left = [s for s in at(snaps, t) if s not in {a for a, _ in ok}]"),
 ('judge_gone no ref check', "elif got['ref'] is not None or got['row'] is not None:", "elif got['row'] is not None:"),
 ('judge_gone no record check', "for kind in ('published', 'abandoned', 'stuck'):\n            if t in dump[kind]:", "for kind in ():\n            if t in dump[kind]:"),
 ('judge_kept no commit check', "if p.get('commit') != printed[t]:", "if False:"),
 ('judge_kept no retire check', "if p.get('retire') != '-':", "if False:"),
 ('judge_kept no pool@t check', "if f'{POOL}@{t}' not in snaps:", "if False:"),
 ('judge_kept no ref commit', "elif got['ref'] is None or got['ref'][0] != printed[t]:", "elif got['ref'] is None:"),
 ('K record: stuck/abandoned unchecked', "!= sorted(set(names) - set(want)) or d['stuck'] or d['abandoned']:", "!= sorted(set(names) - set(want)):"),
 ('K record: published unchecked', "if sorted(d['published']) != sorted(set(names) - set(want)) or", "if False or"),
 ('K others: guid unchecked', "if k['snaps'].get(s) != g]", "if s not in k['snaps']]"),
 ('K others premise: rounds unchecked', "obad = [] if c.text('others.rounds').strip() == '1' else", "obad = [] if True else"),
 ('retired_once ignores order', "return [] if seen == list(want) else", "return [] if sorted(seen) == sorted(want) else"),
 ('retired_once ignores duplicates', "return [] if seen == list(want) else", "return [] if list(dict.fromkeys(seen)) == list(want) else"),
 ('retire_log ignores order', "if seen != list(want):", "if sorted(seen) != sorted(want):"),
 ('retire_log ignores commit', "if fields.get('commit') != printed.get(t):", "if False:"),
 ('S children: grandchild may stay', "f'{POOL}/c1/g@{t1}': False}", "f'{POOL}/c1/g@{t1}': None}"),
 ('S children: root may go', "want = {f'{POOL}@{t1}': True,", "want = {f'{POOL}@{t1}x': True,"),
 ('S said: dedup unchecked', "bad += [f'a report of {t1} said twice running: {a}' for a, b in zip(said, said[1:]) if a == b]", "pass"),
 ('S said: first report text unchecked', "bad = [] if said and f'zfs destroy {POOL}/c1@{t1}' in said[0] else", "bad = [] if said else"),
 ('S stuck: published allowed', "if t1 not in sd['stuck'] or t1 in sd['published']:", "if t1 not in sd['stuck']:"),
 ('S stuck: node B unchecked', "if row['ref'] is not None or row['row'] is not None:", "if False:"),
 ('S others_go_on +1', "if r2 and r2[0]['round'] <= rel else", "if r2 and r2[0]['round'] <= rel + 1 else"),
 ('S released +3', "rel < r1[0]['round'] <= rel + 2 else", "rel < r1[0]['round'] <= rel + 3 else"),
 ('S released may be before', "rel < r1[0]['round'] <= rel + 2 else", "0 < r1[0]['round'] <= rel + 2 else"),
 ('S released twice ok', "if len(r1) == 1 and rel <", "if len(r1) >= 1 and rel <"),
 ('S request_log unfiltered want', "[(t1, 'retired')],\n", "[(t1, 'retired'), (t1, 'gone')],\n"),
 ('X_WANT X-retain said 0/1', "'X-retain': (['retired'], 1, 1)", "'X-retain': (['retired'], 0, 1)"),
 ('X said counts run2 only', "s1 = len(named(k['out'].get('publish', []), 'retired', t1))", "s1 = 0 if c.cid != 'X-retain' else 1"),
 ('X-destroy: nothing gone wanted', "'X-destroy': (f'{POOL}/c1/g@{t1}',)}[cid]", "'X-destroy': ()}[cid]"),
 ('X kill side ignored', "return any(m.groups() == (effect, side, n)", "return any(m.groups()[0::2] == (effect, n)"),
 ('B steps: 9 a round allowed', "for r, n in per if n > STEPS_MAX]", "for r, n in per if n > STEPS_MAX + 1]"),
 ('B steps: first round unchecked', "if got != due:", "if False:"),
 ('B kept_60 unchecked', "named(k['out']['publish'], 'retired')]", "[]]"),
 ('A-old: same round ok', "if len(r) == 1 and r[0]['round'] > ab[0]['round'] else", "if len(r) == 1 and r[0]['round'] >= ab[0]['round'] else"),
 ('A-old: twice ok', "if len(r) == 1 and r[0]['round'] > ab[0]['round'] else", "if len(r) >= 1 and r[0]['round'] > ab[0]['round'] else"),
 ('A-old: left unchecked', "if g.get('why') == 'too-old' and re.fullmatch(r'members:[1-9][0-9]*', left) else", "if True else"),
 ('A-nothing: lines unchecked', "bad = [f\"the run said: {ln['kind']} {t}\" for ln in run if ln['name'] == t]", "bad = []"),
 ('A-nothing: pool @t unchecked', "bad += [f'{s} is on the pool' for s in at(k['snaps'], t)]", "pass"),
 ('A-members: twice ok', "bad = [] if len(r) == 1 else [f'`retired {t}` said {len(r)} times']", "bad = [] if len(r) >= 1 else [f'`retired {t}` said {len(r)} times']"),
 ('A-members: record unchecked', "bad += [f'the record keeps {t} among the names {kind}' for kind in inrec]\n        j.holds('pin_A-members_retired'", "j.holds('pin_A-members_retired'"),
 ('status: served-again ignored', "+ (['the daemon was served again in the case'] if 'the daemon served again' in k['recovered']", "+ (['the daemon was served again in the case'] if False"),
 ('status: commit unchecked', "if t in k['printed'] and fields.get('commit') != k['printed'][t]]", "if False]"),
 ('status: counted unchecked', "for key in STATUS_KEYS if counted[key] != logged[key]]", "for key in STATUS_KEYS if False]"),
 ('holds: any tag', "if len(h) >= 2 and h[1] == HOLD_TAG]", "if len(h) >= 2]"),
 ('holds: none', "if len(h) >= 2 and h[1] == HOLD_TAG]", "if False]"),
 ('verdicts.tsv ignored', "j.holds(f'pin_{cid}_{pin}', [what], 'server', '')", "pass"),
 ('rig-error.tsv ignored', "j.rc = max(j.rc, 2)\n        j.put('rig_error_in_the_course'", "j.put('rig_error_in_the_course'"),
 ('case rig-error ignored', "if cerr:\n", "if False:\n"),
 ('watcher not judged', "agent_fail_check.judge_watcher(text_of('watch.log', need=False), j)", "pass"),
 ('read_out round starts 0', "lines, k = [], 1", "lines, k = [], 0"),
 ('read_dump allows dup', "or f[1] in d[f[0]]:", ":"),
 ('control: MISSED not failing', "self.failed = True\n            self.put(key, 'MISSED'", "self.put(key, 'MISSED'"),
 ('main: exception is exit 1', "j.rc = 2\n        j.put('rig_error', 'ERROR', 'rig', f'{type(e).__name__}: {e}')", "j.failed = True\n        j.put('rig_error', 'ERROR', 'rig', f'{type(e).__name__}: {e}')"),
 ('ctl_K_keep_3 judged at 2', "caught = judge_keep(k['lines'], 3)", "caught = judge_keep(k['lines'], 2) or ['x']"),
 ('premise_B_backlog never', "if j.premise('premise_B_backlog', [f'{len(k[\"reports\"])} not-retired reports in the case'] if k['reports'] else [],", "if j.premise('premise_B_backlog', [],"),
]
res = []
for i, (name, old, new) in enumerate(M):
    d = tempfile.mkdtemp(prefix='mut')
    shutil.copytree(SRC, os.path.join(d, 'k'))
    p = os.path.join(d, 'k', TARGET)
    s = open(p).read()
    n = s.count(old)
    if n != 1:
        res.append((name, f'PLANT-ERROR count={n}'))
        shutil.rmtree(d); continue
    open(p, 'w').write(s.replace(old, new))
    r = subprocess.run([sys.executable, '-I', os.path.join(d, 'k', 'test_agent_retain_check.py')], capture_output=True, text=True, timeout=300)
    tail = r.stderr.strip().splitlines()
    fails = [ln for ln in tail if ln.startswith(('FAIL:', 'ERROR:'))]
    res.append((name, ('caught ' + ' | '.join(x.split(' ')[1] for x in fails[:3])) if r.returncode else 'MISSED'))
    shutil.rmtree(d)
for name, v in res:
    print(f'{v[:6]:7} {name}: {v}')
