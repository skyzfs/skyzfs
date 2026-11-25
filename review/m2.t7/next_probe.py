#!/usr/bin/env python3
"""The next look at #139 (c7cc03f), N5: what publish_agent.raw() makes of
each way a daemon can end a connection, and how publish_check classes it
for the label `peer` (EXPECTED['pchannel']['peer'] = 'refused peer'):
ok, a verdict against the daemon (exit 1), or the rig's error (exit 2).

Usage: next_probe.py KERNEL_DIR [TRIALS]

The servers write their bytes and close at once with the request unread
(as skyzfsd refuses a peer), or read the request first; "late" makes the
client's write wait until the server has closed (EPIPE, not ECONNRESET).
Two more: a socket file with no listener (the daemon gone), and a daemon
that accepts and never answers (the client's 60 s limit cut to 1 s).
"""
import importlib.util
import os
import socket
import sys
import tempfile
import threading
import types

HELLO = b'skyzfs-control 1\n'
FULL = HELLO + b'refused peer this daemon answers its own user\n'
REQ = HELLO + b'cuts\n'

SCENARIOS = [
    # name, bytes written, read the request first, client late, kind
    ('the answer, request unread', FULL, False, False, 'serve'),
    ('the answer, daemon closed before the client wrote', FULL, False, True, 'serve'),
    ('the answer, request read (EOF)', FULL, True, False, 'serve'),
    ('half the answer line, request unread', HELLO + b'refused peer this daem', False, False, 'serve'),
    ('half the answer line, request read (EOF)', HELLO + b'refused peer this daem', True, False, 'serve'),
    ('the greeting only, request unread', HELLO, False, False, 'serve'),
    ('the greeting only, request read (EOF)', HELLO, True, False, 'serve'),
    ('nothing, request unread (reset)', b'', False, False, 'serve'),
    ('nothing, daemon closed before the client wrote (EPIPE)', b'', False, True, 'serve'),
    ('nothing, request read (EOF)', b'', True, False, 'serve'),
    ('the answer with no greeting', b'refused peer this daemon answers its own user\n', False, False, 'serve'),
    ('another greeting, then the answer', b'skyzfs-control 2\nrefused peer x\n', True, False, 'serve'),
    ('a socket file with no listener (the daemon gone)', b'', False, False, 'stale'),
    ('accepted, never answered (limit cut to 1 s)', b'', False, False, 'hang'),
]


def load(kdir):
    spec = importlib.util.spec_from_file_location('publish_agent_probe', os.path.join(kdir, 'publish_agent.py'))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def serve(path, data, read_first, ready, closed, hang):
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(path)
    srv.listen(1)
    srv.settimeout(10)
    ready.set()
    try:
        c, _ = srv.accept()
        c.settimeout(10)
        if hang:
            hang.wait(10)
        if read_first:
            got = b''
            while len(got) < len(REQ):
                b = c.recv(4096)
                if not b:
                    break
                got += b
        if data:
            c.sendall(data)
        c.close()
    finally:
        srv.close()
        closed.set()


def classed(outcome):
    kind, text = outcome
    if kind == 'raised':
        return "exit 2 (the rig's error)"
    return 'ok' if text.startswith('refused peer') else 'exit 1 (a verdict)'


def main():
    kdir = sys.argv[1]
    trials = int(sys.argv[2]) if len(sys.argv) > 2 else 20
    pa = load(kdir)
    real = pa.socket
    for name, data, read_first, late, kind in SCENARIOS:
        seen = {}
        for _ in range(trials if kind != 'hang' else 2):
            with tempfile.TemporaryDirectory() as d:
                os.makedirs(os.path.join(d, 'v'))
                path = os.path.join(d, 'v', 'control')
                ready, closed, hang = threading.Event(), threading.Event(), threading.Event()
                th = None
                if kind == 'stale':
                    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                    s.bind(path)
                    s.close()
                else:
                    th = threading.Thread(target=serve, args=(path, data, read_first, ready, closed,
                                                              hang if kind == 'hang' else None), daemon=True)
                    th.start()
                    ready.wait(10)

                class Probe(socket.socket):
                    def settimeout(self, t):
                        return super().settimeout(1 if kind == 'hang' else t)

                    def sendall(self, b, *a):
                        if late:
                            closed.wait(10)
                        return super().sendall(b, *a)
                mod = types.ModuleType('socket')
                mod.__dict__.update(socket.__dict__)
                mod.socket = Probe
                pa.socket = mod
                try:
                    out = ('answer', pa.raw(d, os.path.join(d, 'agent.tsv'), 'peer', REQ))
                except pa.RigError as e:
                    out = ('raised', str(e).split(': ', 1)[-1])
                finally:
                    pa.socket = real
                    hang.set()
                if th:
                    th.join(10)
            key = (out, classed(out))
            seen[key] = seen.get(key, 0) + 1
        for (out, v), n in sorted(seen.items(), key=lambda kv: -kv[1]):
            print(f'{name}: {n}: {out[0]} {out[1]!r} -> {v}')


if __name__ == '__main__':
    main()
