"""read_bounded() of replay_daemon.py with real processes. Run:
    python3 review/m1.t14/read_bounded_try.py <tests/kernel dir of the head>

Cases: a reader that ends; one that cannot start; one that hangs and dies when
killed; one whose pipe stays open after the kill (a child of the reader keeps
it), which stands in for a reader that outlives SIGKILL. Each must come back
within its bounds. No device is used.
"""
import sys, time
sys.path.insert(0, sys.argv[1])
import replay_daemon as rd

rd.REAP_S = 1.0
def case(name, cmd, timeout):
    calls = []
    t0 = time.monotonic()
    rc, out, _ = rd.read_bounded(cmd, timeout, lambda: calls.append(round(time.monotonic() - t0, 1)))
    print(f'{name:<44} exit {rc:<4} in {time.monotonic() - t0:4.1f} s  unstick at {calls}  | {out.strip()[-70:]!r}')

case('ends', ['sh', '-c', 'echo done'], 5)
case('cannot start', ['/nonexistent/dd'], 5)
case('hangs, dies when killed', ['sleep', '30'], 0.5)
case('its pipe outlives the kill', ['sh', '-c', 'sleep 5 & exec sleep 30'], 0.5)
case('slow but ends before the limit', ['sh', '-c', 'sleep 0.3; echo late'], 5)
