#!/usr/bin/env python3
"""The agent's side of a publish, stood in for (M2.T3, the channel rig).

The agent proper is M2.6's. This stand-in follows the daemon's channel
as `crates/skyzfs-control/src/lib.rs`'s crate documentation gives it
(protocol version 3, M2.7b, or version 2 before it) and design §5.2, "A
snapshot is published by a cut of the pool's present history":

1. `cuts`: the attachment A, the cut state S and the count C;
2. `zfs snapshot POOL@T`, then `zfs hold` on it; G is its `createtxg`;
3. `publish name=T attachment=A state=S after=C group=G manifest=N`,
   with N bytes of a manifest (the snapshot's files and their SHA-256,
   which the rig's check compares with what node B reads);
4. `progress name=T` until it is no longer `pending`, syncing the pool
   a group at a time when asked (`--sync`: 4 KiB written to
   MOUNTPOINT/publish-nudge, then `zpool sync`; after a rewind import no cut
   opens as itself until the pool's groups have passed the abandoned
   ones, and an idle pool makes no next cut), or as the agent's assist
   does (`--assist`, below);
5. `zfs release`.

`--assist` stands in for the agent's `Zfs::assist` (M2.6b2, #144) at
the pace the lead decided on #144: once the publish has waited
ASSIST_AFTER (`--assist-after-s`, 10 s by default) since it was
accepted, ASSIST_BURST (3) assists a round (a second) apart, then one
every ASSIST_AFTER. An assist is `zfs set skyzfs:assist=V` on the
pool's root with a V not set before (the wall clock's ms, or one above
the last), then `zpool sync`, with nothing written to the pool. Each assist is a
line of DIR/assist.tsv: T, the monotonic ns before the set and after
the sync, V, and the newest group that wrote, by the pool's txgs kstat
(committed, `nwritten` above 0), before the set, after the set, and
after the sync (read until it stays put). ZFS 2.4 also commits groups
that write nothing and no uberblock (the implementation notes, M2.T1):
those are no cut, and they come every few seconds on an idle pool, so
they are not counted.

With `--assist`, DIR/accepted.tsv has two lines for T, read from the
kstat whatever the daemon answered: `T accepted NS W A` once the
publish is accepted, and `T end NS W VERB` when its end is read (NS the
monotonic ns, W the newest group that wrote, A the assist's wait in
seconds, VERB the end's first word: `done`, `pending`, `refused`, ...).
The check's premises rest on these and on assist.tsv, and not on the
daemon's answers (#145's review, F1).

A request refused `rewound` (a rewind, or a daemon's start, may have been
counted after C) is asked anew by the rig's courses with a count read
anew (`ask cuts`), as the agent's AskAnew does; `ask look T` reads
`snap/T` as the daemon finds it (`snap ...`, `absent`, `unreadable ...`).
`ask retire T H` asks the daemon to retire `snap/T` where it names the
commit H (version 3, M2.7b-1): `retired`, `gone` where there is no
`snap/T`, `refused other COMMIT` where it names another.

The versions (HELLOS). Version 3 is version 2 with `retire` and the
store's rewind mark in the answer to `cuts` (`rewound=R`), and each
side refuses the other version at the greeting (`version`). The
stand-in reads the answers of either, so that one rig runs on the
daemon before and after M2.7b: `cuts` with or without `rewound=`, and a
raw request's answer after either greeting. Its requests through
`skyzfsd`'s commands speak the version of that build. A raw request
whose greeting is `skyzfs-control ?` offers version 3, and where the
daemon refuses it `version` with the greeting of version 2, is sent
again with that greeting on a new connection (two requests to such a
daemon, each in AGENT.tsv); `raw`'s GREETING_OUT keeps the greeting
the daemon answered with, so that a course can learn the daemon's
version from one request and send the next ones in it.

Every request and its answer goes to AGENT.tsv, one line each, with
the monotonic clock (the request log's) before and after: the check
reads what was asked and answered from it, and nothing else.

Usage:
  publish_agent.py flow  SKYZFSD VOL RUN AGENT.tsv POOL T [--sync] [--expiry-s E]
                         [--limit-s S] [--manifest-out FILE] [--no-hold]
                         [--group-plus K] [--twice] [--assist]
                         [--assist-after-s A]
  publish_agent.py ask   SKYZFSD VOL RUN AGENT.tsv cuts
  publish_agent.py ask   SKYZFSD VOL RUN AGENT.tsv progress|withdraw|look T
  publish_agent.py ask   SKYZFSD VOL RUN AGENT.tsv retire T H
  publish_agent.py ask   SKYZFSD VOL RUN AGENT.tsv publish T A S C G MANIFEST [E]
  publish_agent.py raw   RUN AGENT.tsv LABEL [GREETING_OUT]  (stdin: the bytes to send,
                         as given, but for a first line `skyzfs-control ?`)

`--group-plus K` asks with G+K in place of the snapshot's createtxg G
(the rig's course on the group: no cut below G+K may be published);
`--twice` sends the same publish twice before the first look (asking
twice is asking once). Each publish asked is a line of DIR/published.tsv
(DIR: AGENT.tsv's): T, the G asked, A, S, C, the manifest's path, and
the snapshot's createtxg.

`flow` prints the publish's end (`done commit=H`, `expired`, ...) and
exits 0 when it was answered at all, 1 when a request was refused (the
refusal printed), 2 on the rig's own error. `ask` prints the answer
line and exits 0, or 1 for a refusal; 2 when no answer came. `raw`
prints the answer line and exits 0 whatever it says (the check judges
it), `no answer within N s` among them when the daemon did not answer
within the limit once connected; 2 when the socket could not be
reached.
"""
import hashlib
import os
import socket
import subprocess
import sys
import time

