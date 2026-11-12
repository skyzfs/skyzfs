#!/usr/bin/env python3
"""The daemon's system calls by thread, from a trace of its pid (TB_SYSCALLS).

    syscalls.py perf   SUMMARY THREADS SECONDS > JSON
    syscalls.py strace TRACE   THREADS SECONDS > JSON

SUMMARY is what `perf trace -s -p PID` wrote; TRACE is `strace -f -T
-p PID` output, one line per call with the thread's id first. THREADS
has "tid name" per line, the daemon's threads when the trace began.
The JSON gives, by thread name (the four readers share one), each
system call's count and total milliseconds, the number of threads of
that name, the tool and the trace's seconds:

    {"tool": "perf", "seconds": 20, "threads": {"skyzfs-reader":
     {"tids": 4, "calls": {"io_uring_enter": [1234, 56.7], ...}}}}

A trace slows the daemon; a traced row counts for no ratio.
"""
import json
import re
import sys

PERF_THREAD = re.compile(r'^\s*(.+?) \((\d+)\), \d+ events')
# name, calls, [errors,] total, min, avg, max ms (older perf has no errors column)
PERF_ROW = re.compile(r'^\s*([a-z_0-9]+)\s+(\d+)\s+(?:(\d+)\s+)?([\d.]+)\s+[\d.]+\s+[\d.]+\s+[\d.]+')
# "1234  io_uring_enter(3, 1, 0, ...) = 1 <0.000123>"; also "<... read resumed>"
STRACE_LINE = re.compile(r'^(\d+)\s+(?:<\.\.\. ([a-z_0-9]+) resumed>|([a-z_0-9]+)\().*<([\d.]+)>\s*$')


def threads(text):
    """"tid name" lines -> {tid: name}."""
    out = {}
    for line in text.splitlines():
        f = line.split(None, 1)
        if len(f) == 2 and f[0].isdigit():
            out[int(f[0])] = f[1].strip()
    return out


def by_name(per_tid, names):
    """{tid: {call: [n, ms]}} -> {name: {'tids': k, 'calls': {call: [n, ms]}}}; a tid not named keeps its own name."""
    out = {}
    for tid, calls in per_tid.items():
        name = re.sub(r'-\d+$', '', names[tid]) if tid in names else f'tid {tid}'  # skyzfs-reader-2 -> skyzfs-reader
        t = out.setdefault(name, {'tids': 0, 'calls': {}})
        t['tids'] += 1
        for c, (n, ms) in calls.items():
            a = t['calls'].setdefault(c, [0, 0.0])
            a[0] += n
            a[1] += ms
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


def strace(text):
    """`strace -f -T` -> {tid: {call: [calls, total ms]}}. A call cut in two counts once, at its end."""
    per = {}
    for line in text.splitlines():
        if line.rstrip().endswith('<unfinished ...>'):
            continue
        m = STRACE_LINE.match(line)
        if not m:
            continue
        tid, call, secs = int(m.group(1)), m.group(2) or m.group(3), float(m.group(4))
        a = per.setdefault(tid, {}).setdefault(call, [0, 0.0])
        a[0] += 1
        a[1] += secs * 1000
    return per


def main():
    tool, path, tpath, seconds = sys.argv[1], sys.argv[2], sys.argv[3], float(sys.argv[4])
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
    elif tool == 'strace':
        per = strace(text)
    else:
        sys.exit(f'syscalls.py: unknown tool {tool}')
    json.dump({'tool': tool, 'seconds': seconds, 'threads': by_name(per, names)}, sys.stdout)


if __name__ == '__main__':
    main()
