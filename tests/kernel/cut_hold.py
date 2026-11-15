#!/usr/bin/env python3
"""The cut rig's holder (M2.T3, second stage): an uploader's stand-in
beside a recording on a daemon that holds cuts (`serve --hold-cuts`).

Usage: cut_hold.py OUT PID VOLUME_DIR RUN_DIR IMAGE VOLUME_BYTES STOP SKYZFSD

It talks to the daemon PID through the test build's commands about held
cuts (bins/skyzfsd/src/cut_commands.rs): a line in `cut-command` in the
volume's run directory, SIGUSR2, and the answer in `cut-reply`, whose
last line begins with the token the command began with. Until the file
STOP exists it loops:

- `held`; the cut being uploaded, if there is one, is written
  (`write N IMAGE`) onto IMAGE, the rig's image that the cuts are
  uploaded onto (sized to the volume before the first write: the
  command does not size it), and each block of the cut's map
  (IMAGE.blocks) is hashed as IMAGE then holds it;
- `release N`, as an upload's commit would.

While the file named by CUT_HOLD_PAUSE exists, it does nothing: the
cut being uploaded stays held (the recording `bound`).

A write the daemon refuses at its end (the cut was let go or abandoned
under it) is logged and not recorded: the cut that takes its map writes
those blocks again.

At STOP it records what the daemon holds (`held`, OUT/held-final.txt)
and, a second and a half later, the daemon's status beside it
(OUT/status-held.txt), so that the status's `cuts_held` can be held to
the list; then it drains: every cut still held is written and released
in turn. Then it hashes all of IMAGE once more (OUT/held-image.tsv,
the blocks that are not zeros), so that a block the daemon wrote
outside every map shows.

Out:
- OUT/held.tsv: one line per cut written, `number group opens blocks
  ns_begin ns_end`;
- OUT/held-blocks.tsv: `number block hash` for every block of its map,
  the hash being the first 8 bytes of SHA-256 in hex, as the request
  log's `applied` lines give them;
- OUT/holder.log: every command and reply.

Exit 0 when the drain ended with nothing held, 2 for the rig's own error
(a reply that never came, a daemon gone)."""
import hashlib
import os
import signal
import subprocess
import sys
import time

BLOCK = 4096
ZERO = hashlib.sha256(bytes(BLOCK)).digest()[:8].hex()
REPLY_S = 300           # a write of a whole 512 MiB volume, a slice a turn
DRAIN_S = 600


class HolderError(Exception):
    pass


def block_hash(data):
    return hashlib.sha256(data).digest()[:8].hex()


def run_dir_of(volume_dir, run_dir):
    """The volume's run directory, as the daemon names it (records.rs)."""
    st = os.stat(volume_dir)
    return os.path.join(run_dir, f'v-{st.st_dev:x}-{st.st_ino:x}')


def parse_reply(text, token):
    """A reply's text -> (ok, lines before the last, the last line's rest)
    when its last line is the token's; None while it is another's."""
    lines = text.splitlines()
    if not lines or not lines[-1].startswith(token + ' '):
        return None
    last = lines[-1][len(token) + 1:]
    if last.startswith('ok'):
        return True, lines[:-1], last
    return False, lines[:-1], last


def parse_held(lines):
    """`held`'s lines -> [{'number', 'group', 'phase', 'opens', 'blocks'}]."""
    out = []
    for ln in lines:
        f = ln.split()
        if len(f) != 5 or not (f[0].isdigit() and f[1].isdigit() and f[4].isdigit()) \
                or f[2] not in ('uploading', 'waiting', 'flushing') or f[3] not in ('itself', 'rewind'):
            raise HolderError(f'a held line that is not `number group phase opens blocks`: {ln!r}')
        out.append({'number': int(f[0]), 'group': int(f[1]), 'phase': f[2], 'opens': f[3], 'blocks': int(f[4])})
    return out


def hash_listed(image, blocks):
    """The hashes of IMAGE's blocks -> {block: hash}."""
    out = {}
    with open(image, 'rb') as f:
        for b in blocks:
            f.seek(b * BLOCK)
            data = f.read(BLOCK)
            if len(data) != BLOCK:
                raise HolderError(f'{image}: block {b} is past its end ({len(data)} bytes read)')
            out[b] = block_hash(data)
    return out