# The greetings the stand-in reads, the one it offers first.
HELLOS = ('skyzfs-control 3', 'skyzfs-control 2')
HELLO = HELLOS[0]
OFFER = b'skyzfs-control ?\n'
CUTS_FIELDS = {'attachment', 'state', 'count', 'group', 'opens'}
ASSIST_PROP = 'skyzfs:assist'
ASSIST_AFTER = 10
ASSIST_BURST = 3
ENDS = ('done', 'expired', 'withdrawn', 'unknown')


class RigError(Exception):
    """The stand-in could not do its part: not a verdict on the daemon."""


def parse_answer(line):
    """An answer line -> (verb, {key: value}, refusal code, refusal text).

    `cuts attachment=A ...` -> ('cuts', {...}, None, None);
    `refused CODE TEXT` -> ('refused', {}, CODE, TEXT). A line that is
    none of the protocol's is the rig's error: the daemon answered
    something the stand-in cannot read."""
    line = line.rstrip('\n')
    if line.startswith('refused '):
        rest = line[len('refused '):]
        code, _, text = rest.partition(' ')
        if not code:
            raise RigError(f'a refusal with no code: {line!r}')
        return 'refused', {}, code, text
    words = line.split(' ')
    if not words or not words[0] or any(not w for w in words):
        raise RigError(f'not an answer line: {line!r}')
    verb, fields = words[0], {}
    for w in words[1:]:
        k, sep, v = w.partition('=')
        if not sep or not k or k in fields:
            raise RigError(f'not an answer line: {line!r}')
        fields[k] = v
    known = {'cuts': CUTS_FIELDS, 'accepted': set(), 'pending': set(),
             'done': {'commit'}, 'expired': set(), 'withdrawn': set(), 'unknown': set(),
             'snap': {'commit', 'state', 'number', 'group', 'manifest-len', 'manifest-hash'}, 'absent': set(),
             'unreadable': {'commit', 'why'}, 'retired': set(), 'gone': set()}
    # Version 3's `cuts` carries the rewind mark, version 2's does not.
    if verb == 'cuts' and 'rewound' in fields:
        if not is_number(fields['rewound']):
            raise RigError(f'a cuts answer whose rewound is not a number: {line!r}')
        if set(fields) != CUTS_FIELDS | {'rewound'}:
            raise RigError(f'not an answer line: {line!r}')
    elif verb not in known or set(fields) != known[verb]:
        raise RigError(f'not an answer line: {line!r}')
    if verb in ('done', 'snap', 'unreadable') and not is_hex(fields['commit'], 64):
        raise RigError(f'a commit that is not 64 hex digits: {line!r}')
    if verb == 'snap' and not (is_hex(fields['state'], 32) and is_hex(fields['manifest-hash'], 64)
                               and all(is_number(fields[k]) for k in ('number', 'group', 'manifest-len'))):
        raise RigError(f'a snap answer whose fields do not read: {line!r}')
    if verb == 'cuts':
        if not is_hex(fields['attachment'], 32) or not (fields['state'] == 'none' or is_hex(fields['state'], 32)):
            raise RigError(f'a cuts answer whose attachment or state is not 32 hex digits: {line!r}')
        if not is_number(fields['count']):
            raise RigError(f'a cuts answer whose count is not a number: {line!r}')
        if (fields['group'] == 'none') != (fields['opens'] == 'none') or (
                fields['group'] != 'none' and (not is_number(fields['group'])
                                               or fields['opens'] not in ('itself', 'rewind'))):
            raise RigError(f'a cuts answer whose group and opens do not go together: {line!r}')
    return verb, fields, None, None


