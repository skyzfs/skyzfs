# How far a call's two stamps stand outside the system call itself, with two
# threads as in overlap-tear.py: a pwritev of nothing to /dev/null.
import os, threading, time, statistics
fd = os.open('/dev/null', os.O_WRONLY)
spans = []
def one(out):
    b = time.monotonic_ns(); os.pwritev(fd, [b''], 0); out.append(time.monotonic_ns() - b)
for _ in range(3000):
    gate = threading.Barrier(2); a, b = [], []
    def w(o):
        gate.wait(); one(o)
    ts = [threading.Thread(target=w, args=(o,)) for o in (a, b)]
    [t.start() for t in ts]; [t.join() for t in ts]
    spans += a + b
spans.sort()
print('stamp to stamp around an empty call, ns: median', spans[len(spans)//2], 'p90', spans[len(spans)*9//10], 'p99', spans[len(spans)*99//100], 'max', spans[-1])
