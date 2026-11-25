#!/usr/bin/env python3
"""The channel rig's check (M2.T3, the daemon's publish; design §5.2, "A
snapshot is published by a cut of the pool's present history", and §5.4).

Usage: publish_check.py NAME DIR LOG BASE DEV_BYTES [LOG_DEVICE_IMAGE]

Called by tests/kernel/cut-record.sh as its CUT_CHECK after each of the
publish courses (pplain, prewind, prewindf, pkill, pkillman, pkillpub,
pchannel),
recorded through the daemon with its uploads (CUT_UPLOAD=1). LOG is the
daemon's request log; BASE is `-`. Environment: as nodeb_check.py's
(SKZ_NODEB, SKZ_S3_ENDPOINT, SKZ_S3_BUCKET and the keys). The volume in
object storage is `cut-NAME`.

What the course left in DIR:
- ends.tsv: LABEL and the answer the agent had (publish_agent.py: a
  flow's end, or one request's answer; `none (exit N)` where none came);
- published.tsv: each publish asked that is to be judged as published,
  as publish_agent.py writes it (T, the G asked, A, S, C, the manifest's
  path, the snapshot's createtxg); the last line of a name is the
  request its publish answers;
- agent.tsv: every request and its answer, with the monotonic clock;
- requests.tsv: the daemon's request log (its `cut` lines);
- status.txt and the course's own status files;
- T-between.txt: node B's `snap-row` between a kill and the next
  instance's answer, where the course takes one; kill.tsv, the killed
  daemon's exit status, and serve.log, its stderr, which names the hook
  that killed it (pkillman, pkillpub);
- rewind-row.txt (prewind), tw-row.txt and tw-status.txt (pkill): node
  B's `row` and the daemon's status at the course's premise;
- rewind-top.txt (prewind): the group of the daemon's newest cut before
  the rewind, from `cuts` (the abandoned history's top);
- rewind-f.txt (prewindf): what was zeroed of the newest group's root
  block, and `imported with -F` or `-FX` (on the daemon -F alone is
  refused: the labels' config is of the damaged group);
- channel-status.txt and socket-mode.txt (pchannel): the status after
  the channel's requests, whose counts (COUNTS) must be exact, and the
  socket's mode before the rig widened it for the peer, which must be
  600.

Each answer must be one the table EXPECTED allows for its label, in the
course's own words: a label with an answer outside the table is a
verdict against the daemon (exit 1), and a label the course did not
leave is the rig's error (exit 2). For each name the course published
(PUBLISHED), node B (skyzfs-nodeb snap-row, which decodes the row
itself) must find:

- the ref `snap/T`, on the commit the `done` answer named;
- the row `skyzfs/snap/T` of version 1, on that same commit, naming a
  cut of the state asked, counted after C, of group G or later: a cut
  of a group below G is never what `snap/T` names (the lead's addition
  to P1);
- the ZFS manifest the row names, of the row's length and hash, and the
  bytes the agent sent;
- in the request log, that cut's `cut` line (its number and group) as
  one that opens as itself;
- and the snapshot itself: node B's stages (nodeb_check.Verifier: refs,
  content, import, snapshot, mount, files, copies) on `snap/T`, with
  the agent's manifest as the files and digests to find.

Rule (R) (M2.6a): a request asked with a count from before a take-over
(pkill, pkillman, pkillpub) or from below the top of a rewind's
abandoned groups (prewind) is answered `refused rewound`, and the
courses pin it; asked anew with a count read then, as the agent's
AskAnew asks, it is published. p1 asked anew is refused `taken` (what
landed before the kill was carried by a cut counted before the new
count), and `look p1` then names the commit of snap/p1: for p1 the
`snap` answer of the look stands in for `done` above.

`look` (M2.6a; LOOKS): its `snap` answer for a name published must name
the row node B read (its commit, state, cut, group and manifest), and
for a name never published it must be `absent` (pplain's t5 and t3).
With object storage down it must be `refused store` or `refused
no-answer`, never `absent` (prewind's r1-look).

A publish waiting for its cut when `zpool import -F` goes back past the
newest group (prewindf) is ended by rule (R): its progress reads
`refused rewound`, and f1 is never published.

For each name in ABSENT, node B must find neither the ref nor the row.
A premise (a `premise_` line) that does not hold means the course did
not make the case it is there for: the rig's error, not a verdict.

The premises that make two courses test what they are for (the review
of #139):
- prewind: a cut counted after the first ask's C, of group G or later,
  that opens only as a rewind, was committed after that ask (node B's
  row read after the store came back, DIR/rewind-row.txt, and that
  cut's `cut` line), and r1 was not made by an earlier one. And the
  refusal had its ground in the request log: a cut counted after the
  first C whose group is not above the groups of the instance's cuts
  before it, ahead of the line `r1 refused rewound`. r1 asked anew
  must then be made by a cut above the top (DIR/rewind-top.txt;
  pin_r1_above_top) that opens as itself (pin_r1_cut).
  What the course pins is rule (R) at the ask (pin_answer_r1) and the
  way out past the top. Under rule (R) r1 is refused before the
  rewind-only cut is committed, and no publish can wait across such a
  cut, so the course does not test "opens as itself" with a publish
  waiting: that rests on the unit tests of skyzfs-upload's publish and
  on the cut rig, which judges each cut's opening against the kernel
  (#140's review, N1).
- prewindf: the import was made with -F or -FX after the newest
  group's root block was zeroed (DIR/rewind-f.txt), and the request log ends f1
  `refused rewound` (a `publish ended` line) after a cut counted after
  f1's C whose group is not above the groups of the instance's cuts
  before it.
- pkill: the instance that answered tw's withdrawal then counted cuts
  of tw's G or later that open as itself, and committed one
  (DIR/tw-row.txt): had the withdrawal left tw queued, that commit would
  have made it. Its status then (DIR/tw-status.txt) must count the
  withdrawal and nothing pending or published (a pin on the server).

Controls (pplain only; each must FAIL where it is put, or the run
fails): `ctl_below`, the published row judged against a G one above its
group; `ctl_commit`, against a done commit with its first digit changed;
`ctl_files`, node B's stages against the manifest with one digest
changed, which must be caught at the files.

Each line says what it rests on: `kernel and server` (node B's import of
what the daemon published), `server` (the daemon's answers and what it
put in object storage), `rig` (a premise of the course), or `check`
(a control: only that the check catches what it should).

results.tsv gets the lines (KEY, VERDICT, DETAIL), which cut_summary.py
prints. Exit 0: every line holds. Exit 1: a verdict. Exit 2: the rig's
own error, which is also what any exception ends in.
"""
import os
import shutil
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cut_daemon_check  # noqa: E402
import nodeb_check  # noqa: E402
from cut_check import RigError  # noqa: E402