def is_number(text):
    """ASCII digits only: str.isdigit() also takes `²` and other digits
    that int() refuses or the daemon never writes."""
    return text.isascii() and text.isdigit()


def is_hex(text, n):
    return len(text) == n and all(c in '0123456789abcdef' for c in text)


def manifest_of(root):
    """The snapshot's files and their SHA-256, sorted: what node B must find.
    `root` is the snapshot's directory (`.zfs/snapshot/T`)."""
    lines = []
    for fn in sorted(os.listdir(root)):
        p = os.path.join(root, fn)
        if not os.path.isfile(p):
            continue
        h = hashlib.sha256()
        with open(p, 'rb') as f:
            for b in iter(lambda: f.read(1 << 20), b''):
                h.update(b)
        lines.append(f'{h.hexdigest()}  {fn}\n')
    return ''.join(lines).encode()


class Agent:
    """The channel through `skyzfsd cuts|publish|publish-progress|withdraw|look|retire`."""

    def __init__(self, skyzfsd, vol, run, log, clock=time.monotonic_ns, runner=subprocess.run):
        self.skyzfsd, self.vol, self.run, self.log = skyzfsd, vol, run, log
        self.clock, self.runner = clock, runner

    def record(self, t0, t1, request, rc, answer):
        with open(self.log, 'a') as f:
            f.write(f'{t0}\t{t1}\t{request}\t{rc}\t{answer}\n')

    def ask(self, command, *args):
        """-> (verb, fields, code, text); a refusal is an answer too."""
        cmd = [self.skyzfsd, command, '--dir', self.vol, '--run-dir', self.run, *args]
        t0 = self.clock()
        try:
            p = self.runner(cmd, capture_output=True, text=True, timeout=120)
        except subprocess.TimeoutExpired:
            self.record(t0, self.clock(), ' '.join([command, *args]), 124, '')
            raise RigError(f'skyzfsd {command} did not answer in 120 s') from None
        t1 = self.clock()
        line = p.stdout.strip().splitlines()[-1] if p.stdout.strip() else ''
        self.record(t0, t1, ' '.join([command, *args]), p.returncode, line)
        if p.returncode not in (0, 1) or not line:
            raise RigError(f'skyzfsd {command} ended {p.returncode} with no answer: {p.stderr.strip()[-300:]}')
        answer = parse_answer(line)
        if (answer[0] == 'refused') != (p.returncode == 1):
            raise RigError(f'skyzfsd {command} exit {p.returncode} with the answer {line!r}')
        return answer

    def cuts(self):
        return self.ask('cuts')

    def publish(self, name, attachment, state, after, group, manifest, expiry=None):
        args = ['--name', name, '--attachment', attachment, '--state', state, '--after', str(after),
                '--group', str(group), '--manifest', manifest]
        if expiry is not None:
            args += ['--expiry-s', str(expiry)]
        return self.ask('publish', *args)

    def progress(self, name):
        return self.ask('publish-progress', '--name', name)

    def withdraw(self, name):
        return self.ask('withdraw', '--name', name)

    def look(self, name):
        return self.ask('look', '--name', name)

    def retire(self, name, commit):
        """`retired`, `gone`, or `refused other COMMIT` (M2.7b-1)."""
        return self.ask('retire', '--name', name, '--commit', commit)


