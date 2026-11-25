#!/usr/bin/env python3
"""The cut rig's watcher (M2.T3, the daemon's uploads, M2.4c): node B
beside a recording on a daemon that uploads its cuts to object storage.

Usage: cut_watch.py OUT NODEB ENDPOINT BUCKET VOLUME STOP

NODEB is bins/skyzfs-nodeb; the keys are AWS_ACCESS_KEY_ID and
AWS_SECRET_ACCESS_KEY. The watcher knows only the bucket, as node B
does. Until the file STOP exists it loops, twice a second:

- `row`: head's commit, the cut's row (`skyzfs/cut`: the cut state's
  name, the cut's number and group, which the uploader writes in the
  same transaction as the head swap; from row version 2, F1, also the
  commit that head swap installed) and the lease. Each line but the
  row's commit goes to OUT/rows.tsv with its time;
- where head and the row have changed since the last cut fetched and
  the row names a cut, `get --ref head` into a scratch image, then `row`
  again: where head and the row are what they were before the read, the
  image is the cut the row names. Every 4 KiB block of it is hashed,
  and what changed since the cut fetched before (a block now zeros has
  the hash of zeros) is written as that cut's blocks.

While the file CUT_WATCH_PAUSE names (an environment variable) exists,
the watcher calls nothing: the rig has object storage down on purpose.
At STOP it fetches once more, so that the cut head holds at the end is
in the files, and ends. Out:

- OUT/uploaded.tsv: one line per cut fetched, `number group state head
  blocks ns_begin ns_end commit` (the blocks that are not zeros; the
  times bracket the fetch, on the monotonic clock of the request log;
  commit the one the row names, or `none` for a row of version 1 or a
  node B that prints none);
- OUT/uploaded-blocks.tsv: `fetch number block hash` for each block that
  changed since the cut fetched before, fetch being the line of
  uploaded.tsv (from 0; a cut another writer put at head can carry a
  number of the daemon's) and the hash the first 8 bytes of SHA-256 in
  hex, as the request log's `applied` lines give them;
- OUT/rows.tsv: `ns head row lease` as `row` printed them;
- OUT/watch.log: what it did, and a line `VERDICT ...` where node B
  could not read the content of the cut head named (`get` exit 4: the
  bucket does not hold what the commit needs). The watcher then ends
  with 1. Any other failure of one of node B's commands three times in a
  row (each command counted on its own, so that a `row` that works does
  not hide a `get` that fails), a last look that could not fetch what
  head holds, or a failure of the watcher's own files, ends it with 2,
  the rig's error.
"""
import hashlib
import os
import subprocess
import sys
import time

BLOCK = 4096
ZERO = hashlib.sha256(bytes(BLOCK)).digest()[:8].hex()
TRIES = 3


class RigError(Exception):
    pass


class Verdict(Exception):
    pass


def hash_image(path):
    """{block: hash} of every block of the file that is not zeros."""
    out = {}
    with open(path, 'rb') as f:
        k = 0
        while True:
            b = f.read(BLOCK * 256)
            if not b:
                break
            for j in range(0, len(b), BLOCK):
                blk = b[j:j + BLOCK]
                if blk.count(0) != len(blk):
                    out[k] = hashlib.sha256(blk.ljust(BLOCK, b'\0')).digest()[:8].hex()
                k += 1
    return out


def changed(before, after):
    """The blocks of after that differ from before, with the hash of zeros
    for a block that was not zeros and is now."""
    out = {b: h for b, h in after.items() if before.get(b) != h}
    for b in before:
        if b not in after:
            out[b] = ZERO
    return out


def parse_row(text):
    """`row`'s output -> {'head', 'row', 'lease', 'commit'}: each the
    line's fields after its first, joined by spaces; commit the row's
    (`row_commit`), 'none' where node B printed none."""
    got = {}
    for ln in text.splitlines():
        f = ln.split('\t')
        if f and f[0] in ('head', 'row', 'lease', 'row_commit'):
            got[f[0]] = ' '.join(f[1:])
    commit = got.pop('row_commit', 'none')
    if set(got) != {'head', 'row', 'lease'}:
        raise RigError(f'node B row printed {text!r}')
    return dict(got, commit=commit)


def row_cut(row):
    """A row's text -> (state, number, group), or None where it names no cut."""
    f = row.split()
    if len(f) == 3 and f[1].isdigit() and f[2].isdigit():
        return f[0], int(f[1]), int(f[2])
    return None


