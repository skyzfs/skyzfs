#!/usr/bin/env python3
"""#119 at cfe8146, the next look: plants in what changed since 32d8d67
(the watcher's last look and its failure counts, node B's --expect-row,
the look before another writer's act, the balance's start, the blocks
written before a failure, the lease node B reads, ulease's rows, the
plan of the jobs), and the plants that survived at 32d8d67 once more.

    plants2.py CHECKOUT OUT [PREFIX ...]

The harness is plants.py's (same directory): one plant at a time, the
file restored after each, a limit on every test file, KILLED / SURVIVED /
NOT APPLIED appended to OUT as they come. An earlier survivor whose text
the head no longer has says NOT APPLIED: the code it changed was
rewritten, and the new plants (W3x, J4x and so on) take its place.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import plants  # noqa: E402

W, J, R, N = plants.W, plants.J, plants.R, plants.N
P = 'tests/kernel/cut_plan.py'
plants.TESTS[P] = [('py', 'tests/kernel/test_cut_plan.py')]

EARLIER = ('W02a', 'W02b', 'W04', 'W05', 'W11', 'W12', 'W17', 'W17b', 'W18', 'W19', 'W20', 'W22',
           'J04', 'J07', 'J08', 'J16b', 'J17h', 'J17j', 'J17k', 'J17l', 'J17m', 'J19d', 'J19j',
           'J20d', 'J20e', 'J21', 'J21d', 'J21e', 'J22d', 'J22e', 'J24e', 'J26', 'J26c', 'J26d', 'J26g', 'J26i',
           'J27', 'J28', 'N03', 'N07', 'N08', 'N09', 'N10', 'N11')

REST = ("    return (st.get('upload_state') == 'idle' and st.get('cuts_held') == '0') or \\\n"
        "        all(st.get(k) == '0' for k in BALANCE)\n")
MOVED = "        moved = [r for r in before if r[1] != before[0][1] or r[2] != 'none']\n"
HELD = "image.get(at + k) == hashlib.sha256(raw_block(k)).digest()[:8].hex()"
PATHS = ('crates/skyzfs-upload/', 'bins/skyzfsd/', 'crates/skyzfs-local/', 'crates/skyzfs-ublk/',
         'bins/skyzfs-nodeb/', 'tests/kernel/cut_watch.py', 'tests/kernel/cut-record.sh',
         'tests/kernel/cut_daemon_check.py', '.github/workflows/skyzfs-cut-daemon.yml')
TOUCHES = "c == p or (p.endswith('/') and c.startswith(p))"
LOOK = ("            awk -F '\\t' -v n=\"$n\" -v g=\"$g\" -v s=\"$st\" -v h=\"$h\" "
        "'$1 == n && $2 == g && $3 == s && $4 == h { f = 1 }\n")

NEW = [
    # ---- the watcher: failures by command, and the last look (D2)
    ('W30 a command that works forgets every command\'s failures', W,
     "        self.failures[cmd] = 0\n        return out\n", "        self.failures = {}\n        return out\n"),
    ('W31 failures never end the watcher', W, "            if self.failures[cmd] >= TRIES:\n", "            if False:\n"),
    ('W32 a last look that never settles is no error', W,
     "        raise RigError(f'the last look did not fetch what head holds in {TRIES} tries')\n", "        return\n"),
    ('W34 a cut fetched before does not settle the look', W,
     "        if key == self.last:\n            self.settled = True\n            return False\n",
     "        if key == self.last:\n            return False\n"),
    ('W35 a row that names no cut does not settle the look', W,
     "        if row_cut(before['row']) is None or before['head'] == 'none':\n            self.settled = True\n",
     "        if row_cut(before['row']) is None or before['head'] == 'none':\n"),
    ('W36 a look stays settled once one was', W,
     "        self.settled = False\n        before = self.row()\n", "        before = self.row()\n"),
    ('W38 main: one look at the stop, whatever it found', W,
     "        w.finish()\n        w.log('stopped')\n", "        w.step()\n        w.log('stopped')\n"),

    # ---- the judge: the balance's start (D3)
    ('J40 any look is at rest', J, REST, "    return True\n"),
    ('J41 at rest with cuts held', J, "(st.get('upload_state') == 'idle' and st.get('cuts_held') == '0')",
     "(st.get('upload_state') == 'idle')"),
    ('J42 at rest whatever the uploader\'s state, with nothing held', J,
     "(st.get('upload_state') == 'idle' and st.get('cuts_held') == '0')", "(st.get('cuts_held') == '0')"),
    ('J43 a start with nothing counted yet is not at rest', J,
     " or \\\n        all(st.get(k) == '0' for k in BALANCE)\n", "\n"),
    ('J44 the balance always starts from the instance\'s start', J,
     "    fn = 'balance-start.txt' if os.path.exists(os.path.join(rdir, 'balance-start.txt')) "
     "else f'status-start-{k}.txt'\n", "    fn = f'status-start-{k}.txt'\n"),
    ('J45 a later instance with no look at rest is balanced from its start', J,
     "    if k > 1 and fn != 'balance-start.txt':\n", "    if False:\n"),
    ('J46 a start look not at rest does not set the exit', J,
     "cuts_held {start.get('cuts_held')!r}\")\n        errored = True\n",
     "cuts_held {start.get('cuts_held')!r}\")\n"),
    # ---- the judge: the blocks written before a failure (D4)
    ('J47 upload_raw passes whatever node B holds', J, "        if held == n:\n", "        if held >= 0:\n"),
    ('J48 upload_raw: missing blocks with the content line passed are a verdict, with it failed the rig\'s error', J,
     "        elif ok_content:\n", "        elif not ok_content:\n"),
    ('J49 upload_raw: missing blocks with the content line failed say PASS', J,
     "            put('upload_raw', 'FAIL', f\"node B's last cut holds",
     "            put('upload_raw', 'PASS', f\"node B's last cut holds"),
    ('J50 raw_held reads the first fetch alone', J,
     "        if len(x) == 4:\n            image[int(x[2])] = x[3]\n",
     "        if len(x) == 4 and x[0] == '0':\n            image[int(x[2])] = x[3]\n"),
    ('J51 raw_held counts any content at the blocks', J, HELD, "image.get(at + k) is not None"),
    ('J52 raw_held compares every block with block 0\'s content', J, HELD,
     "image.get(at + k) == hashlib.sha256(raw_block(0)).digest()[:8].hex()"),
    # ---- the judge: the lease node B reads (G5)
    ('J54 the lease is the first the rows named', J, "    lease = rows[-1][3].split()\n",
     "    lease = rows[0][3].split()\n"),
    ('J55 a bucket with no lease is no finding', J,
     "    if holder == 'none' or len(lease) < 2:\n", "    if False:\n",
     "    elif status.get('lease_epoch') != lease[1]:\n",
     "    elif len(lease) > 1 and status.get('lease_epoch') != lease[1]:\n"),
    ('J56 the status\'s epoch is not compared with the bucket\'s', J,
     "    elif status.get('lease_epoch') != lease[1]:\n", "    elif False:\n"),
    ('J57 a recording that is no course drops node B\'s read of the lease', J,
     "        wrong += cbad           # no course line", "        pass                    # no course line"),
    # ---- the judge: ulease's rows before the release (G4, cd849a6)
    ('J58 ulease: no row before the release is no finding', J, "        if not before:\n", "        if False:\n"),
    ('J59 ulease: head may move before the release', J, MOVED,
     "        moved = [r for r in before if r[2] != 'none']\n"),
    ('J60 ulease: the row may name a cut before the release', J, MOVED,
     "        moved = [r for r in before if r[1] != before[0][1]]\n"),
    ('J61 ulease: head must be none before the release (the want before cd849a6)', J, MOVED,
     "        moved = [r for r in before if r[1] != 'none' or r[2] != 'none']\n"),
    ('J62 ulease: rows after the release count as before it', J,
     "        before = [r for r in rows if len(r) == 4 and int(r[0]) < until]\n",
     "        before = [r for r in rows if len(r) == 4]\n"),

    # ---- the recorder, as far as test_cut_record.sh reaches (G2, 6b1be16)
    ('R20 other_act: an act that found the row moved is taken as done', R,
     "        [ \"$rc\" = 6 ] || return 0\n", "        return 0\n"),
    ('R23 before_other: the pool is not synced before the look', R,
     "        if zpool list -H \"$POOL\" > /dev/null 2>&1; then z 60 zpool sync \"$POOL\"; fi\n", "        :\n"),
    ('R24 before_other: the pool is synced whether imported or not', R,
     "        if zpool list -H \"$POOL\" > /dev/null 2>&1; then z 60 zpool sync \"$POOL\"; fi\n",
     "        z 60 zpool sync \"$POOL\"\n"),
    ('R25 before_other: the row is never read again while it waits', R,
     "            if [ $((_i % 10)) = 0 ]; then\n", "            if false; then\n"),
    ('R26 before_other: a row that moved on is no reason to look again', R,
     "                [ \"$now\" = \"$h $st $n $g\" ] || break\n", "                true\n"),
    ('R27 before_other: a fetch under another head counts', R, LOOK,
     LOOK.replace(' && $4 == h', '')),
    ('R28 before_other: a fetch of another cut of the state counts', R, LOOK,
     LOOK.replace('$1 == n && $2 == g && ', '')),
    ('R30 before_other: a cut the watcher never fetched is no error', R,
     "        [ -n \"$now\" ] || rig_error \"the watcher had not fetched cut $n",
     "        [ -n \"$now\" ] || return 0; : \"the watcher had not fetched cut $n"),
    ('R31 before_other: the second look is not written to looks-again.txt', R,
     "        printf '%s\\t%s\\t%s\\n' \"$1\" \"$h $st $n $g\" \"$now\" >> \"$RDIR/looks-again.txt\"\n", ''),
    ('R40 (kernel only) ufailed does not wait for node B before the failure (D1)', R,
     "    before_other failed\n", ''),
    ('R41 (kernel only) guarded: node B\'s exit 6 is taken as done', R, "    return \"$rc\"\n}\n", "    return 0\n}\n"),
    ('R42 (kernel only) the acts are not guarded', R,
     "    nodeb \"$@\" --expect-row \"$row\" > \"$RDIR/$f\" 2>&1 || rc=$?\n",
     "    nodeb \"$@\" > \"$RDIR/$f\" 2>&1 || rc=$?\n"),
    ('R43 (kernel only) ufailed writes its blocks over the front labels', R,
     "    local at=$(((2 << 20) >> 12))\n", "    local at=0\n"),
    ('R44 (kernel only) ulease writes the release\'s time after the release', R,
     "    python3 -I -c 'import time; print(time.monotonic_ns())' > \"$RDIR/release-ns.txt\"\n"
     "    nodeb release --holder other-node > \"$RDIR/other-release.txt\" 2>&1 \\\n"
     "        || rig_error \"node B could not let the lease go: $(tail -n 2 \"$RDIR/other-release.txt\")\"\n",
     "    nodeb release --holder other-node > \"$RDIR/other-release.txt\" 2>&1 \\\n"
     "        || rig_error \"node B could not let the lease go: $(tail -n 2 \"$RDIR/other-release.txt\")\"\n"
     "    sleep 30; python3 -I -c 'import time; print(time.monotonic_ns())' > \"$RDIR/release-ns.txt\"\n"),

    # ---- node B: --expect-row (G2)
    ('N20 move-head: a row that did not hold is an error, not "moved"', N,
     "        Err(EngineError::OpConflict { .. }) => Ok(None),\n", ''),
    ('N21 move-head: head moved under a guarded commit is an error, not "moved"', N,
     "        Err(EngineError::Conflict) if guarded => Ok(None),\n", ''),
    ('N22 move-head: the guard is built and not given to the commit', N,
     "        Some(op) => eng.commit_with(CommitKind::Flush, vec![op]).await,\n",
     "        Some(op) => {\n            drop(op);\n            eng.commit(CommitKind::Flush).await\n        }\n"),
    ('N23 row_held: expects no row', N,
     "        key: ROW_KEY.to_string(),\n        expected: expect,\n        value,\n",
     "        key: ROW_KEY.to_string(),\n        expected: None,\n        value,\n"),
    ('N24 row_held: holds the row and writes it empty', N,
     "        key: ROW_KEY.to_string(),\n        expected: expect,\n        value,\n",
     "        key: ROW_KEY.to_string(),\n        expected: expect,\n        value: {\n            drop(value);\n"
     "            Vec::new()\n        },\n"),
    ('N25 set-row: the guard is dropped (a plain put)', N,
     "        (Some(value), Some(expected)) => vec![RefOp::MetaCas {\n            key,\n            expected,\n"
     "            value,\n        }],\n",
     "        (Some(value), Some(_)) => vec![RefOp::MetaPut { key, value }],\n"),
    ('N26 set-row: a row that did not hold reads as set', N,
     "        TxnOutcome::Conflict { .. } => Ok(false),\n", "        TxnOutcome::Conflict { .. } => Ok(true),\n"),
    ('N27 set-row none: the guard is dropped', N,
     "        (None, Some(expected)) => vec![\n            row_held(expected)?,\n",
     "        (None, Some(_)) => vec![\n"),
    ('N28 a row that moved ends with 0', N,
     "            println!(\"row\\tmoved\");\n            6\n", "            println!(\"row\\tmoved\");\n            0\n"),
    ('N29 --expect-row is read and ignored', N,
     "            \"--expect-row\" => a.expect_row = Some(v),\n", "            \"--expect-row\" => drop(v),\n"),
    ('N30 the move-head command gives the function no expected row', N,
     "    let moved = move_head_on(store, refs, volume_id(&a.volume), at, group, expect).await?;\n",
     "    drop(expect);\n    let moved = move_head_on(store, refs, volume_id(&a.volume), at, group, None).await?;\n"),
    ('N31 the set-row command gives the function no expected row', N,
     "    let set = set_row_on(&refs, volume_id(&a.volume), bytes, expect).await?;\n",
     "    drop(expect);\n    let set = set_row_on(&refs, volume_id(&a.volume), bytes, None).await?;\n"),

    # ---- the plan of the jobs (G7, cfe8146)
    ('P01 a path that begins like a listed file counts', P, TOUCHES, "c.startswith(p)"),
    ('P02 a directory\'s name without its slash counts as a prefix', P, TOUCHES, "c.startswith(p.rstrip('/'))"),
    ('P03 every changed path must be an upload path', P, "    return any(c == p or", "    return all(c == p or"),
    ('P12 a pull request runs the held cuts\' stage on one image', P,
     "        jobs = [entry(i, True, False, '') for i in PR_IMAGES]\n",
     "        jobs = [entry(i, True, False, '') for i in PR_IMAGES[:1]]\n"),
    ('P13 the upload job takes the place of the held cuts\' jobs', P,
     "            jobs.append(entry(PR_UPLOAD_IMAGE, False, True, PR_UPLOAD_RECORDINGS))\n",
     "            jobs = [entry(PR_UPLOAD_IMAGE, False, True, PR_UPLOAD_RECORDINGS)]\n"),
    ('P14 the pull request\'s upload job also holds cuts', P,
     "entry(PR_UPLOAD_IMAGE, False, True, PR_UPLOAD_RECORDINGS)", "entry(PR_UPLOAD_IMAGE, True, True, PR_UPLOAD_RECORDINGS)"),
    ('P15 the pull request\'s upload job leaves out ulost', P,
     "PR_UPLOAD_RECORDINGS = 'life umoved urow ukill ulease ulost'", "PR_UPLOAD_RECORDINGS = 'life umoved urow ukill ulease'"),
    ('P15b the pull request\'s upload job runs every recording (empty list)', P,
     "PR_UPLOAD_RECORDINGS = 'life umoved urow ukill ulease ulost'", "PR_UPLOAD_RECORDINGS = ''"),
    ('P16 a dispatch with upload also holds cuts', P, "hold == 'true' and not up", "hold == 'true'"),
    ('P17 a dispatch uploads whatever its input says', P, "        up = upload == 'true'\n", "        up = bool(upload)\n"),
    ('P17b a dispatch never uploads', P, "        up = upload == 'true'\n", "        up = False\n"),
    ('P17c a dispatch drops its recordings', P,
     "return [entry(i, hold == 'true' and not up, up, recordings) for i in json.loads(images)]",
     "return [entry(i, hold == 'true' and not up, up, '') for i in json.loads(images)]"),
    ('P17d a dispatch runs its first image alone', P, "for i in json.loads(images)]", "for i in json.loads(images)[:1]]"),
    ('P18 a push runs on arm64', P, "        return [entry('ubuntu-26.04', True, False, '')]\n",
     "        return [entry('ubuntu-26.04-arm', True, False, '')]\n"),
    ('P18b a push does not hold cuts', P, "        return [entry('ubuntu-26.04', True, False, '')]\n",
     "        return [entry('ubuntu-26.04', False, False, '')]\n"),
    ('P19 the upload job runs on arm64', P, "PR_UPLOAD_IMAGE = 'ubuntu-26.04'\n", "PR_UPLOAD_IMAGE = 'ubuntu-26.04-arm'\n"),
    ('P20 the command does not read the changed paths', P, "    if len(sys.argv) == 7:\n", "    if False:\n"),
    ('P21 the upload job has the held cuts\' job\'s label', P, "label = image + ('-upload' if upload else '')",
     "label = image"),
    ('P22 an event with no plan runs nothing', P, "    raise ValueError(f'no plan for the event {event!r}')\n",
     "    return []\n"),
    ('P23 upload is written true or false, not 1 or 0', P, "'upload': '1' if upload else '0'",
     "'upload': 'true' if upload else 'false'"),
    ('P24 a pull request always gets the upload job', P, "        if touches_upload(changed):\n", "        if True:\n"),
] + [(f'P0{4 + i} the plan does not know the path {p}', P, f"    '{p}',\n", '') for i, p in enumerate(PATHS[:6])] \
  + [(f'P1{i} the plan does not know the path {p}', P, f"    '{p}',\n", '')
     for i, p in zip(('0', '1', '1b'), PATHS[6:])]

plants.PLANTS = [p for p in plants.PLANTS if p[0].split()[0] in EARLIER] + NEW

if __name__ == '__main__':
    plants.main()