def raw(run, log, label, data, clock=time.monotonic_ns, limit=60, greeting_out=None):
    """Send `data` to the volume's socket (the one `control` under RUN)
    and read the answer: for the requests that `skyzfsd`'s commands would
    not send (another greeting, a malformed line). Data whose first line
    is `skyzfs-control ?` is sent with HELLO in its place, and once more,
    on a new connection, with the greeting of version 2 where the daemon
    refused it `version` with that greeting (the version it speaks). With
    GREETING_OUT, the greeting the daemon answered the last request with
    is written there (nothing where it wrote no whole line).

    A daemon that refuses before it has read the whole request (a peer
    of another user) answers and closes with the request unread. On
    Linux the write can then fail, and the read after the answer fails
    with ECONNRESET and not EOF. As `skyzfsd`'s own `ask` does, the
    answer is read all the same.

    The answer is the whole line (ended by a newline) after the greeting
    of HELLOS (`skyzfs-control 3` or `2`), as `ask` takes it: anything else the daemon wrote
    before it closed, a line with no greeting before it among them, is
    answered `no answer line: ...`, which no course allows. Once the
    connection is made, a daemon that closes with nothing written (EOF,
    a reset or a broken pipe) answers `''`, and one that writes nothing
    and does not close within LIMIT seconds answers `no answer within
    LIMIT s`: neither is allowed, and each is the daemon's fault and a
    verdict, whether it read the request or not (the error goes to
    stderr). The rig's error (RigError, exit 2) is a socket it cannot
    connect to, a connect that times out, and anything else."""
    if not data.startswith(OFFER):
        return raw_once(run, log, label, data, clock, limit, greeting_out)
    rest = data[len(OFFER):]
    for k, hello in enumerate(HELLOS):
        answer = raw_once(run, log, label, hello.encode() + b'\n' + rest, clock, limit, greeting_out)
        said = last_greeting.get('said')
        if not (answer.startswith('refused version') and said in HELLOS[k + 1:]):
            return answer
    return answer


# The greeting the daemon wrote first, in raw_once's last request.
last_greeting = {}


def raw_once(run, log, label, data, clock, limit, greeting_out):
    """One request of `raw`, as given."""
    path = socket_path(run)
    t0 = clock()
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(limit)
    got = b''
    err = None
    late = False
    try:
        s.connect(path)
        try:
            s.sendall(data)
            s.shutdown(socket.SHUT_WR)
        except (BrokenPipeError, ConnectionResetError) as e:
            err = e
        while True:
            try:
                b = s.recv(65536)
            except ConnectionResetError as e:
                err = err or e
                break
            except TimeoutError:
                late = True
                break
            if not b:
                break
            got += b
        if late:
            print(f'raw {label}: the daemon did not close within {limit} s ({len(got)} bytes written)', file=sys.stderr)
        elif not got and err is not None:
            print(f'raw {label}: the daemon closed with nothing written: {err}', file=sys.stderr)
    except OSError as e:
        with open(log, 'a') as f:
            f.write(f'{t0}\t{clock()}\traw {label}\t2\t{e}\n')
        raise RigError(f'the socket {path}: {e}') from None
    finally:
        s.close()
    answer = f'no answer within {limit} s' if late and not got else answer_of(got)
    said = got.decode('utf-8', 'replace').split('\n')[0] if b'\n' in got else None
    last_greeting['said'] = said
    if greeting_out is not None:
        with open(greeting_out, 'w') as f:
            f.write(f'{said}\n' if said is not None else '')
    with open(log, 'a') as f:
        f.write(f'{t0}\t{clock()}\traw {label}\t{1 if answer.startswith("refused ") else 0}\t{answer}\n')
    return answer


def answer_of(got):
    """The bytes a daemon wrote -> the answer: the first whole line after
    the greeting, '' for nothing at all, and `no answer line: ...` for
    anything else: bytes with no whole line after a whole greeting, a
    first line that is not a greeting of HELLOS (skyzfsd's own `ask`
    refuses it)."""
    text = got.decode('utf-8', 'replace')
    whole = text.split('\n')[:-1]
    if len(whole) > 1 and whole[0] in HELLOS:
        return whole[1]
    if not text:
        return ''
    return 'no answer line: ' + text.replace('\\', '\\\\').replace('\n', '\\n').replace('\t', '\\t')[:120]