COURSES = ('pplain', 'prewind', 'prewindf', 'pkill', 'pkillman', 'pkillpub', 'pchannel')

# label -> the answers allowed, each a prefix of the answer line.
EXPECTED = {
    'pplain': {
        't1': ('done ',),
        't1-again': ('accepted',),            # asking twice is asking once
        't1-again-end': ('done ',),
        't1-look': ('snap ',),                # the row of snap/t1 (LOOKS)
        't1-other': ('accepted',),            # another manifest: answered with no I/O
        't1-other-end': ('refused taken',),   # then looked up: snap/t1 names another
        't2': ('done ',),                     # --twice
        'att': ('refused attachment',),
        'state': ('refused state',),
        't4': ('done ',),                     # G asked 20 above the snapshot's
        't5': ('expired',),
        't5-look': ('absent',),               # expired: never made
        't3-look': ('absent',),               # never asked validly
        't7': ('pending after',),
        't7-other': ('refused asked',),       # pending with other parameters
        't7-withdraw': ('withdrawn',),
        't7-again': ('accepted',),            # withdrawn, then asked anew
        't7-end': ('done ',),
    },
    # Rule (R) (M2.6a): a request asked with a count from before a
    # take-over or a rewind is refused `rewound`; asked anew with a count
    # read then, it is published.
    'prewind': {
        'r1': ('refused rewound',),           # the snapshot's and hold's cuts open only as a rewind
        'r1-look': ('refused store', 'refused no-answer'),  # object storage down: never `absent`
        'r1-anew': ('accepted',),             # past the abandoned top, a count read then
        'r1-end': ('done ',),
    },
    # A rewind by `zpool import -F` while f1 waits for its cut: the mark
    # ends it.
    'prewindf': {
        'f1': ('pending after',),
        'f1-end': ('refused rewound',),
    },
    'pkill': {
        'tw': ('pending after',),
        'tw-withdraw': ('withdrawn',),
        'tk': ('pending after',),
        'tw-after': ('withdrawn', 'unknown'),  # never done, with whatever commit
        'tk-after': ('unknown',),             # pending publishes are in memory
        'tk-again': ('refused rewound',),     # the new instance's count at its start is a mark
        'tk-anew': ('accepted',),
        'tk-end': ('done ',),
        'tw-last': ('withdrawn', 'unknown'),
    },
    'pkillman': {
        # The daemon killed itself under the agent: no answer, or the
        # next instance's to its last look (it does not know m1).
        'm1': ('none ', 'unknown'),
        'm1-after': ('unknown',),
        'm1-again': ('refused rewound',),
        'm1-anew': ('accepted',),
        'm1-end': ('done ',),
    },
    'pkillpub': {
        'p1': ('none ', 'unknown'),
        'p1-after': ('unknown',),
        'p1-again': ('refused rewound',),
        'p1-anew': ('accepted',),
        # The lookup finds what landed, on a cut counted before the new
        # count: not this request's, so `taken`; the look reads the row.
        'p1-end': ('refused taken',),
        'p1-look': ('snap ',),
    },
    'pchannel': {
        'nu': ('refused no-upload',),
        'version': ('refused version',),
        'malformed': ('refused malformed',),
        'name': ('refused name',),
        'peer': ('refused peer',),
        'cuts': ('cuts ',),
    },
}
# The names published -> the label whose `done` (or, for p1, `look`'s
# `snap`) names their commit.
PUBLISHED = {
    'pplain': {'t1': 't1', 't2': 't2', 't4': 't4', 't7': 't7-end'},
    'prewind': {'r1': 'r1-end'},
    'prewindf': {},
    'pkill': {'tk': 'tk-end'},
    'pkillman': {'m1': 'm1-end'},
    'pkillpub': {'p1': 'p1-look'},
    'pchannel': {},
}
# Names that must have no ref and no row: an expired publish, one never
# asked validly, one withdrawn before a kill and past whose G the pool's
# cuts went.
ABSENT = {'pplain': ('t5', 't3'), 'prewindf': ('f1',), 'pkill': ('tw',)}
# The names published -> the label of a `look` at them, whose `snap`
# answer must name the row node B read (pin_T_look).
LOOKS = {'pplain': {'t1': 't1-look'}, 'pkillpub': {'p1': 'p1-look'}}
# Between a kill and the next instance's answer (T-between.txt): whether
# snap/T is there. manifest:1 kills before the transaction (a crash
# there leaves no snap/T: the design's atomic visibility, the server's);
# publish:1 after it landed (the hook's premise).
BETWEEN = {'pkillman': ('m1', False), 'pkillpub': ('p1', True)}
# The channel's counts in the status of the daemon that answered them
# (pchannel), exactly: four requests answered (version, malformed, name,
# cuts; the peer's is refused before it is read and is not one of them),
# two of them malformed (another greeting is counted as one; a bad name
# is not), one peer.
COUNTS = {'pchannel': {'control_answered': 4, 'control_peer': 1, 'control_malformed': 2}}
# pkill's status after tw's withdrawal and the commit past its G, before
# tk is asked (DIR/tw-status.txt), exactly.
TW_STATUS = {'publish_withdrawn': 1, 'publish_pending': 0, 'publish_published': 0}
# The hook's words in the killed instance's serve.log, by course.
KILLED_BY = {'pkillman': 'after the framed manifest PUT 1 landed',
             'pkillpub': 'after the transaction of publish 1 (p1) landed'}


