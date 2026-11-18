#!/usr/bin/env python3
"""#119 at 32d8d67 (M2.T3 stage 3): plants, one at a time, in what the
pull request added, against the tests of the changed files.

    plants.py CHECKOUT OUT [PREFIX ...]

A plant is a text of a file replaced by another. One whose text is not
found exactly once says NOT APPLIED. The file is restored after each
(try/finally), each result is appended to OUT as it comes, a plant whose
name OUT already holds is skipped, and the script refuses to start on a
checkout that `git status --short` does not call clean. Each test file
runs under a limit of its own; a file that does not end in it counts as
failed. The first test file that fails ends the plant's run.

What runs for a plant goes by its file: the watcher's plants run
test_cut_watch.py; the judge's run test_cut_daemon_main.py and
test_cut_daemon_check.py; the recorder's run test_cut_record.sh; node
B's run `cargo test --locked -p skyzfs-nodeb` (CARGO_TARGET_DIR from the
environment). Nothing here needs a kernel, so a plant in what only a
kernel run reaches (the recorder's courses) says SURVIVED by
construction: those are listed to say so, not to count.
"""
import os
import subprocess
import sys

ROOT, OUT = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
W = 'tests/kernel/cut_watch.py'
J = 'tests/kernel/cut_daemon_check.py'
R = 'tests/kernel/cut-record.sh'
N = 'bins/skyzfs-nodeb/src/main.rs'
TESTS = {
    W: [('py', 'tests/kernel/test_cut_watch.py')],
    J: [('py', 'tests/kernel/test_cut_daemon_main.py'), ('py', 'tests/kernel/test_cut_daemon_check.py')],
    R: [('sh', 'tests/kernel/test_cut_record.sh')],
    N: [('cargo', 'skyzfs-nodeb')],
}

AFTER = "        if after is None or (after['head'], after['row']) != key:\n"
SKIP = "        if g['head'] in heads or (g['state'], g['number'], g['group']) in rows:\n"
WANT = ("    want = {'upload_state': 'idle', 'cuts_held': '0', 'lease': 'held', 'lease_holder': holder,\n"
        "            'uploaded_whole': '1', 'lease_epoch': '1'}\n")
ZERO = ("UPLOAD_ZERO = ('upload_mismatches', 'upload_abandoned_at_mismatch', 'upload_cuts_lost', 'upload_given_up',\n"
        "               'upload_committed_kept', 'upload_found_committed')\n")
COMMITTED = ("    committed = d['uploaded'] + d['upload_found_committed'] - d['upload_committed_gone'] "
             "- d['upload_committed_kept']\n")
REST = "    rest = committed + d['cuts_replaced'] + d['cuts_abandoned'] + d['cuts_held']\n"
UMOVED = ("        want.update(upload_mismatches=at_least(1), upload_abandoned_at_mismatch=at_least(1),\n"
          "                    upload_checked='head_moved', uploaded=n)\n")
UROW = ("        want.update(upload_mismatches='2', upload_abandoned_at_mismatch=at_least(0), uploaded_whole='3',\n"
        "                    upload_checked='matched')\n")
UROW_END = "        if end['cut'] is None or end['cut'] in rows or end['cut'][0] != first['cut'][0]:\n"
UKILL = "            want.update(upload_found_committed='1', uploaded_whole='0', uploaded=at_least(1))\n"
UFAILED = ("            want.update(uploaded_whole='0', uploaded=at_least(1), upload_rule='delta', "
           "upload_checked='matched')\n")
ULOST = ("        want.update(upload_state='lease_lost', lease='lost', lease_holder=OTHER, cuts_held=at_least(0),\n"
         "                    lease_epoch=took[3] if len(took) > 3 and took[:2] == ['lease', 'acquired'] else '?', "
         "uploaded=n)\n")
ULOST_END = "        if (end['head'], end['cut']) != (before['head'], before['cut']):\n"
ANY = 'lambda v: True'


def want_without(key):
    return WANT.replace(f"'{key}': 'idle', ", '').replace(f"'{key}': '0', ", '').replace(f"'{key}': 'held', ", '') \
        .replace(f"'{key}': holder,\n            ", '').replace(f"'{key}': '1', ", '') \
        .replace(f", '{key}': '1'}}", '}')