def socket_path(run):
    """The socket `control` under RUN, or RUN itself where it is one."""
    if os.path.basename(run) == 'control' and os.path.exists(run):
        return run
    found = []
    for d, _, files in os.walk(run):
        if 'control' in files:
            found.append(os.path.join(d, 'control'))
    if len(found) != 1:
        raise RigError(f'not one socket `control` under {run}: {found}')
    return found[0]


def write_nudge(path):
    with open(path, 'wb') as f:
        f.write(os.urandom(4096))


def sh(cmd, timeout=120):
    p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    if p.returncode:
        raise RigError(f'{" ".join(cmd)}: exit {p.returncode}: {(p.stderr or p.stdout).strip()[-300:]}')
    return p.stdout


def written_txg(pool, root='/proc/spl/kstat/zfs'):
    """The newest group the pool's txgs kstat says it committed (state C)
    with something written (`nwritten`, the sixth column, above 0)."""
    path = os.path.join(root, pool, 'txgs')
    try:
        with open(path) as f:
            rows = f.read().splitlines()
    except OSError as e:
        raise RigError(f'{path}: {e}') from e
    best = None
    for ln in rows[1:]:
        f = ln.split()
        if len(f) >= 6 and f[0].isdigit() and f[2] == 'C' and f[5].isdigit() and int(f[5]) > 0:
            best = max(best or 0, int(f[0]))
    if best is None:
        raise RigError(f'{path}: no group that wrote in its history')
    return best


def settled(pool, synced, sleep, tries=10):
    """The pool's newest group that wrote, once two reads 0.1 s apart
    agree (the kstat may lag the command that waited for the group)."""
    prev = synced(pool)
    for _ in range(tries):
        sleep(0.1)
        cur = synced(pool)
        if cur == prev:
            return cur
        prev = cur
    return prev


