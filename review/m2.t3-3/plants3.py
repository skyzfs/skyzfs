#!/usr/bin/env python3
"""#119 at 4aac932, the last look: plants in what changed since cfe8146
(the status against the row in before_other, the guard's control in
other_act, the plan's check of a dispatch's images), and the plants the
answers to the next look say are now killed.

    plants3.py CHECKOUT OUT [PREFIX ...]

The harness is plants.py's and the earlier plants' texts are plants.py's
and plants2.py's (same directory). Node B's source did not change since
cfe8146, so its plants are not run again.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import plants   # noqa: E402
import plants2  # noqa: E402,F401  (puts its plants in plants.PLANTS)

R, P = plants.R, plants2.P
AGAIN = ('W02a', 'W04', 'W05', 'W18', 'W19', 'W20', 'W22', 'W38', 'J42', 'J49', 'J50', 'R27', 'R28', 'R30',
         'R41', 'R42')

STALE = "        if [[ $cg =~ ^[0-9]+$ ]] && [ \"$cg\" -lt \"$g\" ]; then\n"
CONTROL = "        \"$2\" \"$st\" $((n + 1)) \"$g\" || rc=$?\n"
WIRED = "        [ \"$rc\" = 6 ] || rig_error \"node B's act at $1 on the row $st:$((n + 1)):$g, which it did not read,"
IMAGES = "        if not isinstance(names, list) or not names or not all(isinstance(i, str) and i for i in names):\n"

NEW = [
    # ---- before_other: the status against the row (F1)
    ('R51 before_other: a status older than its row is taken', R, STALE, "        if false; then\n"),
    ('R52 before_other: a status whose cut group is not the row\'s, either way, means look again', R, STALE,
     STALE.replace('-lt', '-ne')),
    ('R53 before_other: a status ahead of its row means look again, one behind does not', R, STALE,
     STALE.replace('-lt', '-gt')),
    ('R54 before_other: a look taken again for its status is not written to looks-again.txt', R,
     "            printf '%s\\t%s\\t%s\\n' \"$1\" \"$h $st $n $g\" \"status at group $cg\" >> \"$RDIR/looks-again.txt\"\n",
     ''),
    ('R55 before_other: a status older than its row is logged and the look kept', R,
     "\"status at group $cg\" >> \"$RDIR/looks-again.txt\"\n            continue\n",
     "\"status at group $cg\" >> \"$RDIR/looks-again.txt\"\n"),
    ('R56 before_other: the status\'s count of cuts is compared with the row\'s group', R,
     "        cg=$(skey \"$1-status.txt\" cut_group)\n", "        cg=$(skey \"$1-status.txt\" cuts)\n"),
    ('R57 before_other: a status older than its row is looked at again without end', R,
     "\"status at group $cg\" >> \"$RDIR/looks-again.txt\"\n            continue\n",
     "\"status at group $cg\" >> \"$RDIR/looks-again.txt\"\n            before_other \"$1\"; return\n"),
    # ---- other_act: the guard's control (F2)
    ('R60 other_act: no control', R, CONTROL, "        rc=6\n"),
    ('R61 other_act: a control that did not end with 6 is no error', R, WIRED, "        true || rig_error \"" + WIRED.split('rig_error "', 1)[1]),
    ('R62 other_act: the control acts on the row as read', R, CONTROL, "        \"$2\" \"$st\" \"$n\" \"$g\" || rc=$?\n"),
    ('R63 other_act: the control may end with any failure', R, WIRED,
     WIRED.replace('[ "$rc" = 6 ]', '[ "$rc" != 0 ]')),
    ('R64 other_act: the control is not written to guard-controls.txt', R,
     "        printf '%s\\t%s\\n' \"$1\" \"$st:$((n + 1)):$g\" >> \"$RDIR/guard-controls.txt\"\n", ''),
    ('R65 other_act: one control, before the first act alone', R, CONTROL,
     "        [ \"$_i\" = 1 ] && { \"$2\" \"$st\" $((n + 1)) \"$g\" || rc=$?; } || rc=6\n"),
    # ---- the plan: a dispatch's images (F4)
    ('P30 an empty list of images runs nothing', P, IMAGES, IMAGES.replace(' or not names', '')),
    ('P31 images that is no list is taken letter by letter', P, IMAGES, IMAGES.replace('not isinstance(names, list) or ', '')),
    ('P32 an image with no name is a job', P, IMAGES, IMAGES.replace(' and i for i in names', ' for i in names')),
    ('P33 images that is not JSON ends in a traceback', P, "        except ValueError:\n            names = None\n",
     "        except KeyError:\n            names = None\n"),
    ('P34 the command lets the plan\'s error through as a traceback', P, "    except ValueError as e:\n",
     "    except KeyError as e:\n"),
]

plants.PLANTS = [p for p in plants.PLANTS if p[0].split()[0] in AGAIN] + NEW

if __name__ == '__main__':
    plants.main()