def hash_all(image):
    """{block: hash} of every block of IMAGE that is not zeros."""
    out = {}
    with open(image, 'rb') as f:
        b = 0
        while True:
            data = f.read(BLOCK * 256)
            if not data:
                break
            for k in range(0, len(data), BLOCK):
                h = block_hash(data[k:k + BLOCK])
                if h != ZERO:
                    out[b + k // BLOCK] = h
            b += len(data) // BLOCK
    return out


def read_blocks(path):
    with open(path) as f:
        out = []
        for ln in f:
            ln = ln.strip()
            if not ln.isdigit():
                raise HolderError(f'{path}: a line that is not a block number: {ln!r}')
            out.append(int(ln))
    return out


class Daemon:
    """The commands about held cuts of daemon PID in its volume's run directory."""

    def __init__(self, pid, vrun, log, clock=time.monotonic, sleep=time.sleep):
        self.pid, self.vrun, self.log = pid, vrun, log
        self.clock, self.sleep = clock, sleep
        self.n = 0

    def send(self, command, limit=REPLY_S):
        """-> (ok, lines, last) of the reply to COMMAND."""
        self.n += 1
        token = f't{os.getpid()}x{self.n}'
        tmp = os.path.join(self.vrun, 'cut-command.tmp')
        with open(tmp, 'w') as f:
            f.write(f'{token} {command}\n')
        os.rename(tmp, os.path.join(self.vrun, 'cut-command'))
        self.log.write(f'{time.monotonic_ns()}\t> {token} {command}\n')
        # One signal: the file is in place before it, and the daemon
        # reads the file at its next turn. A second signal during a write
        # would read the command again and be answered busy under this
        # token.
        try:
            os.kill(self.pid, signal.SIGUSR2)
        except ProcessLookupError:
            raise HolderError(f'the daemon {self.pid} is gone') from None
        end = self.clock() + limit
        while True:
            now = self.clock()
            try:
                with open(os.path.join(self.vrun, 'cut-reply')) as f:
                    got = parse_reply(f.read(), token)
            except FileNotFoundError:
                got = None
            if got is not None:
                self.log.write(f'{time.monotonic_ns()}\t< {token} {got[2]} ({len(got[1])} lines)\n')
                self.log.flush()
                return got
            if now >= end:
                raise HolderError(f'no reply to `{command}` in {limit} s')
            self.sleep(0.02)


class Holder:
    def __init__(self, daemon, image, out):
        self.d, self.image, self.out = daemon, image, out
        self.written = []

    def held(self):
        ok, lines, last = self.d.send('held')
        if not ok:
            raise HolderError(f'held refused: {last}')
        return parse_held(lines)

    def step(self):
        """Write and release the cut being uploaded -> whether there was one."""
        up = [c for c in self.held() if c['phase'] == 'uploading']
        if not up:
            return False
        c = up[0]
        t0 = time.monotonic_ns()
        ok, _, last = self.d.send(f"write {c['number']} {self.image}")
        t1 = time.monotonic_ns()
        if ok:
            blocks = read_blocks(self.image + '.blocks')
            hashes = hash_listed(self.image, blocks)
            with open(os.path.join(self.out, 'held-blocks.tsv'), 'a') as f:
                f.write(''.join(f"{c['number']}\t{b}\t{hashes[b]}\n" for b in blocks))
            with open(os.path.join(self.out, 'held.tsv'), 'a') as f:
                f.write(f"{c['number']}\t{c['group']}\t{c['opens']}\t{len(blocks)}\t{t0}\t{t1}\n")
            self.written.append(c['number'])
        # A write refused at its end (let go or abandoned under it) is
        # not recorded; the release says whether it is still held.
        ok, _, last = self.d.send(f"release {c['number']}")
        return True


def main():
    if len(sys.argv) != 9:
        sys.exit(__doc__)
    out, pid, vdir, rdir, image, size, stop, skyzfsd = sys.argv[1:]
    pid, size = int(pid), int(size)
    os.makedirs(out, exist_ok=True)
    rc = 0
    with open(os.path.join(out, 'holder.log'), 'a') as log:
        try:
            with open(image, 'ab') as f:
                f.truncate(size)
            d = Daemon(pid, run_dir_of(vdir, rdir), log)
            h = Holder(d, image, out)
            pause = os.environ.get('CUT_HOLD_PAUSE', '')
            while not os.path.exists(stop):
                if (pause and os.path.exists(pause)) or not h.step():
                    time.sleep(0.2)
            # What is held at the stop, and the status a second and a
            # half after (the daemon writes its status at most every
            # 100 ms, when a counter changed).
            ok, lines, last = d.send('held')
            with open(os.path.join(out, 'held-final.txt'), 'w') as f:
                f.write(''.join(ln + '\n' for ln in lines) + ('' if ok else f'refused: {last}\n'))
            time.sleep(1.5)
            with open(os.path.join(out, 'status-held.txt'), 'w') as f:
                subprocess.run([skyzfsd, 'status', '--dir', vdir, '--run-dir', rdir], stdout=f,
                               stderr=subprocess.STDOUT, timeout=30, check=False)
            end = time.monotonic() + DRAIN_S
            while h.step():
                if time.monotonic() > end:
                    raise HolderError(f'still holding cuts after a drain of {DRAIN_S} s')
            left = h.held()
            if left:
                raise HolderError(f'after the drain the daemon still holds {left}')
            with open(os.path.join(out, 'held-image.tsv'), 'w') as f:
                f.write(''.join(f'{b}\t{v}\n' for b, v in sorted(hash_all(image).items())))
            log.write(f'{time.monotonic_ns()}\tdrained: {len(h.written)} cuts written\n')
        except (HolderError, OSError, subprocess.SubprocessError) as e:
            log.write(f'{time.monotonic_ns()}\tERROR {type(e).__name__}: {e}\n')
            print(f'cut_hold: {e}', file=sys.stderr)
            rc = 2
    sys.exit(rc)


if __name__ == '__main__':
    main()
