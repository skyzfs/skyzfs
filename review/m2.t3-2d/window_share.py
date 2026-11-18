#!/usr/bin/env python3
"""#115 at afd217a: how much of an instance's life each kill window is,
by the request log's times, for the instances of `takeover` that the rig
ends with kill -9 (the first two).

    python3 -I window_share.py CHECKOUT ARTIFACT_DIR...

A kill at a uniformly random moment of the load leaves:
  N  a barrier line last, of a flush that carried no cut (barrier to the
     flush's commit, the store's sync between them): obs_unstored_at_kill
     reports it as a flush killed "between the barrier and the state
     storing the cut", shape B;
  B  a barrier line last, of a flush that would cut (barrier to cut line);
  C+ a barrier and its cut line last (cut line to the flush's commit): the
     cut not stored yet (shape C), then stored and the flush syncing.
The last column is what share of the kills that end on a barrier line
(what the check calls shape B) had no cut at stake."""
import os
import sys

root, arts = sys.argv[1], sys.argv[2:]
sys.path.insert(0, os.path.join(root, 'tests/kernel'))
import cut_daemon_check as c  # noqa: E402

for art in arts:
    with open(os.path.join(art, 'takeover', 'requests.tsv')) as f:
        inst = c.parse_log(f.read(), [])
    for k, ins in enumerate(inst[:2]):
        lines = ins['lines']
        life = lines[-1]['ns'] - lines[0]['ns']
        n = b = cplus = 0
        nn = nb = 0
        for i, ln in enumerate(lines[:-1]):
            if ln['event'] != 'barrier':
                continue
            nxt = lines[i + 1]
            if nxt['event'] == 'cut' and nxt['tag'] == ln['tag']:
                b += nxt['ns'] - ln['ns']
                nb += 1
                if i + 2 < len(lines):
                    cplus += lines[i + 2]['ns'] - nxt['ns']
            else:
                n += nxt['ns'] - ln['ns']
                nn += 1
        print(f"{os.path.basename(os.path.normpath(art))} instance {k + 1}: {life / 1e6:.0f} ms of lines, "
              f'{nn} flushes with no cut and {nb} with one; N {n / 1e6:.1f} ms ({100 * n / life:.2f}%), '
              f'B {b / 1e6:.3f} ms ({100 * b / life:.4f}%), C+ {cplus / 1e6:.1f} ms ({100 * cplus / life:.2f}%); '
              f'of the kills that end on a barrier line, {100 * n / (n + b):.2f}% had no cut at stake')
