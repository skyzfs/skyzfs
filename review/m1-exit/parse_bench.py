"""Parse one topology-bench summary.md: the machine(s), each row's tps, and each
daemon row's build, serve arguments, memory limit, bracket width and ratio.
Usage: python3 -I parse_bench.py JOBNUM RUNID SUMMARY.md -> one compact block."""
import re, sys
num, run, path = sys.argv[1], sys.argv[2], sys.argv[3]
t = open(path, encoding='utf-8', errors='replace').read()
lines = t.splitlines()
machines = set()
for l in lines:
    m = re.match(r'^\| ([0-9a-f-]{36}) (\S+) \| ([^|]+)\| ([\d.]+) \| ([\d.]+) \| (\d+) \|', l)
    if m: machines.add((m.group(1)[:8], m.group(3).strip(), m.group(6)))
tps = {}
for l in lines:
    m = re.match(r'^\| tpcb \| (zfs-\S+ \[[^\]]+\]) \| (\d+) \|', l)
    if m and m.group(1) not in tps: tps[m.group(1)] = int(m.group(2))
rows = []
for l in lines:
    m = re.match(r'^\| tpcb \| (zfs-skyzfsd \[[^\]]+\]) \(row (\d+)\) \| (\S+) .*?\); ([^;|]+); ([^|]+)\| (\d+) \| [^|]+\| [^|]+\| ([\d.]+)% \| ([\d.]+) \| ([^|]*)\|', l)
    if m: rows.append((int(m.group(2)), m.group(1), m.group(3)[:7], m.group(4).strip(), m.group(5).strip(), int(m.group(6)), m.group(7), m.group(8), m.group(9).strip()[:40]))
builds = {r[2] for r in rows}; args = {r[3] for r in rows}
print(f"job {num} run {run}: machines {sorted(machines)}; builds {sorted(builds)}; serve args {sorted(args)}")
print("  tps:", "; ".join(f"{k.split('[')[1][:-1]} {v}" for k, v in sorted(tps.items(), key=lambda kv: kv[0])))
for r in sorted(rows):
    print(f"  row {r[0]} {r[1].split('[')[1][:-1]} mem {r[4]} tps {r[5]} width {r[6]}% ratio {r[7]} verdict {r[8]}")