def flat(text):
    return ' '.join(str(text).split())


def read_ends(text):
    """ends.tsv -> {label: answer}; a label twice is the rig's error."""
    out = {}
    for ln in text.splitlines():
        if not ln:
            continue
        f = ln.split('\t', 1)
        if len(f) != 2 or not f[0]:
            raise RigError(f'ends.tsv: not LABEL and an answer: {ln!r}')
        if f[0] in out:
            raise RigError(f'ends.tsv: label {f[0]} twice')
        out[f[0]] = f[1]
    return out


def judge_ends(course, ends):
    """-> [(label, ok, answer, allowed)]; a label missing is the rig's error."""
    want = EXPECTED[course]
    missing = [k for k in want if k not in ends]
    if missing:
        raise RigError(f'the course left no answer for {", ".join(missing)}')
    extra = [k for k in ends if k not in want]
    if extra:
        raise RigError(f'answers the table does not know: {", ".join(extra)}')
    return [(k, any(ends[k].startswith(a) for a in want[k]), ends[k], want[k]) for k in want]


def read_published(text, first=False):
    """published.tsv -> {T: the last request asked of it}, or the first
    with `first`."""
    out = {}
    for ln in text.splitlines():
        if not ln:
            continue
        f = ln.split('\t')
        if len(f) != 7 or not (f[1].isdigit() and f[4].isdigit() and f[6].isdigit()) \
                or len(f[2]) != 32 or len(f[3]) != 32:
            raise RigError(f'published.tsv: not T, G, A, S, C, a path and a createtxg: {ln!r}')
        if first and f[0] in out:
            continue
        out[f[0]] = {'name': f[0], 'group': int(f[1]), 'attachment': f[2], 'state': f[3], 'after': int(f[4]),
                     'manifest': f[5], 'txg': int(f[6])}
    return out


