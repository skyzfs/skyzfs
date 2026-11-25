# Review probe (Q10, third look): the head's txg_probe.py against a
# simulated pool. A simulator writes the txgs kstat (a group every 0.5 s,
# and one held in sync for HOLD seconds over the window); a fake zpool's
# sync waits while the hold file exists. Args: PROBE_PY DIR MODE
# MODE: hold (the probe runs through) | dead (the probe is SIGKILLed before the window)
import os, signal, subprocess, sys, time
probe_py, d, mode = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(os.path.join(d, "bin"), exist_ok=True)
kstat, log, hold = os.path.join(d, "txgs"), os.path.join(d, "p.tsv"), os.path.join(d, "held")
with open(os.path.join(d, "bin", "zpool"), "w") as f:
    f.write("#!/bin/sh\nwhile [ -e %s ]; do sleep 0.05; done\nsleep 0.05\n" % hold)
os.chmod(os.path.join(d, "bin", "zpool"), 0o755)
HDR = "txg      birth            state ndirty       nread        nwritten     reads    writes   otime        qtime        wtime        stime\n"
raw = lambda: time.clock_gettime_ns(time.CLOCK_MONOTONIC_RAW)
groups = []  # (txg, birth, o, q, w, s, state)
def dump():
    with open(kstat + ".tmp", "w") as f:
        f.write(HDR)
        for g in groups[-100:]:
            f.write("%d %d %s 0 0 0 0 0 %d %d %d %d\n" % (g[0], g[1], g[6], g[2], g[3], g[4], g[5]))
    os.replace(kstat + ".tmp", kstat)
def commit(n, began_sync, ended):
    birth = began_sync - 10**8
    groups.append((n, birth, 5 * 10**7, 10**6, 49 * 10**6, ended - began_sync, "C"))
    dump()
n = 1
for _ in range(4):
    t = raw(); commit(n, t - 5 * 10**7, t); n += 1
env = dict(os.environ, PATH=os.path.join(d, "bin") + os.pathsep + os.environ["PATH"])
p = subprocess.Popen([sys.executable, probe_py, "probe", kstat, "skzt", log, "0.5"], env=env)
def tick(secs):
    global n
    end = time.time() + secs
    while time.time() < end:
        time.sleep(0.5); t = raw(); commit(n, t - 5 * 10**7, t); n += 1
tick(2.0)
if mode == "dead":
    p.send_signal(signal.SIGKILL); p.wait()
open(hold, "w").close()
t0 = time.time(); hs = raw()
time.sleep(3.0)
t1 = time.time(); he = raw()
os.unlink(hold)
commit(n, hs, he); n += 1
tick(2.0)
if mode == "hold":
    p.send_signal(signal.SIGTERM); p.wait(timeout=30)
r = subprocess.run([sys.executable, probe_py, "report", log, "%.6f" % t0, "%.6f" % t1], capture_output=True, text=True)
print(mode + ": window %.3f s -> %s (exit %d)" % (t1 - t0, r.stdout.strip(), r.returncode))
