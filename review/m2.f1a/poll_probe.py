"""Probe poll(2) revents on the accepting end of an AF_UNIX SOCK_STREAM
connection, for each way a client can end. Two polls per case: with
events=0 (what skyzfsd's hung_up asks) and with events=POLLRDHUP.
The accepted socket is polled after the server has read the request whole
(as skyzfsd does), unless the case says otherwise."""
import os
import select
import socket
import sys
import tempfile
import time

NAMES = [(select.POLLIN, 'IN'), (select.POLLPRI, 'PRI'), (select.POLLOUT, 'OUT'),
         (select.POLLERR, 'ERR'), (select.POLLHUP, 'HUP'), (select.POLLNVAL, 'NVAL'),
         (getattr(select, 'POLLRDHUP', 0x2000), 'RDHUP')]
RDHUP = getattr(select, 'POLLRDHUP', 0x2000)
REQ = b'skyzfs-control 2\npublish name=t manifest=5\nhello'


def fmt(m):
    return '|'.join(n for b, n in NAMES if m & b) or '0'


def poll(fd, events):
    p = select.poll()
    p.register(fd, events)
    r = p.poll(0)
    return r[0][1] if r else 0


def verdict(fd):
    m = poll(fd, 0)
    return 'hung_up' if m & (select.POLLHUP | select.POLLERR) else 'present'


def listener():
    d = tempfile.mkdtemp()
    path = os.path.join(d, 'control')
    ls = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    ls.bind(path)
    ls.listen(8)
    return ls, path


def read_whole(s, n=len(REQ)):
    got = b''
    s.settimeout(2)
    while len(got) < n:
        b = s.recv(n - len(got))
        if not b:
            break
        got += b
    return got


def case(name, client_act, read_first=True, accept_after=False, server_writes=False):
    ls, path = listener()
    c = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    c.connect(path)
    if accept_after:
        # The connection waits in the backlog while the client acts.
        client_act(c)
        s, _ = ls.accept()
    else:
        s, _ = ls.accept()
        if server_writes:
            s.sendall(b'unsolicited\n')
        client_act(c)
    time.sleep(0.05)
    got = read_whole(s) if read_first else b''
    e0 = poll(s.fileno(), 0)
    er = poll(s.fileno(), RDHUP)
    ein = poll(s.fileno(), select.POLLIN | RDHUP)
    err = s.getsockopt(socket.SOL_SOCKET, socket.SO_ERROR)
    print(f'{name:<58} read={len(got):>3}  events=0: {fmt(e0):<10} events=RDHUP: {fmt(er):<14} '
          f'events=IN|RDHUP: {fmt(ein):<14} SO_ERROR={err}  hung_up={verdict(s.fileno()) if True else ""}')
    # what a write of the answer does now
    try:
        s.setblocking(False)
        n = s.send(b'skyzfs-control 2\naccepted\n')
        w = f'write ok ({n})'
    except OSError as e:
        w = f'write {type(e).__name__} errno={e.errno}'
    print(f'{"":<58} answer: {w}')
    for x in (c, s, ls):
        try:
            x.close()
        except OSError:
            pass


def send_close(c):
    c.sendall(REQ)
    c.close()


def send_shutwr(c):
    c.sendall(REQ)
    c.shutdown(socket.SHUT_WR)


def send_shutrd(c):
    c.sendall(REQ)
    c.shutdown(socket.SHUT_RD)


def send_shutrdwr(c):
    c.sendall(REQ)
    c.shutdown(socket.SHUT_RDWR)


def send_only(c):
    c.sendall(REQ)


def send_extra_close(c):
    c.sendall(REQ + b'trailing bytes the server never reads')
    c.close()


def send_fork_close(c):
    # A child that inherited the descriptor keeps the connection open
    # after the parent closes its own.
    c.sendall(REQ)
    c.set_inheritable(True)
    pid = os.fork()
    if pid == 0:
        time.sleep(1.0)
        os._exit(0)
    c.close()
    send_fork_close.pid = pid


def send_exit_child(c):
    # The client is another process, killed after it sent.
    fd = c.fileno()
    c.set_inheritable(True)
    pid = os.fork()
    if pid == 0:
        os.write(fd, REQ)
        os.kill(os.getpid(), 9)
    os.waitpid(pid, 0)
    c.close()


print(f'kernel {os.uname().release}  python {sys.version.split()[0]}')
case('1 client sent, still connected (waits for answer)', send_only)
case('2 client sent, then shutdown(SHUT_WR) (half-close)', send_shutwr)
case('3 client sent, then close()', send_close)
case('4 client sent, then close(); server polls before reading', send_close, read_first=False)
case('5 client sent more than the request, then close()', send_extra_close)
case('6 client sent, then shutdown(SHUT_RD)', send_shutrd)
case('7 client sent, then shutdown(SHUT_RDWR) (not closed)', send_shutrdwr)
case('8 queued in backlog: client sent, then close(), then accept', send_close, accept_after=True)
case('9 queued in backlog: client sent, then SHUT_WR, then accept', send_shutwr, accept_after=True)
case('10 server wrote first, client sent and closed unread', send_close, server_writes=True)
case('11 client forked a child holding the fd, parent closed', send_fork_close)
try:
    os.waitpid(send_fork_close.pid, 0)
except Exception:
    pass
case('12 client process killed (SIGKILL) after sending', send_exit_child)