def read_snap_row(text):
    """skyzfs-nodeb snap-row's lines -> {'ref': (commit, gen) or None,
    'row': dict, None (no row) or 'unreadable', 'manifest': (len, hash,
    matches) or None (no object) or absent}."""
    got = {}
    for ln in text.splitlines():
        f = ln.split('\t')
        if f[0] == 'ref':
            got['ref'] = None if f[1:] == ['none'] else (f[1], f[2]) if len(f) == 3 else RigError
        elif f[0] == 'snaprow':
            if f[1:] == ['none']:
                got['row'] = None
            elif f[1] == 'unreadable':
                got['row'] = 'unreadable'
            elif len(f) == 8 and f[2].isdigit() and f[3].isdigit() and f[6].isdigit():
                got['row'] = {'state': f[1], 'number': int(f[2]), 'group': int(f[3]), 'commit': f[4],
                              'snap': f[5], 'manifest_len': int(f[6]), 'manifest_hash': f[7]}
            else:
                got['row'] = RigError
        elif f[0] == 'manifest':
            got['manifest'] = None if f[1:] == ['none'] else (int(f[1]), f[2], f[3] == 'matches') \
                if len(f) == 4 and f[1].isdigit() and f[3] in ('matches', 'differs') else RigError
    if RigError in got.values() or 'ref' not in got or 'row' not in got:
        raise RigError(f'skyzfs-nodeb snap-row said what the rig cannot read: {flat(text)[:300]}')
    return got


def done_commit(answer):
    """`done commit=H`, or `look`'s `snap commit=H ...` -> H; anything
    else -> None."""
    f = answer.split()
    if ((len(f) == 2 and f[0] == 'done') or (len(f) == 7 and f[0] == 'snap')) and f[1].startswith('commit='):
        return f[1][len('commit='):]
    return None


def judge_look(answer, row):
    """`look`'s `snap commit=H state=S number=N group=G manifest-len=L
    manifest-hash=X` against the row node B read -> [what differs]
    (#140's review, N6): its commit, state, cut, group and manifest."""
    f = dict(w.split('=', 1) for w in answer.split()[1:] if '=' in w)
    if not isinstance(row, dict):
        return ['node B read no row to compare the look with']
    want = {'commit': row['commit'], 'state': row['state'], 'number': str(row['number']), 'group': str(row['group']),
            'manifest-len': str(row['manifest_len']), 'manifest-hash': row['manifest_hash']}
    return [f'the look says {k} {f.get(k, "nothing")}, the row {v}' for k, v in want.items() if f.get(k) != v]


def other_commit(commit):
    """ctl_commit's commit: COMMIT with its first digit changed, so
    that the line's short ids (12 digits) differ as the commits do."""
    return ('0' if commit[0] != '0' else '1') + commit[1:]


def judge_row(got, req, commit):
    """What node B found of snap/T against the request asked and the
    commit `done` named -> [what is wrong] (empty when it holds)."""
    bad = []
    ref, row = got.get('ref'), got.get('row')
    if ref is None:
        bad.append('no ref snap/T')
    elif ref[0] != commit:
        bad.append(f'the ref is on {ref[0][:12]}, done named {str(commit)[:12]}')
    if row is None:
        bad.append('no row skyzfs/snap/T')
        return bad
    if row == 'unreadable':
        bad.append('a row that is not of version 1')
        return bad
    if ref is not None and row['commit'] != ref[0]:
        bad.append(f"the row names commit {row['commit'][:12]}, the ref {ref[0][:12]}")
    if row['state'] != req['state']:
        bad.append(f"the row's cut state {row['state']} is not the one asked, {req['state']}")
    if row['number'] <= req['after']:
        bad.append(f"the row's cut {row['number']} was not counted after C = {req['after']}")
    if row['group'] < req['group']:
        bad.append(f"the row's cut is of group {row['group']}, below G = {req['group']}")
    return bad


def judge_manifest(got, sent):
    """-> [what is wrong] with the ZFS manifest node B fetched (bytes in
    `fetched`, None where not fetched) against the agent's `sent`."""
    m = got.get('manifest', 'absent')
    if m == 'absent':
        return ['node B did not fetch the manifest']
    if m is None:
        return ['no manifest object where the row says']
    bad = []
    if not m[2]:
        bad.append(f'the manifest object ({m[0]} bytes) is not the length and hash the row names')
    if got.get('fetched') is not None and got['fetched'] != sent:
        bad.append(f"the manifest object is not the bytes the agent sent ({len(got['fetched'])} and {len(sent)})")
    return bad