def zero_without(key):
    return ZERO.replace(f"'{key}', ", '').replace(f"'{key}',\n               ", '').replace(f", '{key}')", ')')


PLANTS = [
    # ---- the watcher (cut_watch.py): which image goes with which row
    ('W01 a fetch is kept whatever head and the row are after the read', W, AFTER, "        if after is None:\n"),
    ('W02a a fetch is kept when head alone did not move (the row may have)', W, AFTER,
     "        if after is None or after['head'] != key[0]:\n"),
    ('W02b a fetch is kept when the row alone did not move (head may have)', W, AFTER,
     "        if after is None or after['row'] != key[1]:\n"),
    ('W03 the same head and row are fetched again at every look', W,
     "        if key == self.last:\n            return False\n", ''),
    ('W04 a row seen before is not fetched again under another head', W, "        if key == self.last:\n",
     "        if self.last is not None and key[1] == self.last[1]:\n"),
    ('W05 a row with no head is fetched', W, " or before['head'] == 'none'", ''),
    ('W07 a block that became zeros is not reported as changed', W,
     "    for b in before:\n        if b not in after:\n            out[b] = ZERO\n", ''),
    ('W08 the blocks of the last fetch are not kept: every fetch is compared with nothing', W,
     "        self.last, self.blocks = key, blocks\n", "        self.last = key\n"),
    ('W09 what changed is taken against an empty image', W, "        diff = changed(self.blocks, blocks)\n",
     "        diff = changed({}, blocks)\n"),
    ('W10 a head node B cannot read (exit 4) is not a verdict', W,
     "        if rc == 4 and cmd == 'get':\n            raise Verdict(f'node B could not read head: {err}')\n", ''),
    ('W11 any failure of get is a verdict', W, "        if rc == 4 and cmd == 'get':\n",
     "        if rc != 0 and cmd == 'get':\n"),
    ('W12 failures are never forgotten: three in the whole recording end it', W,
     "        self.failures = 0\n        return out\n", "        return out\n"),
    ('W13 failures never end the watcher', W, "            if self.failures >= TRIES:\n", "            if False:\n"),
    ('W14 blocks of zeros are hashed like any other', W, "                if blk.count(0) != len(blk):\n",
     "                if True:\n"),
    ('W15 blocks are numbered among those that are not zeros', W,
     "digest()[:8].hex()\n                k += 1\n", "digest()[:8].hex()\n                    k += 1\n"),
    ('W17 no fetch after the stop', W,
     "        for _ in range(TRIES):\n            w.step()\n            if w.last is not None and w.failures == 0:\n"
     "                break\n", ''),
    ('W17b one try after the stop, whatever it found', W, "        for _ in range(TRIES):\n", "        for _ in range(1):\n"),
    ('W18 the pause is ignored', W, "            if not (pause and os.path.exists(pause)):\n                w.step()\n",
     "            w.step()\n"),
    ('W19 the watcher ends with 0 after a verdict', W, "        w.log(f'VERDICT {e}')\n        return 1\n",
     "        w.log(f'VERDICT {e}')\n        return 0\n"),
    ('W20 the watcher ends with 1 after its own error', W, "        w.log(f'ERROR {e}')\n        return 2\n",
     "        w.log(f'ERROR {e}')\n        return 1\n"),
    ('W21 uploaded.tsv has the group where the number goes', W,
     'f.write(f"{number}\\t{group}\\t{state}', 'f.write(f"{group}\\t{number}\\t{state}'),
    ('W22 uploaded.tsv has the end of the read where its beginning goes', W, '{t0}\\t{t1}\\n")', '{t1}\\t{t0}\\n")'),
    ('W23 every fetch is fetch 0 in uploaded-blocks.tsv', W, "        self.fetched += 1\n", ''),
    ('W24 the row file is not written', W,
     "            f.write(f\"{time.monotonic_ns()}\\t{got['head']}\\t{got['row']}\\t{got['lease']}\\n\")\n",
     "            pass\n"),
    # ---- the judge: which fetches are the daemon's
    ('J01 a fetch under another writer\'s head counts as the daemon\'s', J, SKIP,
     "        if (g['state'], g['number'], g['group']) in rows:\n"),
    ('J02 a fetch of another writer\'s row counts as the daemon\'s', J, SKIP, "        if g['head'] in heads:\n"),
    ('J04 the image is not followed through a skipped fetch', J,
     "        image.update(per.get(k, {}))\n" + SKIP + "            skipped += 1\n            continue\n",
     SKIP + "            skipped += 1\n            continue\n        image.update(per.get(k, {}))\n"),
    ('J05 a number fetched twice is not named', J,
     "        if g['number'] in blocks:\n            twice.add(g['number'])\n            continue\n",
     "        if g['number'] in blocks:\n            continue\n"),
    ('J05b a cut under two commits is not a verdict', J,
     "    if twice:\n        ok, detail = False, f\"cuts {sorted(twice)} were at head under two commits; {detail}\"\n", ''),
    ('J06 a later state\'s numbers are not moved to their places in the log', J,
     "        g = dict(g, number=g['number'] + offsets[e])\n", "        g = dict(g)\n"),
    ('J07 a state past the last epoch is taken as the last epoch\'s', J,
     "        if e >= len(offsets):\n            continue\n", "        if e >= len(offsets):\n            e = len(offsets) - 1\n"),
    ('J08 more states than epochs is not a verdict', J, "    if len(states) > n_epochs:\n", "    if False:\n"),
    ('J09 each cut\'s blocks are all the image\'s, not what changed', J, "        at_last = dict(image)\n", ''),
    ('J10 the watcher\'s VERDICT is not read', J,
     "    bad = [ln.split('\\tVERDICT ', 1)[1] for ln in log.splitlines() if '\\tVERDICT ' in ln]\n"
     "    put('upload_reads',", "    bad = []\n    put('upload_reads',"),
    ('J11 the watcher\'s ERROR is not read', J,
     "    log = read_file(rdir, 'watch.log')\n    err = [ln for ln in log.splitlines() if '\\tERROR ' in ln]\n",
     "    log = read_file(rdir, 'watch.log')\n    err = []\n"),
    ('J12 no cut of the daemon\'s fetched is not a verdict', J, "        return 1, False\n    by_number",
     "        return 0, False\n    by_number"),
    ('J13 a course that stops the uploads is held to the last cut counted', J, "    if name in STOPPED_AT:\n",
     "    if False:\n"),
    ('J13b the last cut fetched is whatever it is', J,
     "    ok, detail, controls, _ = held_content(records, dcuts, written, blocks, last=last)\n",
     "    ok, detail, controls, _ = held_content(records, dcuts, written, blocks, last=written[-1]['number'])\n"),
    ('J14 held_content does not hold the last cut written', J, "    if written[-1]['number'] != last:\n", "    if False:\n"),
    ('J16 the content line does not set the exit', J,
     "if skipped else '') + f\": {detail}\")\n        rc = rc if ok else 1\n", "if skipped else '') + f\": {detail}\")\n"),
    ('J16b a content control that differs does not set the exit', J,
     "        rc = rc if good else 1\n    drain = read_file", "    drain = read_file"),
    # ---- the judge: the status after the drain
    ('J17a upload_state is not wanted', J, WANT, want_without('upload_state')),
    ('J17b cuts_held is not wanted', J, WANT, want_without('cuts_held')),
    ('J17c lease is not wanted', J, WANT, want_without('lease')),
    ('J17d lease_holder is not wanted', J, WANT, want_without('lease_holder')),
    ('J17e uploaded_whole is not wanted', J, WANT, want_without('uploaded_whole')),
    ('J17f lease_epoch is not wanted', J, WANT, want_without('lease_epoch')),
    ('J17g upload_mismatches may be anything', J, ZERO, zero_without('upload_mismatches')),
    ('J17h upload_abandoned_at_mismatch may be anything', J, ZERO, zero_without('upload_abandoned_at_mismatch')),
    ('J17i upload_cuts_lost may be anything', J, ZERO, zero_without('upload_cuts_lost')),
    ('J17j upload_given_up may be anything', J, ZERO, zero_without('upload_given_up')),
    ('J17k upload_committed_kept may be anything', J, ZERO, zero_without('upload_committed_kept')),
    ('J17l upload_found_committed may be anything', J, ZERO, zero_without('upload_found_committed')),
    ('J17m the holder is the first the rows named, not the last', J, "    holder = rows[-1][3].split()[0]\n",
     "    holder = rows[0][3].split()[0]\n"),
    ('J18 uploaded may be fewer than node B fetched', J,
     "    if name not in UPLOAD_COURSES and (not n.isdigit() or int(n) < len(mine)):\n", "    if False:\n"),
    ('J18b the status line does not set the exit', J, "    rc = rc if not wrong else 1\n", ''),
    # ---- the judge: the balance
    ('J19a the balance without upload_found_committed', J, COMMITTED,
     COMMITTED.replace(" + d['upload_found_committed']", '')),
    ('J19b the balance without upload_committed_gone', J, COMMITTED,
     COMMITTED.replace(" - d['upload_committed_gone']", '')),
    ('J19c the balance without upload_committed_kept', J, COMMITTED,
     COMMITTED.replace(" - d['upload_committed_kept']", '')),
    ('J19d the balance adds upload_committed_gone', J, COMMITTED,
     COMMITTED.replace(" - d['upload_committed_gone']", " + d['upload_committed_gone']")),
    ('J19e the balance without cuts_replaced', J, REST, REST.replace(" + d['cuts_replaced']", '')),
    ('J19f the balance without cuts_abandoned', J, REST, REST.replace(" + d['cuts_abandoned']", '')),
    ('J19g the balance without cuts_held', J, REST, REST.replace(" + d['cuts_held']", '')),
    ('J19h the balance of the figures at the end, not of their change', J,
     "    d = {k: int(end[k]) - int(start[k]) for k in BALANCE}\n", "    d = {k: int(end[k]) for k in BALANCE}\n"),
    ('J19i the balance holds whatever the sides are', J, "    return rest == d['cuts'], detail\n",
     "    return True, detail\n"),
    ('J19j the balance from the first instance\'s start', J, "    k = max(int(m.group(1))", "    k = min(int(m.group(1))"),
    ('J19k the balance line does not set the exit', J,
     "f'serve {k}: {detail}')\n    rc = rc if ok else 1\n", "f'serve {k}: {detail}')\n"),
    ('J19l the balance of the end against itself', J,
     "upload_balance(read_status(read_file(rdir, f'status-start-{k}.txt')), status)", "upload_balance(status, status)"),
    ('J19m a status with no cuts_replaced balances as 0', J,
     "    d = {k: int(end[k]) - int(start[k]) for k in BALANCE}\n",
     "    d = {k: int(end.get(k) or 0) - int(start.get(k) or 0) for k in BALANCE}\n",
     "    if missing:\n        return False, f\"the status has no", "    if False:\n        return False, f\"the status has no"),
    # ---- the judge: the courses
    ('J20 umoved: uploaded need not be what it was before the other writer', J, UMOVED,
     UMOVED.replace(", uploaded=n)", ")")),
    ('J20b umoved: head at the end may be anyone\'s', J, "        if end['head'] not in heads:\n", "        if False:\n"),
    ('J20c umoved: the row at the end may have moved', J, "        if end['cut'] != before['cut']:\n", "        if False:\n"),
    ('J20d umoved: no mismatch need be counted', J, UMOVED, UMOVED.replace("upload_mismatches=at_least(1)",
                                                                           "upload_mismatches=at_least(0)")),
    ('J20e umoved: nothing need be abandoned at the mismatch', J, UMOVED,
     UMOVED.replace("upload_abandoned_at_mismatch=at_least(1)", "upload_abandoned_at_mismatch=at_least(0)")),
    ('J20f umoved: upload_checked may be anything', J, UMOVED, UMOVED.replace("upload_checked='head_moved', ", '')),
    ('J21 urow: one mismatch is enough', J, UROW, UROW.replace("upload_mismatches='2'", "upload_mismatches=at_least(1)")),
    ('J21b urow: one whole cut is enough', J, UROW, UROW.replace("uploaded_whole='3'", "uploaded_whole=at_least(1)")),
    ('J21c urow: the row at the end may be the other writer\'s', J, UROW_END,
     "        if end['cut'] is None or end['cut'][0] != first['cut'][0]:\n"),
    ('J21d urow: the row at the end may be of another state', J, UROW_END,
     "        if end['cut'] is None or end['cut'] in rows:\n"),
    ('J21e urow: upload_checked may be anything', J, UROW, UROW.replace(",\n                    upload_checked='matched')",
                                                                       ")")),
    ('J22 ukill: the first instance may have ended any way', J, "        if rc != '137':\n", "        if False:\n"),
    ('J22b ukill: the hook\'s line may name any point', J, "        if not hook or point not in hook[0]:\n",
     "        if not hook:\n"),
    ('J22c ukill: upload_found_committed may be 0', J, UKILL,
     UKILL.replace("upload_found_committed='1'", "upload_found_committed=at_least(0)")),
    ('J22d ukill: the next instance may upload a whole cut', J, UKILL,
     UKILL.replace("uploaded_whole='0'", "uploaded_whole=at_least(0)")),
    ('J22e ukill: the next instance need upload nothing', J, UKILL, UKILL.replace("uploaded=at_least(1)",
                                                                                  "uploaded=at_least(0)")),
    ('J23 unostore: the status with the store down may say anything', J, "            if down.get(k) not in v:\n",
     "            if False:\n"),
    ('J23b unostore: a lease held with the store down', J, "(('lease', ('none',)), ('upload_state', ('no_lease', 'backoff'))",
     "(('lease', ('none', 'held')), ('upload_state', ('no_lease', 'backoff'))"),
    ('J23c unostore: a cut uploaded with the store down', J, "('uploaded', ('0',))):\n", "('uploaded', ('0', '1'))):\n"),
    ('J24 ufailed: the failed instance may have ended any way', J, "        if rc != '3':\n", "        if False:\n"),
    ('J24b ufailed: the start after the failure may have taken the state up', J,
     "        if st2.get('cut_state') != 'begun':\n", "        if False:\n"),
    ('J24c ufailed: with the map taken up, a whole first cut passes', J, UFAILED,
     "            want.update(uploaded_whole=at_least(0), uploaded=at_least(1), upload_rule='delta', "
     "upload_checked='matched')\n"),
    ('J24d ufailed: with the map taken up, any rule passes', J, UFAILED, UFAILED.replace(", upload_rule='delta'", '')),
    ('J24e ufailed: with the map taken up, upload_checked may be anything', J, UFAILED,
     UFAILED.replace(", upload_checked='matched'", '')),
    ('J24f ufailed: a log that says nothing of the intent map passes', J,
     "        elif 'intent map: begun anew' not in log2:\n", "        elif False:\n"),
    ('J24g ufailed: the map taken up is read as begun anew', J, "        if 'intent map: taken up' in log2:\n",
     "        if False:\n"),
    ('J25 ulease: the refused start may have ended any way', J, "        if rc != '10':\n", "        if False:\n"),
    ('J25b ulease: served without upload, the status may say anything', J, "            if without.get(k) not in v:\n",
     "            if False:\n"),
    ('J25c ulease: served without upload, cuts may have been uploaded', J, "        if n != '0':\n", "        if False:\n"),
    ('J26 ulost: upload_state may be anything', J, ULOST, ULOST.replace("upload_state='lease_lost'", f"upload_state={ANY}")),
    ('J26b ulost: lease may be anything', J, ULOST, ULOST.replace("lease='lost'", f"lease={ANY}")),
    ('J26c ulost: lease_holder may be anything', J, ULOST, ULOST.replace("lease_holder=OTHER", f"lease_holder={ANY}")),
    ('J26d ulost: lease_epoch may be anything', J, ULOST,
     ULOST.replace("lease_epoch=took[3] if len(took) > 3 and took[:2] == ['lease', 'acquired'] else '?'",
                   f"lease_epoch={ANY}")),
    ('J26e ulost: uploaded need not be what it was when the lease was lost', J, ULOST, ULOST.replace(", uploaded=n)", ")")),
    ('J26f ulost: head and the row at the end may have moved', J, ULOST_END, "        if False:\n"),
    ('J26g ulost: head at the end may have moved', J, ULOST_END, "        if end['cut'] != before['cut']:\n"),
    ('J26h ulost: the row at the end may have moved', J, ULOST_END, "        if end['head'] != before['head']:\n"),
    ('J26i ulost: the lease at the end may be anyone\'s', J,
     "        if end['lease'] is None or end['lease'][0] != OTHER:\n", "        if False:\n"),
    ('J27 a course\'s own findings do not set the exit', J, "        rc = rc if not cbad else 1\n", ''),
    ('J27b a course\'s own findings are a PASS line', J, "put('upload_course', 'FAIL' if cbad else 'PASS',",
     "put('upload_course', 'PASS',"),
    ('J28 any recording that uploads may start the daemon again', J,
     "            if len(instances) > 1 and name not in TWO_STARTS:\n", "            if False:\n"),
    # ---- the test of a state through two starts (the code is #116's; the test is this pull request's)
    ('T01 the state\'s cuts are counted from the instance before, not from the start that began it', J,
     "        lo = bounds[j - 1]['i'] if j else -1\n", "        lo = bounds[k - 2]['i'] if k > 1 else -1\n"),
    ('T02 the state\'s releases are counted from the instance before, not from the start that began it', J,
     "            since = instances[j]['ns'] if j else -1\n", "            since = instances[k - 1]['ns']\n"),
    # ---- the recorder (what test_cut_record.sh reaches, and what only a kernel run does)
    ('R01 the pause is lifted after PRE_SERVE set it', R,
     '    rm -f "${WORK:?}/watch.pause"\n    [ -z "$PRE_SERVE" ] || "$PRE_SERVE"\n',
     '    [ -z "$PRE_SERVE" ] || "$PRE_SERVE"\n    rm -f "${WORK:?}/watch.pause"\n'),
    ('R02 the watcher starts by lifting the pause', R, '    rm -f "${WORK:?}/watch.stop"\n    CUT_WATCH_PAUSE=',
     '    rm -f "${WORK:?}/watch.stop" "$WORK/watch.pause"\n    CUT_WATCH_PAUSE='),
    ('R03 (kernel only) before_other does not wait for the watcher', R,
     '    h=$(awk -F \'\\t\' \'$1 == "head" { print $2 }\' "$RDIR/$1-row.txt")\n',
     '    h=$(awk -F \'\\t\' \'$1 == "head" { print $2 }\' "$RDIR/$1-row.txt"); return 0\n'),
    ('R04 (kernel only) the watcher\'s verdict ends the recording as the rig\'s error', R,
     '    if [ "$rc" = 1 ] && grep -q "$(printf \'\\tVERDICT \')" "$RDIR/watch.log"; then\n',
     '    if false; then\n'),
    ('R05 (kernel only) the drain does not wait for the held cuts', R,
     '{ [ -z "$held" ] || grep -qx \'cuts_held 0\' <<< "$st"; } && break', 'break'),
    ('R06 (kernel only) the watcher is stopped before the drain', R,
     '    drain drain.txt\n    touch "$WORK/watch.stop"\n', '    touch "$WORK/watch.stop"\n    drain drain.txt\n'),
    # ---- node B's new commands
    ('N01 row: the number and the group read from each other\'s place', N,
     "    n.copy_from_slice(&b[18..26]);\n    g.copy_from_slice(&b[26..34]);\n",
     "    n.copy_from_slice(&b[26..34]);\n    g.copy_from_slice(&b[18..26]);\n"),
    ('N02 row: any version is read as version 1', N, " || b[..2] != 1u16.to_le_bytes()", ''),
    ('N03 row: a longer row is read as version 1', N, "    if b.len() != ROW_LEN ||", "    if b.len() < ROW_LEN ||"),
    ('N04 row: the state is read one byte late', N, "        hex(&b[2..18]),\n", "        hex(&b[3..19]),\n"),
    ('N05 set-row: the number and the group put in each other\'s place', N,
     '    b.extend_from_slice(&number("--row\'s number", n)?.to_le_bytes());\n'
     '    b.extend_from_slice(&number("--row\'s group", g)?.to_le_bytes());\n',
     '    b.extend_from_slice(&number("--row\'s group", g)?.to_le_bytes());\n'
     '    b.extend_from_slice(&number("--row\'s number", n)?.to_le_bytes());\n'),
    ('N06 set-row: a state of any length', N,
     '    if state.len() != 16 {\n        return Err(format!("--row {v}: the state is 16 bytes"));\n    }\n', ''),
    ('N07 lease: exit 0 when another holds it', N, 'LeaseOutcome::Held(l) => ("held", l, 5),',
     'LeaseOutcome::Held(l) => ("held", l, 0),'),
    ('N08 move-head: the group is not set', N, "    eng.set_group(group);\n", ''),
    ('N09 set-row: the row is put under another key', N,
     "        Some(value) => RefOp::MetaPut {\n            key: ROW_KEY.to_string(),\n",
     "        Some(value) => RefOp::MetaPut {\n            key: \"skyzfs/other\".to_string(),\n"),
    ('N10 row: the cut\'s row read under another key', N, "        .meta_get(vol, ROW_KEY)\n",
     "        .meta_get(vol, \"skyzfs/other\")\n"),
    ('N11 the row\'s key is another', N, 'const ROW_KEY: &str = "skyzfs/cut";', 'const ROW_KEY: &str = "skyzfs/row";'),
]


