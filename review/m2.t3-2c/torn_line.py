#!/usr/bin/env python3
"""#113, the next look: the request log of the recording `takeover` that
went red on f5afb60 (run 44183715455, ubuntu-26.04): where its malformed
line is, how it is cut, and what stands around it.

    python3 -I torn_line.py REQUESTS_TSV INSTANCES_TSV
"""
import sys

path, inst = sys.argv[1], sys.argv[2]
with open(path, 'rb') as f:
    data = f.read()
with open(inst) as f:
    print('instances.tsv:', ' | '.join(ln.strip().replace('\t', ' ') for ln in f))
lines = data.split(b'\n')
print(f'{len(data)} bytes, {len(lines) - 1} lines, the file ends with a newline: {data.endswith(b"\n")}')
off, bad = 0, []
for k, ln in enumerate(lines[:-1]):
    f = ln.split(b'\t')
    if not 8 <= len(f) <= 10 or not f[0].isdigit():
        bad.append((k, off, ln))
    off += len(ln) + 1
print(f'{len(bad)} lines that are not eight to ten columns')
for k, off, ln in bad:
    f = ln.split(b'\t')
    print(f'line {k + 1} at byte {off}: {len(ln)} bytes, {len(f)} columns')
    print(f'  its first columns: {[x[:24].decode("ascii", "replace") for x in f[:8]]}')
    print(f'  column lengths: {[len(x) for x in f]}')
    # a start line fused onto it: ... <ns>\t<pid>\tstart\t...
    if b'start' in f:
        s = f.index(b'start')
        tail = f[s - 2:]
        print(f'  a start line is in it, from column {s - 1}: {[x[-24:].decode("ascii", "replace") for x in tail]}')
        fused = f[s - 2]
        print(f'  the column where the two meet has {len(fused)} bytes and ends {fused[-40:].decode("ascii", "replace")!r}')
        # the start line's own text is its last 8 columns but the first, whose beginning is fused
        rest = b'\t'.join(f[s - 1:])
        cut_at = off + len(ln) - len(rest) - 1    # the byte of the tab after the fused column
        prev = lines[k - 1].split(b'\t')
        nxt = lines[k + 1].split(b'\t')
        print(f'  the line before: ns {prev[0].decode()} pid {prev[1].decode()} {prev[2].decode()}; '
              f'the line after: ns {nxt[0].decode()} pid {nxt[1].decode()} {nxt[2].decode()}')
        ns_len = len(nxt[0])
        torn_end = cut_at - ns_len      # if the start line's ns has as many digits as the next line's
        print(f'  if the start line\'s time has {ns_len} digits like the next line\'s, the torn line ends at byte '
              f'{torn_end} = {torn_end // 4096} * 4096 + {torn_end % 4096}, {torn_end - off} bytes after its start '
              f'(its start: {off // 4096} * 4096 + {off % 4096})')
        print(f'  the start line would then be: {fused[-ns_len:].decode()} {[x.decode("ascii", "replace") for x in f[s - 1:]]}')
    pids = {}
    for ln2 in lines[max(0, k - 3):k]:
        g = ln2.split(b'\t')
        print(f'    before: {g[0].decode()} {g[1].decode()} {g[2].decode()} {g[3].decode()} {g[4].decode()} '
              f'{g[5].decode()} {g[6].decode()} {g[7][:20].decode()} ({len(ln2)} bytes)')
    for ln2 in lines[k + 1:k + 3]:
        g = ln2.split(b'\t')
        print(f'    after:  {g[0].decode()} {g[1].decode()} {g[2].decode()} {g[3].decode()} {g[4].decode()} '
              f'{g[5].decode()} {g[6].decode()} {g[7][:20].decode()} ({len(ln2)} bytes)')
# how long are lines, and how many are longer than a page
sizes = sorted(len(ln) + 1 for ln in lines[:-1])
big = sum(1 for s in sizes if s > 4096)
print(f'lines longer than 4096 bytes: {big} of {len(sizes)}; the longest {sizes[-1]} bytes; '
      f'bytes in such lines: {sum(s for s in sizes if s > 4096)} of {len(data)}')