def cut_lines(instances):
    """The request log's `cut` lines -> [(number, group, itself, pid, line)]."""
    out = []
    for inst in instances:
        for ln in inst['lines']:
            if ln['event'] == 'cut':
                if not (ln['result'].isdigit() and ln['sectors'].isdigit() and ln['sector'] in ('itself', 'rewind')):
                    raise RigError(f"request log line {ln['line']}: a cut line that does not read")
                out.append((int(ln['sectors']), int(ln['result']), ln['sector'] == 'itself', inst['pid'], ln['line']))
    return out


def rewind_committed(cuts, cut, req, named):
    """prewind's premise (B2): CUT, the (state, number, group) node B's
    row named once the store was back, must be of the state asked,
    counted after C, of group G or later, and one whose `cut` line says
    it opens only as a rewind; NAMED, the number of the cut that made
    r1, must not be an earlier one that opens as itself (then CUT was
    never offered to the publish). r1 made by CUT, or by an earlier cut
    that opens only as a rewind, is no premise's failure: it is the fault
    the course is there to catch, and the row's pin (judge_cut) says so
    as a verdict, which a premise after it must not turn into the rig's
    error (#139's second look, D1) -> [what does not hold]."""
    if cut is None:
        return ['node B\'s row named no cut after the store came back']
    state, number, group = cut
    bad = []
    if state != req['state']:
        bad.append(f'the row\'s cut is of state {state}, not the one asked')
    if number <= req['after']:
        bad.append(f"the row's cut {number} is not counted after C = {req['after']}")
    if group < req['group']:
        bad.append(f"the row's cut is of group {group}, below G = {req['group']}")
    same = [c for c in cuts if c[0] == number and c[1] == group]
    if not same:
        bad.append(f'no cut line of number {number} and group {group} in the request log')
    elif any(c[2] for c in same):
        bad.append(f'cut {number} (group {group}) opens as itself: no daemon would pass it over')
    if named is not None and named < number and any(c[0] == named and c[2] for c in cuts):
        bad.append(f'r1 was made by cut {named}, which opens as itself, before cut {number} was committed')
    return bad


def withdrawn_past(instances, name, group):
    """pkill's premise (B3): the instance that answered NAME's withdrawal,
    and the cuts it counted after that line, of group GROUP or later,
    that open as itself -> (pid or None, [(number, group)])."""
    for inst in instances:
        for ln in inst['lines']:
            if ln['event'] == 'publish' and ln['op'] == 'withdraw' and ln['result'] == f'{name} withdrawn':
                return inst['pid'], [(int(c['sectors']), int(c['result'])) for c in inst['lines']
                                     if c['event'] == 'cut' and c['line'] > ln['line'] and c['sector'] == 'itself'
                                     and c['result'].isdigit() and int(c['result']) >= group]
    return None, []


def rewound_grounds(instances, name, after, op='asked'):
    """The premise of prewind and prewindf (rule R): the cuts counted
    after AFTER, ahead of the request log's `NAME refused rewound` line
    of OP (`asked`: the refusal of the request; `ended`: a publish ended
    while it waited), whose group is not above the highest group of
    that instance's cut lines before them -> (whether that line was
    found, [(number, group, the highest before)])."""
    for inst in instances:
        top, seen = None, []
        for ln in inst['lines']:
            if ln['event'] == 'cut' and ln['result'].isdigit() and ln['sectors'].isdigit():
                number, group = int(ln['sectors']), int(ln['result'])
                if top is not None and number > after and group <= top:
                    seen.append((number, group, top))
                top = group if top is None else max(top, group)
            elif ln['event'] == 'publish' and ln['op'] == op and \
                    ln['result'].startswith(f'{name} refused rewound'):
                return True, seen
    return False, []


def exact_counts(status, want):
    """-> [what differs]: each key of WANT in STATUS, exactly."""
    return [f'{k} is {status.get(k) or "missing"}, not {v}' for k, v in want.items() if status.get(k) != str(v)]


def judge_cut(cuts, row):
    """-> [what is wrong]: the row's cut must be a cut the daemon counted,
    of that group, and one that opens as itself."""
    same = [c for c in cuts if c[0] == row['number']]
    if not same:
        return [f"no cut line of number {row['number']} in the request log"]
    if not any(c[1] == row['group'] for c in same):
        return [f"cut {row['number']} is of group {', '.join(str(c[1]) for c in same)} in the request log, "
                f"the row says {row['group']}"]
    if not any(c[1] == row['group'] and c[2] for c in same):
        return [f"cut {row['number']} (group {row['group']}) opens only with a rewind"]
    return []