def run_tests(env, path):
    """-> the names of what failed, [] when every test passes."""
    for kind, t in TESTS[path]:
        if kind == 'py':
            cmd, limit = [sys.executable, '-B', t], 300
        elif kind == 'sh':
            cmd, limit = ['bash', t], 300
        else:
            cmd, limit = ['cargo', 'test', '--locked', '-p', t], 1500
        try:
            p = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True, timeout=limit)
        except subprocess.TimeoutExpired:
            return [f'{os.path.basename(t)} did not end in {limit} s']
        if kind == 'py':
            failed = sorted({ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL: ', 'ERROR: '))})
        elif kind == 'sh':
            failed = [f'{os.path.basename(t)}: {ln[:80]}' for ln in p.stdout.splitlines()
                      if ln.startswith(('FAIL ', 'not ok', 'FAIL:'))]
        else:
            failed = sorted({ln.split()[1] for ln in p.stdout.splitlines()
                             if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
            if p.returncode != 0 and not failed and 'error' in p.stderr:
                failed = ['no test ran: ' + next((ln for ln in p.stderr.splitlines() if ln.startswith('error')), '')[:80]]
        if p.returncode != 0 and not failed:
            failed = [f'{os.path.basename(t)} exit {p.returncode}']
        if failed:
            return failed
    return []


def main():
    dirty = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True, timeout=120)
    if dirty.returncode != 0 or dirty.stdout.strip():
        sys.exit(f'{ROOT} is not a clean checkout: {dirty.stdout.strip() or dirty.stderr.strip()}')
    done = []
    if os.path.exists(OUT):
        with open(OUT) as f:
            done = f.read().splitlines()
    cache = {}
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
    with open(OUT, 'a') as out:
        def say(text):
            print(text, flush=True)
            out.write(text + '\n')
            out.flush()
            os.fsync(out.fileno())
        for name, path, *pairs in PLANTS:
            if ONLY and not name.startswith(ONLY):
                continue
            if any(ln.startswith(name + ': ') for ln in done):
                continue
            full = os.path.join(ROOT, path)
            if full not in cache:
                with open(full) as f:
                    cache[full] = f.read()
            orig = cache[full]
            counts = [orig.count(old) for old in pairs[0::2]]
            if any(c != 1 for c in counts) or any(old == new for old, new in zip(pairs[0::2], pairs[1::2])):
                say(f'{name}: NOT APPLIED (found {counts} times)')
                continue
            text = orig
            for old, new in zip(pairs[0::2], pairs[1::2]):
                text = text.replace(old, new)
            try:
                with open(full, 'w') as f:
                    f.write(text)
                failed = run_tests(env, path)
            finally:
                with open(full, 'w') as f:
                    f.write(orig)
            say(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(failed[:6])}' if failed else 'SURVIVED'))


if __name__ == '__main__':
    main()