def assist(pool, name, out_dir, last, shell, synced, wall, mono, sleep):
    """One assist, as the agent's `Zfs::assist` makes it -> the value set."""
    value = max(wall() // 1_000_000, last + 1)
    before = synced(pool)
    t0 = mono()
    shell(['zfs', 'set', f'{ASSIST_PROP}={value}', pool])
    mid = synced(pool)
    shell(['zpool', 'sync', pool])
    t1 = mono()
    after = settled(pool, synced, sleep)
    with open(os.path.join(out_dir, 'assist.tsv'), 'a') as f:
        f.write(f'{name}\t{t0}\t{t1}\t{value}\t{before}\t{mid}\t{after}\n')
    return value


def flow(agent, pool, name, sync=False, expiry=None, limit=300, manifest_out=None, hold=True, sleep=time.sleep,
         shell=None, files=manifest_of, now=time.monotonic, group_plus=0, twice=False, nudge=None,
         assist_after=None, synced=written_txg, wall=time.time_ns, mono=time.monotonic_ns):
    """One publish, as design §5.2 has the agent make it -> its end line."""
    sh_ = shell or sh
    verb, cuts, code, text = agent.cuts()
    if verb == 'refused':
        return f'refused {code} {text}'
    sh_(['zfs', 'snapshot', f'{pool}@{name}'])
    if hold:
        sh_(['zfs', 'hold', 'skyzfs-publish', f'{pool}@{name}'])
    try:
        txg = sh_(['zfs', 'get', '-Hp', '-o', 'value', 'createtxg', f'{pool}@{name}']).strip()
        if not txg.isdigit():
            raise RigError(f'createtxg of {pool}@{name} is not a number: {txg!r}')
        group = int(txg) + group_plus
        mnt = sh_(['zfs', 'get', '-H', '-o', 'value', 'mountpoint', pool]).strip()
        body = files(os.path.join(mnt, '.zfs', 'snapshot', name))
        path = manifest_out or os.path.join(os.path.dirname(agent.log), f'manifest-{name}')
        with open(path, 'wb') as f:
            f.write(body)
        with open(os.path.join(os.path.dirname(agent.log), 'published.tsv'), 'a') as f:
            f.write(f'{name}\t{group}\t{cuts["attachment"]}\t{cuts["state"]}\t{cuts["count"]}\t{path}\t{txg}\n')
        for _ in range(2 if twice else 1):
            verb, _, code, text = agent.publish(name, cuts['attachment'], cuts['state'], cuts['count'], group, path,
                                                expiry)
            if verb == 'refused':
                return f'refused {code} {text}'
        out_dir = os.path.dirname(agent.log)

        def mark(what, last):
            # The pool as the kstat has it, whatever the daemon answers
            # (#145's F1): the check's premises read this, not the answers.
            if assist_after is not None:
                with open(os.path.join(out_dir, 'accepted.tsv'), 'a') as f:
                    f.write(f'{name}\t{what}\t{mono()}\t{synced(pool)}\t{last}\n')

        def ended(end):
            mark('end', end.split(' ', 1)[0])
            return end
        mark('accepted', assist_after)
        t0 = now()
        last = 0
        assists, assisted_at = 0, 0
        while True:
            verb, fields, code, text = agent.progress(name)
            if verb == 'refused':
                return ended(f'refused {code} {text}')
            if verb in ENDS:
                return ended(verb + ''.join(f' {k}={v}' for k, v in fields.items()))
            waited = now() - t0
            if waited > limit:
                return ended(f'pending after {limit} s')
            if sync:
                # A group with something in it: an idle pool's sync makes
                # no cut.
                (nudge or write_nudge)(os.path.join(mnt, 'publish-nudge'))
                sh_(['zpool', 'sync', pool])
            if assist_after is not None and waited >= assist_after and \
                    (assists < ASSIST_BURST or waited - assisted_at >= assist_after):
                last = assist(pool, name, out_dir, last, sh_, synced, wall, mono, sleep)
                assists, assisted_at = assists + 1, waited
            sleep(1)
    finally:
        if hold:
            sh_(['zfs', 'release', 'skyzfs-publish', f'{pool}@{name}'])


def main(argv):
    try:
        if len(argv) >= 4 and argv[1] == 'raw':
            print(raw(argv[2], argv[3], argv[4] if len(argv) > 4 else 'raw', sys.stdin.buffer.read(),
                      greeting_out=argv[5] if len(argv) > 5 else None))
            return 0
        if len(argv) < 6:
            print(__doc__, file=sys.stderr)
            return 64
        what, skyzfsd, vol, run, log = argv[1:6]
        agent = Agent(skyzfsd, vol, run, log)
        rest = argv[6:]
        if what == 'flow':
            pool, name = rest[0], rest[1]
            opts = rest[2:]
            val = lambda k: opts[opts.index(k) + 1] if k in opts else None
            end = flow(agent, pool, name, sync='--sync' in opts,
                       expiry=int(val('--expiry-s')) if val('--expiry-s') else None,
                       limit=int(val('--limit-s') or 300), manifest_out=val('--manifest-out'),
                       hold='--no-hold' not in opts, group_plus=int(val('--group-plus') or 0),
                       twice='--twice' in opts,
                       assist_after=(float(val('--assist-after-s') or ASSIST_AFTER) if '--assist' in opts
                                     else None))
            print(end)
            return 1 if end.startswith('refused ') else 0
        if what == 'ask':
            req, args = rest[0], rest[1:]
            if req == 'cuts':
                a = agent.cuts()
            elif req in ('progress', 'withdraw', 'look'):
                a = {'progress': agent.progress, 'withdraw': agent.withdraw, 'look': agent.look}[req](args[0])
            elif req == 'retire':
                a = agent.retire(args[0], args[1])
            elif req == 'publish':
                a = agent.publish(*args[:6], expiry=args[6] if len(args) > 6 else None)
            else:
                print(f'not a request: {req}', file=sys.stderr)
                return 64
            verb, fields, code, text = a
            print(f'refused {code} {text}' if verb == 'refused'
                  else verb + ''.join(f' {k}={v}' for k, v in fields.items()))
            return 1 if verb == 'refused' else 0
        print(__doc__, file=sys.stderr)
        return 64
    except RigError as e:
        print(f'rig: {e}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main(sys.argv))
