#!/usr/bin/env python3
"""#121 at 75f3227: plants in what the pull request changed (the plan's
paths, the guard's control on group 0, the wait before a second look on
a stale status), and the plants of #119's last look that it says are
now killed (R52, R65).

    plants4.py CHECKOUT OUT [PREFIX ...]

The harness is plants.py's, under ../m2.t3-3, with plants2.py's plants
of the plan run again as they were.
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'm2.t3-3'))
import plants   # noqa: E402
import plants2  # noqa: E402,F401  (puts its plants in plants.PLANTS)

R, P = plants.R, plants2.P
STALE = "        if [[ $cg =~ ^[0-9]+$ ]] && [ \"$cg\" -lt \"$g\" ]; then\n"
CONTROL = "        \"$2\" \"$st\" \"$n\" 0 || rc=$?\n"
WIRED = "        [ \"$rc\" = 6 ] || rig_error \"node B's act at $1 on the row $st:$n:0, which it did not read,"
TOUCHES = "c == p or (p.endswith('/') and c.startswith(p))"
NEW_PATHS = ('crates/skyzfs-blk/', 'crates/skynbd-engine/', 'crates/skynbd-refs/', 'crates/skynbd-store/',
             'crates/skynbd-format/', 'crates/skynbd-types/', 'Cargo.toml', 'Cargo.lock', 'tests/lib/',
             'tests/kernel/cut_plan.py', 'tests/kernel/cut_summary.py', 'rust-toolchain.toml')
OLD_PATHS = ('crates/skyzfs-upload/', 'bins/skyzfsd/', 'crates/skyzfs-local/', 'crates/skyzfs-ublk/',
             'bins/skyzfs-nodeb/', 'tests/kernel/cut_watch.py', 'tests/kernel/cut-record.sh',
             'tests/kernel/cut_daemon_check.py', '.github/workflows/skyzfs-cut-daemon.yml')

NEW = [
    # ---- before_other: the status against the row
    ('R51 before_other: a status older than its row is taken', R, STALE, "        if false; then\n"),
    ('R52 before_other: a status whose cut group is not the row\'s, either way, means look again', R, STALE,
     STALE.replace('-lt', '-ne')),
    ('R53 before_other: a status ahead of its row means look again, one behind does not', R, STALE,
     STALE.replace('-lt', '-gt')),
    ('R58 before_other: a status at its row\'s group means look again too', R, STALE, STALE.replace('-lt', '-le')),
    ('R70 before_other: no wait before the look again on a stale status', R,
     "            sleep 1\n            continue\n", "            continue\n"),
    ('R71 before_other: the wait is taken and the look kept', R,
     "            sleep 1\n            continue\n", "            sleep 1\n"),
    # ---- other_act: the guard's control on group 0
    ('R60 other_act: no control', R, CONTROL, "        rc=6\n"),
    ('R61 other_act: a control that did not end with 6 is no error', R, WIRED,
     "        true || rig_error \"" + WIRED.split('rig_error "', 1)[1]),
    ('R62 other_act: the control acts on the row as read', R, CONTROL, "        \"$2\" \"$st\" \"$n\" \"$g\" || rc=$?\n"),
    ('R63 other_act: the control may end with any failure', R, WIRED, WIRED.replace('[ "$rc" = 6 ]', '[ "$rc" != 0 ]')),
    ('R64 other_act: the control is not written to guard-controls.txt', R,
     "        printf '%s\\t%s\\n' \"$1\" \"$st:$n:0\" >> \"$RDIR/guard-controls.txt\"\n", ''),
    ('R65 other_act: one control, before the first act alone', R, CONTROL,
     "        [ \"$_i\" = 1 ] && { \"$2\" \"$st\" \"$n\" 0 || rc=$?; } || rc=6\n"),
    ('R66 other_act: the control is on the number one past and the row\'s group, as before', R, CONTROL,
     "        \"$2\" \"$st\" $((n + 1)) \"$g\" || rc=$?\n"),
    ('R67 other_act: the control is on group 1', R, CONTROL, "        \"$2\" \"$st\" \"$n\" 1 || rc=$?\n"),
    # ---- the plan's paths
    ('P50 a path that ends like a listed file counts', P, TOUCHES, "c.endswith(p) or (p.endswith('/') and c.startswith(p))"),
    ('P51 a listed directory anywhere in the path counts', P, TOUCHES, "c == p or (p.endswith('/') and p in c)"),
    ('P52 a listed file counts as a directory too', P, TOUCHES,
     "c == p or c.startswith(p if p.endswith('/') else p + '/')"),
    ('P54 every crate starts the upload job', P, "    'Cargo.lock',\n", "    'Cargo.lock',\n    'crates/',\n"),
    ('P55 every file of tests/kernel starts the upload job', P, "    'Cargo.lock',\n", "    'Cargo.lock',\n    'tests/kernel/',\n"),
]
DISPATCH = "        return [entry(i, hold == 'true' and not up, up, recordings) for i in names]\n"
NEW += [
    # plants2.py's P17c and P17d, whose line 4aac932 rewrote
    ('P17e a dispatch drops its recordings', P, DISPATCH, DISPATCH.replace('up, recordings)', "up, '')")),
    ('P17f a dispatch runs its first image alone', P, DISPATCH, DISPATCH.replace('in names]', 'in names[:1]]')),
]
for k, p in enumerate(NEW_PATHS + OLD_PATHS):
    NEW.append((f'P{60 + k} the plan without {p}', P, f"    '{p}',\n", ''))

plants.PLANTS = [p for p in plants.PLANTS if p[0].startswith('P')] + NEW

if __name__ == '__main__':
    plants.main()
