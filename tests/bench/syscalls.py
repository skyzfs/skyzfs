#!/usr/bin/env python3
"""The daemon's system calls by thread, from a trace of its pid (TB_SYSCALLS).

    syscalls.py perf   SUMMARY THREADS SECONDS [SLOW_MS] > JSON
    syscalls.py strace TRACE   THREADS SECONDS [SLOW_MS] > JSON

SUMMARY is what `perf trace -S --duration SLOW_MS -p PID` wrote: the
calls that took SLOW_MS or more, one line each, then the summary of
all calls by thread (`-s` alone writes the summary only). A line
"... [continued]: pread64())" is a call whose entry perf did not see,
timed from the thread's previous entry and so too long: it is not
counted as slow (PERF_EVENT wants the call's name after the thread). TRACE is
`strace -f -T -p PID` output, one line per call with the thread's id
first. THREADS has "tid name" per line, the daemon's threads when the
trace began. The JSON gives, by thread name (the four readers share
one), each system call's count and total milliseconds, the number of
threads of that name, the tool and the trace's seconds, and, when
SLOW_MS is given, how many of each call took SLOW_MS or more:

    {"tool": "perf", "seconds": 20, "slow_ms": 1.0, "threads":
     {"skyzfs-reader": {"tids": 4, "calls": {"io_uring_enter":
     [1234, 56.7], ...}, "slow": {"io_uring_enter": 3, ...}}}}

A trace slows the daemon; a traced row counts for no ratio.
"""
import json
import re
import sys

PERF_THREAD = re.compile(r'^\s*(.+?) \((\d+)\), \d+ events')
# name, calls, [errors,] total, min, avg, max ms (older perf has no errors column)
PERF_ROW = re.compile(r'^\s*([a-z_0-9]+)\s+(\d+)\s+(?:(\d+)\s+)?([\d.]+)\s+[\d.]+\s+[\d.]+\s+[\d.]+')
# "1234  io_uring_enter(3, 1, 0, ...) = 1 <0.000123>"; also "<... read resumed>"
# One call of `perf trace`: "  0.123 ( 1.234 ms): comm/tid pwrite64(fd: 5, ...) = 4096"
PERF_EVENT = re.compile(r'\(\s*([\d.]+) ms\):\s+.*?/(\d+)\s+([a-z_0-9]+)\(')
STRACE_LINE = re.compile(r'^(\d+)\s+(?:<\.\.\. ([a-z_0-9]+) resumed>|([a-z_0-9]+)\().*<([\d.]+)>\s*$')


def threads(text):
    """"tid name" lines -> {tid: name}."""
    out = {}
    for line in text.splitlines():
        f = line.split(None, 1)
        if len(f) == 2 and f[0].isdigit():
            out[int(f[0])] = f[1].strip()
    return out


def by_name(per_tid, names, slow=None):
    """{tid: {call: [n, ms]}} -> {name: {'tids': k, 'calls': {call: [n, ms]}}}; a tid not named keeps its own name.

    With slow ({tid: {call: n}}), each name also gets 'slow': {call: n}."""
    out = {}
    def name_of(tid):
        return re.sub(r'-\d+$', '', names[tid]) if tid in names else f'tid {tid}'  # skyzfs-reader-2 -> skyzfs-reader
    for tid, calls in per_tid.items():
        t = out.setdefault(name_of(tid), {'tids': 0, 'calls': {}})
        t['tids'] += 1
        for c, (n, ms) in calls.items():
            a = t['calls'].setdefault(c, [0, 0.0])
            a[0] += n
            a[1] += ms
    if slow is not None:
        for t in out.values():
            t['slow'] = {}
        for tid, calls in slow.items():
            t = out.setdefault(name_of(tid), {'tids': 1, 'calls': {}, 'slow': {}})
            for c, n in calls.items():
                t['slow'][c] = t['slow'].get(c, 0) + n
    for t in out.values():
        for a in t['calls'].values():
            a[1] = round(a[1], 3)
    return out


def perf(text):
    """`perf trace -s` -> ({tid: {call: [calls, total ms]}}, {tid: the name perf gave})."""
    per, names, tid = {}, {}, None
    for line in text.splitlines():
        m = PERF_THREAD.match(line)
        if m:
            tid = int(m.group(2))
            names[tid] = m.group(1).strip()
            per.setdefault(tid, {})
            continue
        m = PERF_ROW.match(line)
        if m and tid is not None and m.group(1) not in ('syscall',):
            per[tid][m.group(1)] = [int(m.group(2)), float(m.group(4))]
    return per, names


def perf_slow(text, slow_ms):
    """The calls `perf trace --duration` printed -> {tid: {call: n}}, those of slow_ms or more."""
    out = {}
    for line in text.splitlines():
        m = PERF_EVENT.search(line)
        if m and float(m.group(1)) >= slow_ms:
            c = out.setdefault(int(m.group(2)), {})
            c[m.group(3)] = c.get(m.group(3), 0) + 1
    return out


def strace(text, slow_ms=None):
    """`strace -f -T` -> ({tid: {call: [calls, total ms]}}, {tid: {call: calls of slow_ms or more}}).

    A call cut in two counts once, at its end."""
    per, slow = {}, {}
    for line in text.splitlines():
        m = STRACE_LINE.match(line)
        if not m:
            continue
        tid, call, secs = int(m.group(1)), m.group(2) or m.group(3), float(m.group(4))
        a = per.setdefault(tid, {}).setdefault(call, [0, 0.0])
        a[0] += 1
        a[1] += secs * 1000
        if slow_ms is not None and secs * 1000 >= slow_ms:
            c = slow.setdefault(tid, {})
            c[call] = c.get(call, 0) + 1
    return per, slow


def main():
    tool, path, tpath, seconds = sys.argv[1], sys.argv[2], sys.argv[3], float(sys.argv[4])
    slow_ms = float(sys.argv[5]) if len(sys.argv) > 5 else None
    with open(path, errors='replace') as f:
        text = f.read()
    try:
        with open(tpath) as f:
            names = threads(f.read())
    except OSError:
        names = {}
    if tool == 'perf':
        per, perf_names = perf(text)
        names = {**perf_names, **names}
        slow = perf_slow(text, slow_ms) if slow_ms is not None else None
    elif tool == 'strace':
        per, slow = strace(text, slow_ms)
        slow = slow if slow_ms is not None else None
    else:
        sys.exit(f'syscalls.py: unknown tool {tool}')
    out = {'tool': tool, 'seconds': seconds, 'threads': by_name(per, names, slow)}
    if slow_ms is not None:
        out['slow_ms'] = slow_ms
    json.dump(out, sys.stdout)


if __name__ == '__main__':
    main()
