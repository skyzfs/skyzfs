#!/usr/bin/env python3
"""The review of #139 at 9a87140: what publish_agent.raw() makes of each
way a daemon can end a connection, before and after the fix ("an answer
read before a reset is the raw client's answer").

Usage: fix_probe.py KERNEL_DIR [TRIALS]
KERNEL_DIR is a tests/kernel directory whose publish_agent.py is probed
(the head's, or the head's with cabd5a0's raw() put back). Each scenario
runs TRIALS times; the line gives each distinct outcome and its count,
and what publish_check's EXPECTED['pchannel']['peer'] would make of it
(ok, FAILED: a verdict against the daemon, or exit 2: the rig's error).

The servers write their bytes and close at once with the request unread
(as skyzfsd refuses a peer), or read the request first; "late" makes the
client's sendall wait 0.2 s so that the daemon has closed before it, the
other order on the wire (EPIPE, not ECONNRESET).
"""
import importlib.util
import os
import socket
import sys
import tempfile
import threading
import time
import types

HELLO = b'skyzfs-control 1\n'
FULL = HELLO + b'refused peer this daemon answers its own user\n'
REQ = HELLO + b'cuts\n'

SCENARIOS = [
    # name, bytes the daemon writes, read the request first, client late
    ('the answer, request unread', FULL, False, False),
    ('the answer, daemon closed before the client wrote', FULL, False, True),
    ('the answer, request read (EOF)', FULL, True, False),
    ('half the answer line, request unread', HELLO + b'refused peer this daem', False, False),
    ('half the answer line, request read (EOF)', HELLO + b'refused peer this daem', True, False),
    ('the greeting only, request unread', HELLO, False, False),
    ('the greeting only, request read (EOF)', HELLO, True, False),
    ('nothing, request unread (reset, no answer)', b'', False, False),
    ('nothing, daemon closed before the client wrote', b'', False, True),
    ('nothing, request read (EOF, no answer)', b'', True, False),
]


def load(kdir):
    spec = importlib.util.spec_from_file_location('publish_agent_probe', os.path.join(kdir, 'publish_agent.py'))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def serve(path, data, read_first, ready):
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(path)
    srv.listen(1)
    srv.settimeout(10)
    ready.set()
    try:
        c, _ = srv.accept()
        c.settimeout(10)
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


def verdict(pa_mod, outcome):
    kind, text = outcome
    if kind == 'raised':
        return 'exit 2 (the rig\'s error)'
    return 'ok' if text.startswith('refused peer') else 'FAILED (a verdict against the daemon)'


def main():
    kdir = sys.argv[1]
    trials = int(sys.argv[2]) if len(sys.argv) > 2 else 20
    pa = load(kdir)
    real = pa.socket

    class Late(socket.socket):
        def sendall(self, data, *a):
            time.sleep(0.2)
            return super().sendall(data, *a)

    late_mod = types.SimpleNamespace(**{k: getattr(socket, k) for k in dir(socket) if not k.startswith('__')})
    late_mod.socket = Late
    for name, data, read_first, late in SCENARIOS:
        seen = {}
        for i in range(trials):
            with tempfile.TemporaryDirectory() as d:
                os.makedirs(os.path.join(d, 'v'))
                path = os.path.join(d, 'v', 'control')
                ready = threading.Event()
                th = threading.Thread(target=serve, args=(path, data, read_first, ready), daemon=True)
                th.start()
                ready.wait(10)
                pa.socket = late_mod if late else real
                try:
                    out = ('answer', pa.raw(d, os.path.join(d, 'agent.tsv'), 'peer', REQ))
                except pa.RigError as e:
                    out = ('raised', str(e).split(': ', 1)[-1])
                finally:
                    pa.socket = real
                th.join(10)
            key = (out, verdict(pa, out))
            seen[key] = seen.get(key, 0) + 1
        for (out, v), n in sorted(seen.items(), key=lambda kv: -kv[1]):
            print(f'{name}: {n}/{trials}: {out[0]} {out[1]!r} -> {v}')


if __name__ == '__main__':
    main()