def counted_between(cuts, after, lo, hi):
    """Cuts counted after C whose group is in [lo, hi): what a publish
    of G = hi must pass over (pplain's t4)."""
    return [c for c in cuts if c[0] > after and lo <= c[1] < hi]


def status_of(text):
    out = {}
    for ln in text.splitlines():
        k, _, v = ln.partition(' ')
        out.setdefault(k, v)
    return out


def main():
    if len(sys.argv) not in (6, 7):
        sys.exit(__doc__)
    name, rdir, log = sys.argv[1:4]
    t0 = time.monotonic()
    lines = []

    def put(key, verdict, rests, detail):
        lines.append(f'{key}\t{verdict}\t{flat(f"[{rests}] {detail}")}')
        print(f'{name}: {key}: {verdict}: [{rests}] {flat(detail)}', flush=True)

    rc = 0

    def holds(key, bad, rests, good):
        nonlocal rc
        if bad:
            rc = max(rc, 1)
            put(key, 'FAILED', rests, '; '.join(bad))
        else:
            put(key, 'ok', rests, good)

    def premise(key, ok, detail):
        put(key, 'ok' if ok else 'ERROR', 'rig', detail)
        if not ok:
            raise RigError(f'{key}: {detail}')

    def text_of(fn, need=True):
        p = os.path.join(rdir, fn)
        if not os.path.exists(p):
            if need:
                raise RigError(f'no {fn}')
            return ''
        with open(p, errors='replace') as f:
            return f.read()

    try:
        if name not in COURSES:
            raise RigError(f'not a publish course: {name}')
        nb = nodeb_check.NodeB(rdir, None)
        vol = f'cut-{name}'
        ends = read_ends(text_of('ends.tsv'))
        for label, ok, answer, allowed in judge_ends(name, ends):
            holds(f'pin_answer_{label}', [] if ok else [f'answered {answer!r}, not {" or ".join(allowed)}'],
                  'server', f'answered {answer!r}')
        published = read_published(text_of('published.tsv', need=bool(PUBLISHED[name])))
        torn = []
        with open(log, errors='replace') as f:
            instances = cut_daemon_check.parse_log(f.read(), torn)
        cuts = cut_lines(instances)
        put('obs_cuts', 'INFO', 'server', f'{len(cuts)} cut lines in {len(instances)} instances'
            + (f', {len(torn)} line(s) cut short by a kill' if torn else ''))

        def snap_row(t, out=None):
            args = ['snap-row', '--volume', vol, '--snapshot', t] + (['--out', out] if out else [])
            rc_, text = nb.tool_run(args)
            if rc_:
                raise RigError(f'skyzfs-nodeb snap-row {t}: exit {rc_}: {flat(text)[-300:]}')
            got = read_snap_row(text)
            if out and os.path.exists(out):
                with open(out, 'rb') as f:
                    got['fetched'] = f.read()
            return got

        controlled = False
        for t, label in PUBLISHED[name].items():
            req = published.get(t)
            if req is None:
                raise RigError(f'no request of {t} in published.tsv')
            commit = done_commit(ends[label])
            if commit is None:
                put(f'pin_{t}', 'FAILED', 'server', f'not published: {label} answered {ends[label]!r}')
                rc = max(rc, 1)
                continue
            with open(req['manifest'], 'rb') as f:
                sent = f.read()
            got = snap_row(t, os.path.join(nb.dir, f'manifest-{t}.got'))
            bad = judge_row(got, req, commit)
            row = got.get('row')
            holds(f'pin_{t}_row', bad, 'server',
                  f"snap/{t} and its row on {commit[:12]}: cut {row['number']} of group {row['group']} "
                  f"(asked after {req['after']}, G {req['group']}, the snapshot's createtxg {req['txg']})"
                  if not bad else '')
            if isinstance(row, dict):
                holds(f'pin_{t}_manifest', judge_manifest(got, sent), 'server',
                      f'{len(sent)} bytes, the row\'s length and hash, the bytes sent')
                holds(f'pin_{t}_cut', judge_cut(cuts, row), 'server',
                      f"cut {row['number']} of group {row['group']} opens as itself in the request log")
            look = LOOKS.get(name, {}).get(t)
            if look is not None:
                holds(f'pin_{t}_look', judge_look(ends[look], row), 'server',
                      f'{look} names the row node B read: its commit, state, cut, group and manifest')
            if name == 'pplain' and t == 't1':
                again = done_commit(ends['t1-again-end'])
                holds('pin_asking_twice_is_once', [] if again == commit else
                      [f't1 asked again was done on {str(again)[:12]}, the first on {commit[:12]}'], 'server',
                      'asked again with the same request: done on the same commit')
            if name == 'pplain' and t == 't4':
                below = counted_between(cuts, req['after'], req['txg'], req['group'])
                premise('premise_t4_cuts_below_g', bool(below),
                        f"{len(below)} cuts counted after C = {req['after']} of groups {req['txg']} to "
                        f"{req['group'] - 1}, below the G asked: the publish passed over them")
            want = nodeb_check.read_sums(sent.decode(errors='replace'))
            if not want:
                raise RigError(f'the manifest of {t} lists no file')
            seen = nb.verify(vol, f'snap/{t}', t, want, damaged=False)
            failed = seen.failed()
            holds(f'pin_{t}_nodeb', [f'node B failed at {failed}: {seen.text()}'] if failed else [],
                  'kernel and server', seen.text())
            if name == 'pplain' and not controlled and isinstance(row, dict):
                controlled = True
                for key, bad_ in (('ctl_below', judge_row(got, dict(req, group=row['group'] + 1), commit)),
                                  ('ctl_commit', judge_row(got, req, other_commit(commit)))):
                    put(key, 'CAUGHT' if bad_ else 'MISSED', 'check', '; '.join(bad_) or 'the pin held')
                    rc = max(rc, 0 if bad_ else 1)
                wrong = dict(want)
                k0 = sorted(wrong)[0]
                wrong[k0] = ('0' if wrong[k0][0] != '0' else '1') + wrong[k0][1:]
                cseen = nb.verify(vol, f'snap/{t}', t, wrong, damaged=False)
                verdict, ok, _ = nodeb_check.control_verdict(cseen, ('files',))
                put('ctl_files', verdict.upper().replace(' ', '_'), 'check',
                    f'{k0} with another digest: {cseen.text()}{"" if ok else "; DIFFERS from caught at files"}')
                rc = max(rc, 0 if ok else 1)
        for t in ABSENT.get(name, ()):
            got = snap_row(t)
            holds(f'pin_{t}_absent', [w for w, there in (('a ref snap/T', got['ref'] is not None),
                                                          ('a row', got['row'] is not None)) if there],
                  'server', f'no snap/{t} and no row')
        if name == 'pkill':
            tw, tk = published.get('tw'), published.get('tk')
            if tw is None or tk is None:
                raise RigError('no request of tw or tk in published.tsv')
            pid, past = withdrawn_past(instances, 'tw', tw['group'])
            row = cut_daemon_check.read_row(text_of('tw-row.txt'), 'tw-row.txt')['cut']
            committed = row is not None and row[0] == tw['state'] and row[1] > tw['after'] and \
                (row[1], row[2]) in past
            premise('premise_tw_committed_past_g', committed,
                    f"instance {pid or 'none'} answered tw's withdrawal, then counted {len(past)} cuts of group "
                    f"{tw['group']} or later that open as itself, and node B's row named cut "
                    f"{row[1] if row else 'none'} of group {row[2] if row else 'none'}"
                    + (': a commit that would have made tw' if committed else ', not one of them'))
            holds('pin_tw_withdrawn_status', exact_counts(status_of(text_of('tw-status.txt')), TW_STATUS),
                  'server', 'after the withdrawal and that commit: ' +
                  ', '.join(f'{k} {v}' for k, v in TW_STATUS.items()))
        if name == 'prewind':
            # The premise is of the first ask, refused `rewound`; the
            # request judged is the one asked anew.
            r1 = read_published(text_of('published.tsv'), first=True).get('r1')
            if r1 is None:
                raise RigError('no request of r1 in published.tsv')
            if published['r1'] == r1:
                raise RigError('r1 was not asked anew: its last request in published.tsv is its first')
            found, grounds = rewound_grounds(instances, 'r1', r1['after'])
            premise('premise_r1_rewound_with_a_rewind_cut', found and bool(grounds),
                    'no `r1 refused rewound` line in the request log' if not found else
                    f"cuts counted after C = {r1['after']} before the refusal, not above the groups before them: "
                    + (', '.join(f'{n} of group {g} (the top before it {t})' for n, g, t in grounds) or 'none'))
            held = status_of(text_of('rewind-status.txt')).get('cuts_held', '')
            premise('premise_held_at_rewind', held.isdigit() and int(held) > 0,
                    f'cuts held with object storage down, at the rewind: {held or "none"}')
            cut = cut_daemon_check.read_row(text_of('rewind-row.txt'), 'rewind-row.txt')['cut']
            got = snap_row('r1')
            named = got['row']['number'] if isinstance(got.get('row'), dict) else None
            top = text_of('rewind-top.txt').strip()
            if not top.isdigit():
                raise RigError(f'rewind-top.txt: not a group: {top!r}')
            group = got['row']['group'] if isinstance(got.get('row'), dict) else None
            holds('pin_r1_above_top', [] if group is not None and group > int(top) else
                  [f"r1's cut is of group {group if group is not None else 'none'}, not above the top {top}"],
                  'server', f"r1 asked anew was made by a cut of group {group}, above the abandoned history's "
                  f"top {top}")
            bad = rewind_committed(cuts, cut, r1, named)
            premise('premise_rewind_cut_committed', not bad,
                    '; '.join(bad) if bad else
                    f"cut {cut[1]} of group {cut[2]}, counted after C = {r1['after']} at G = {r1['group']} or "
                    f"later, opens only as a rewind and was committed after the ask; r1 was made by cut {named}")
        if name == 'prewindf':
            f1 = published.get('f1')
            if f1 is None:
                raise RigError('no request of f1 in published.tsv')
            damage = text_of('rewind-f.txt')
            zeroed = damage.split()[1] if damage.startswith('zeroed ') and len(damage.split()) > 1 else ''
            premise('premise_f1_import_f', zeroed.isdigit() and int(zeroed) > 0 and
                    any(ln in ('imported with -F', 'imported with -FX') for ln in damage.splitlines()),
                    flat(damage)[:300] or 'rewind-f.txt is empty')
            found, grounds = rewound_grounds(instances, 'f1', f1['after'], 'ended')
            premise('premise_f1_ended_by_a_rewind_cut', found and bool(grounds),
                    'no `f1 refused rewound` end in the request log' if not found else
                    f"cuts counted after C = {f1['after']} before f1 was ended, not above the groups before them: "
                    + (', '.join(f'{n} of group {g} (the top before it {t})' for n, g, t in grounds) or 'none'))
        if name in BETWEEN:
            t, there = BETWEEN[name]
            kill = text_of('kill.tsv').strip()
            serve = text_of('serve.log', need=False)
            premise(f'premise_{t}_killed_by_the_hook', kill == 'killed_rc\t137' and KILLED_BY[name] in serve,
                    f"{flat(kill)}; serve.log {'has' if KILLED_BY[name] in serve else 'lacks'} "
                    f"'{KILLED_BY[name]}'")
            got = read_snap_row(text_of(f'{t}-between.txt'))
            is_there = got['ref'] is not None or got['row'] is not None
            if there:
                premise(f'premise_{t}_landed', is_there,
                        f'snap/{t} {"was" if is_there else "was not"} there after the kill (the hook after the '
                        'transaction)')
                end = done_commit(ends.get(PUBLISHED[name][t], ''))
                holds(f'pin_{t}_found', [] if got['ref'] is not None and got['ref'][0] == end else
                      [f"done names {str(end)[:12]}, the ref after the kill {got['ref'][0][:12] if got['ref'] else 'none'}"],
                      'server', 'asked anew, read on the commit that landed before the kill: found, not made anew')
            else:
                holds(f'pin_{t}_not_before_the_transaction', ['snap/T or its row after a kill before the '
                                                              'transaction'] if is_there else [], 'server',
                      f'no snap/{t} after a kill between the manifest\'s PUT and the transaction')
        if name in COUNTS:
            st = status_of(text_of('channel-status.txt'))
            for k, want in COUNTS[name].items():
                holds(f'pin_status_{k}', exact_counts(st, {k: want}), 'server', f'{k} {want}')
            mode = text_of('socket-mode.txt').strip()
            holds('pin_socket_mode', [] if mode == '600' else [f'the socket\'s mode is {mode or "unknown"}'],
                  'server', 'the socket is 0600 as the daemon made it')
    except RigError as e:
        put('rig_error', 'ERROR', 'rig', str(e))
        rc = 2
    except Exception as e:  # noqa: BLE001 -- anything else is the rig's too, not a verdict
        put('rig_error', 'ERROR', 'rig', f'{type(e).__name__}: {e}')
        rc = 2
    put('time', 'INFO', 'rig', f'{time.monotonic() - t0:.0f} s')
    with open(os.path.join(rdir, 'results.tsv'), 'a') as f:
        f.write(''.join(ln + '\n' for ln in lines))
    img = os.path.join(rdir, 'nodeb', 'img')
    if os.path.isdir(img):
        shutil.rmtree(img)
    sys.exit(rc)


if __name__ == '__main__':
    main()