class Watcher:
    def __init__(self, out, nodeb, endpoint, bucket, volume):
        self.out, self.nodeb = out, nodeb
        self.base = ['--endpoint', endpoint, '--bucket', bucket, '--volume', volume]
        self.image = os.path.join(out, 'watch.img')
        self.last = None        # (head, row) of the last cut fetched
        self.blocks = {}        # its blocks that are not zeros
        self.failures = {}      # node B's command -> its failures in a row
        self.settled = False    # the last look found nothing left to fetch
        self.fetched = 0        # lines of uploaded.tsv

    def log(self, text):
        with open(os.path.join(self.out, 'watch.log'), 'a') as f:
            f.write(f'{time.monotonic_ns()}\t{text}\n')

    def run(self, cmd, *args):
        p = subprocess.run([self.nodeb, cmd] + self.base + list(args), capture_output=True, text=True,
                           timeout=300)
        return p.returncode, p.stdout, p.stderr.strip()

    def node_b(self, cmd, *args):
        """node B's command -> its output, or None after a failure that is
        not yet the third in a row."""
        rc, out, err = self.run(cmd, *args)
        if rc == 4 and cmd == 'get':
            raise Verdict(f'node B could not read head: {err}')
        if rc != 0:
            self.failures[cmd] = self.failures.get(cmd, 0) + 1
            self.log(f'node B {cmd} ended with {rc}: {err}')
            if self.failures[cmd] >= TRIES:
                raise RigError(f'node B {cmd} failed {TRIES} times in a row: {err}')
            return None
        self.failures[cmd] = 0
        return out

    def row(self):
        out = self.node_b('row')
        if out is None:
            return None
        got = parse_row(out)
        with open(os.path.join(self.out, 'rows.tsv'), 'a') as f:
            f.write(f"{time.monotonic_ns()}\t{got['head']}\t{got['row']}\t{got['lease']}\n")
        return got

    def step(self):
        """One look -> True where a cut was fetched. settled says whether
        the look left nothing to fetch: the row names no cut, its cut was
        fetched before, or now."""
        self.settled = False
        before = self.row()
        if before is None:
            return False
        if row_cut(before['row']) is None or before['head'] == 'none':
            self.settled = True
            return False
        key = (before['head'], before['row'], before['commit'])
        if key == self.last:
            self.settled = True
            return False
        t0 = time.monotonic_ns()
        if self.node_b('get', '--ref', 'head', '--out', self.image) is None:
            return False
        t1 = time.monotonic_ns()
        after = self.row()
        if after is None or (after['head'], after['row'], after['commit']) != key:
            self.log(f'head moved under the read: {key} then '
                     f"{(after['head'], after['row'], after['commit']) if after else 'nothing'}")
            return False
        blocks = hash_image(self.image)
        state, number, group = row_cut(before['row'])
        diff = changed(self.blocks, blocks)
        with open(os.path.join(self.out, 'uploaded-blocks.tsv'), 'a') as f:
            f.write(''.join(f'{self.fetched}\t{number}\t{b}\t{h}\n' for b, h in sorted(diff.items())))
        with open(os.path.join(self.out, 'uploaded.tsv'), 'a') as f:
            f.write(f"{number}\t{group}\t{state}\t{before['head'].split()[0]}\t{len(blocks)}\t{t0}\t{t1}\t"
                    f"{before['commit']}\n")
        self.log(f'fetched cut {number} (group {group}, state {state}): {len(blocks)} blocks, {len(diff)} changed')
        self.last, self.blocks = key, blocks
        self.fetched += 1
        self.settled = True
        return True

    def finish(self):
        """The last look, at STOP: the cut head holds at the end is in the
        files, whatever came before, or the rig's error."""
        for _ in range(TRIES):
            self.step()
            if self.settled:
                return
        raise RigError(f'the last look did not fetch what head holds in {TRIES} tries')


def main():
    if len(sys.argv) != 7:
        sys.exit(__doc__)
    out, nodeb, endpoint, bucket, volume, stop = sys.argv[1:]
    w = Watcher(out, nodeb, endpoint, bucket, volume)
    pause = os.environ.get('CUT_WATCH_PAUSE', '')
    try:
        while not os.path.exists(stop):
            if not (pause and os.path.exists(pause)):
                w.step()
            time.sleep(0.5)
        w.finish()
        w.log('stopped')
    except Verdict as e:
        w.log(f'VERDICT {e}')
        return 1
    except (RigError, OSError, subprocess.SubprocessError) as e:
        w.log(f'ERROR {e}')
        return 2
    finally:
        try:
            os.unlink(w.image)
        except OSError:
            pass
    return 0


if __name__ == '__main__':
    sys.exit(main())
