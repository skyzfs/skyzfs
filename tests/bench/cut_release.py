#!/usr/bin/env python3
"""A releaser of held cuts beside a benchmark row (M2.T4, round 2): it
lets the daemon's cuts go as an upload's commit would, so that the
store's intent map clears its bits and a write into a region a cut has
committed waits for the map again, as it will once M2.4 uploads.

Usage: cut_release.py OUT PID VOLUME_DIR RUN_DIR EVERY STOP

It talks to daemon PID through the test build's commands about held
cuts, as the cut rig's holder does (tests/kernel/cut_hold.py: a line in
`cut-command`, SIGUSR2, the answer in `cut-reply`). Only a build with
the test hooks has them (`skyzfsd --features test-hooks`); a release
build answers nothing, and the releaser ends with its error.

Until the file STOP exists it asks `held` every POLL_S seconds and
releases the cut being uploaded once EVERY cuts have been taken since
it, counting it: EVERY 1 releases each cut as soon as it is being
uploaded, EVERY 4 holds each one until three more have been cut. The
daemon keeps one cut waiting behind the one being uploaded, and a newer
cut replaces it, so with EVERY above 1 the cuts in between are never
uploaded, as with an upload slower than the pool. It writes nothing out:
what it measures is the store's side of a release, not an upload's
reads.

Out: OUT, one line per release, `ns number newest`, with the monotonic
time the release was answered; and, at the end, a line `# released N,
polls P` or `# ERROR ...`.

Exit 0 at STOP, 2 for its own error (the daemon gone, no reply, a
refusal)."""
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'kernel'))
import cut_hold as ch  # noqa: E402

POLL_S = 0.1
# How long a reply may take (CUT_RELEASE_REPLY_S): a release build never
# answers, and `held` and `release` are quick in a test build.
REPLY_S = float(os.environ.get('CUT_RELEASE_REPLY_S', '30'))


def due(held, every):
    """`held`'s cuts -> the number of the cut to release now, or None."""
    up = [c for c in held if c['phase'] == 'uploading']
    if not up:
        return None
    newest = max(c['number'] for c in held)
    return up[0]['number'] if newest - up[0]['number'] >= every - 1 else None


class Releaser:
    def __init__(self, daemon, every, out):
        self.d, self.every, self.out = daemon, every, out
        self.released = self.polls = 0

    def held(self):
        ok, lines, last = self.d.send('held', REPLY_S)
        if not ok:
            raise ch.HolderError(f'held refused: {last}')
        return ch.parse_held(lines)

    def step(self):
        """One poll -> whether a cut was released."""
        self.polls += 1
        held = self.held()
        n = due(held, self.every)
        if n is None:
            return False
        ok, _, last = self.d.send(f'release {n}', REPLY_S)
        if not ok:
            raise ch.HolderError(f'release {n} refused: {last}')
        self.released += 1
        self.out.write(f"{time.monotonic_ns()}\t{n}\t{max(c['number'] for c in held)}\n")
        self.out.flush()
        return True


def main():
    if len(sys.argv) != 7:
        sys.exit(__doc__)
    out, pid, vdir, rdir, every, stop = sys.argv[1:]
    if not every.isdigit() or int(every) < 1:
        sys.exit(f'cut_release: EVERY must be a whole number from 1, not {every!r}')
    rc = 0
    with open(out, 'a') as f, open(out + '.log', 'a') as log:
        r = Releaser(ch.Daemon(int(pid), ch.run_dir_of(vdir, rdir), log), int(every), f)
        try:
            while not os.path.exists(stop):
                if not r.step():
                    time.sleep(POLL_S)
            f.write(f'# released {r.released}, polls {r.polls}\n')
        except (ch.HolderError, OSError) as e:
            f.write(f'# ERROR {type(e).__name__}: {e} (released {r.released}, polls {r.polls})\n')
            print(f'cut_release: {e}', file=sys.stderr)
            rc = 2
    sys.exit(rc)


if __name__ == '__main__':
    main()
