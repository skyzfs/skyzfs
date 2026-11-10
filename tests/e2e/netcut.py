#!/usr/bin/env python3
"""A TCP link that can be cut *silently* — the e2e harness's partition
primitive (origin-advance design §8.2).

Sits between a client and a server and relays bytes. While a flag file
exists the link is CUT, which here means exactly one thing:

    the socket stays open and nothing crosses it, in either direction

Not a refusal. That distinction is the whole reason this file exists.
An unreachable server has two shapes, and they are not interchangeable:

  * connection REFUSED — a dead port. Fails instantly, at connect(). This
    is what `server = "http://127.0.0.1:1"` gives you, and it is the only
    shape the harness could produce before this primitive.
  * accepted and SILENT — a SIGSTOPped peer, a load balancer that accepts
    and never proxies, a conntrack entry dropped after establishment. The
    connect succeeds; the request goes out; no answer ever comes back.

The second shape is the one that hangs a client with no read timeout, and
a hung client never reports a failed cycle — which is how the entire §8.1
degradation ladder can be unreachable in exactly the outage it exists
for. Reproducing that needs a link that goes quiet without going away.

**This supplies the accepted-and-silent shape only, and the distinction
is worth stating.** A dropped-SYN black hole (a NetworkPolicy DROP, a
vanished node) is also silent, but it is silent *earlier*: the handshake
never completes, `connect()` hangs, and the client gives up on its
CONNECT timeout rather than its read timeout. Here the accept always
completes immediately, so what is exercised is the read timeout.

Both shapes end in a retryable transport error, so the failing clock runs
either way and the ladder is reachable under both. But it does mean the
agent's connect timeout has no end-to-end coverage from this primitive —
only its read timeout does. Modelling the other one honestly needs the
SYN to go unanswered, which userspace cannot arrange by declining to
`accept()` (the kernel completes the handshake into the backlog first).

Usage:
    netcut.py --listen PORT --to HOST:PORT --cut-file PATH

    touch PATH   # cut: bytes stop crossing, sockets stay up
    rm PATH      # heal: held connections proceed, new ones connect

The flag file is polled rather than signalled so a shell scenario can
drive it with `touch`/`rm` and no PID bookkeeping.

Cut semantics, precisely:

  * A connection ACCEPTED while cut is held open with no upstream
    connection behind it — accepted, and never answered.
  * A connection ESTABLISHED before the cut stalls where it is. Bytes
    already in flight are not delivered: the check is re-taken after
    every read and before the matching write, so no byte crosses a cut
    link even if the read that produced it started before the cut.
  * Nothing is closed, reset, or refused. A cut that closed sockets would
    hand the client an error instantly and test the refused path again.
"""

import argparse
import asyncio
import os
import select
import sys

POLL_INTERVAL = 0.05
CHUNK = 65536

# Linux-only; absent elsewhere, in which case the abandoned-peer check
# degrades to "assume still there" and the old replay behaviour returns.
POLLRDHUP = getattr(select, "POLLRDHUP", 0)


def log(msg):
    print(f"netcut: {msg}", file=sys.stderr, flush=True)


def peer_gone(writer):
    """Has the client closed its end while we were holding it through a cut?

    This exists because a cut buffers whatever the client sent, and a
    client with a read timeout does not wait forever: it gives up and
    closes. Without this check the heal then dials upstream and delivers
    that request — on a connection the server never had, for a client
    that is already gone. Every cycle the agent abandoned during the
    outage would arrive at the daemon in a burst the moment the link came
    back, concurrently with the recovery cycle the scenario is making
    exact assertions about.

    Real TCP can retransmit queued data once a block clears, so replaying
    is not *wrong* — but under the dropped-SYN shape those requests would
    never reach the server at all, and a stale `prepare` landing mid
    recovery is the kind of thing that would look like anything but this
    when it eventually caused a flake.

    `POLLRDHUP` is the precise signal: the peer closed its writing side.
    Only consulted for connections that actually stalled, so a client
    that legitimately half-closes on a healthy link is never affected.
    """
    sock = writer.get_extra_info("socket")
    if sock is None or not POLLRDHUP:
        return False
    poller = select.poll()
    poller.register(sock, POLLRDHUP | select.POLLHUP | select.POLLERR)
    return bool(poller.poll(0))


class Link:
    """The cut flag, polled into an asyncio.Event.

    `up` is set when the link passes traffic, so every pump can simply
    `await link.up.wait()` and stall for the duration of a cut.
    """

    def __init__(self, cut_file):
        self.cut_file = cut_file
        self.up = asyncio.Event()
        self.up.set()

    async def poll_forever(self):
        while True:
            cut = os.path.exists(self.cut_file)
            if cut and self.up.is_set():
                self.up.clear()
                log("CUT — sockets stay open, nothing crosses")
            elif not cut and not self.up.is_set():
                self.up.set()
                log("HEALED — traffic resumes")
            await asyncio.sleep(POLL_INTERVAL)


async def pump(link, reader, writer, abandoned=None):
    """Relay one direction, stalling for the duration of a cut.

    The `up.wait()` after the read is not redundant with the one before
    it: a read can complete while the link is up and the cut can land
    before the write, and letting that byte through would make the cut
    leaky in precisely the race a partition test needs to be tight about.

    `abandoned` is consulted only after a stall, and only on the
    client→upstream direction: see `peer_gone`.
    """
    try:
        while True:
            stalled = not link.up.is_set()
            await link.up.wait()
            if stalled and abandoned is not None and abandoned():
                log("dropping a stalled connection whose client gave up")
                break
            data = await reader.read(CHUNK)
            if not data:
                break
            if not link.up.is_set():
                stalled = True
                await link.up.wait()
                if abandoned is not None and abandoned():
                    log("dropping a stalled connection whose client gave up")
                    break
            writer.write(data)
            await writer.drain()
    except (ConnectionResetError, BrokenPipeError, OSError):
        pass
    finally:
        writer.close()


async def handle(link, upstream, client_reader, client_writer):
    host, port = upstream
    # A connection accepted during a cut is held here — accepted at the
    # TCP layer, with nothing behind it and no answer coming. Dialing
    # upstream first would make the cut invisible to a client that
    # connects during it.
    held = not link.up.is_set()
    await link.up.wait()
    # ...and if the client gave up while being held, it gets no upstream
    # connection at all. Dialing here would deliver its request after the
    # heal, on a connection the server never had.
    if held and peer_gone(client_writer):
        log("dropping a held connection whose client gave up during the cut")
        client_writer.close()
        return
    try:
        up_reader, up_writer = await asyncio.open_connection(host, port)
    except OSError as e:
        log(f"upstream connect failed: {e}")
        client_writer.close()
        return
    await asyncio.gather(
        pump(link, client_reader, up_writer, abandoned=lambda: peer_gone(client_writer)),
        pump(link, up_reader, client_writer),
        return_exceptions=True,
    )


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--listen", type=int, required=True)
    ap.add_argument("--to", required=True, help="HOST:PORT")
    ap.add_argument("--cut-file", required=True)
    args = ap.parse_args()

    host, _, port = args.to.rpartition(":")
    upstream = (host, int(port))
    link = Link(args.cut_file)
    asyncio.create_task(link.poll_forever())

    server = await asyncio.start_server(
        lambda r, w: handle(link, upstream, r, w),
        "127.0.0.1",
        args.listen,
    )
    log(f"listening on 127.0.0.1:{args.listen} -> {args.to} (cut file: {args.cut_file})")
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
