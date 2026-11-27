#!/usr/bin/env python3
"""Show-cases for the review of #178: what publish_check.py says of a
retire torn at the kill (pkillret), next to a publish torn at the kill
(pkillpub). Usage: showcase.py CHECKOUT (a worktree at the head)."""
import os
import sys

co = sys.argv[1]
sys.path.insert(0, os.path.join(co, 'tests', 'kernel'))
import test_publish_check_main as t  # noqa: E402

ROW_K1 = f'snaprow\t{t.S}\t8\t50\t{t.commit_of("k1")}\t{t.ULID}\t1\t{"0" * 64}\n'
ROW_P1 = f'snaprow\t{t.S}\t8\t50\t{t.commit_of("p1")}\t{t.ULID}\t1\t{"0" * 64}\n'


def both(*changes):
    def change(d):
        for c in changes:
            c(d)
    return change


def show(title, name, change=None, **kw):
    rc, lines = t.run_course(name, change, **kw)
    keep = {k: v for k, v in lines.items() if v[0] not in ('ok', 'INFO')}
    print(f'== {title}\n   exit {rc}')
    for k, (v, d) in keep.items():
        print(f'   {k}: {v}: {d[:150]}')


# A retire torn at the kill: the ref deleted, the row left. The next
# instance finds no ref (`gone`) and leaves the row, so node B still
# finds it after the take-over.
show('S1 pkillret: torn at the kill, the row left (ref gone)', 'pkillret',
     t.write('k1-between.txt', 'ref\tnone\n' + ROW_K1), rows={'k1': (8, 50)}, row_only=('k1',))
# The other way: the row deleted, the ref left. The next instance finds
# the ref on H and retires it: `retired`, and logs it.
show('S2 pkillret: torn at the kill, the ref left (row gone)', 'pkillret',
     both(t.write('k1-between.txt', f'ref\t{t.commit_of("k1")}\t3\nsnaprow\tnone\n'),
          t.answer('k1-again', 'retired'),
          t.write('requests.tsv', t.log_text([(t.PID, [(8, 50, True)]),
                                              (t.PID2, [('publish', 'retire', 'k1 retired')])]))))
# The same kind of fault at pkillpub's kill (after the transaction): a
# verdict there (pin_p1_found).
show('S3 pkillpub: torn at the kill, the row only', 'pkillpub',
     t.write('p1-between.txt', 'ref\tnone\n' + ROW_P1))
