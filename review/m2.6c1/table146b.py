"""Reads the rows `review5_the_ended_arm_in_full` prints (plan146.rs) at
4ff4549 and at cd3a67c, and sums up what 4ff4549 changed:
python3 -I table146b.py 4FF4549.tsv CD3A67C.tsv

1. Every cell whose action or report differs between the two heads.
2. `refused stopped` not too old, by phase, pool and what `look` saw
   (`-`: not looked, or `look` answered a refusal such as `store` or
   `no-answer`, which `snap_seen` reads as nothing seen), with the time
   (before or at `held_until`) and the flag folded where they agree.
3. Where a publish not too old is released or given up at 4ff4549, by
   answer, against cd3a67c: only `refused stopped` is new.
4. The report: made exactly where the arm gives up on an end too old or
   stalled (L7), or on `refused stopped` at any age (the ruling), with
   the code; nowhere else."""
import collections, sys

COLS = ["t", "ended", "phase", "time", "pool", "flag", "snap", "answer", "action", "report"]


def load(path):
    rows = {}
    for line in open(path):
        f = line.rstrip("\n").split("\t")
        if len(f) != 10 or f[0] != "TABLE" or f[1] == "ended":
            continue
        f[8] = f[8].replace(", ".join(["192"] * 32), "COMMIT").replace(", ".join(["9"] * 32), "OTHER")
        rows[tuple(f[1:8])] = dict(zip(COLS, f))
    return rows


head, base = load(sys.argv[1]), load(sys.argv[2])
print(f"cells: {len(head)} at 4ff4549, {len(base)} at cd3a67c; "
      f"missing at either: {len(set(head) ^ set(base))}")


def gives_up(a):
    return a == "Release" or a.startswith("Abandon")


print("\n1. Cells that differ (action or report)")
diff = [(k, base[k], head[k]) for k in head
        if k in base and (base[k]["action"], base[k]["report"]) != (head[k]["action"], head[k]["report"])]
print(f"  {len(diff)} cells")
by = collections.Counter((k[0], k[6], b["action"], h["action"], h["report"]) for k, b, h in diff)
for (ended, answer, was, now, rep), n in sorted(by.items()):
    print(f"    {n:4}  {ended:8} {answer:16} {was:14} -> {now:30} report {rep}")
print("  states among them:", sorted({k[0] for k, _, _ in diff}), sorted({k[6] for k, _, _ in diff}))

print("\n2. refused stopped, not too old (time and flag folded)")
cells = collections.defaultdict(set)
for k, r in head.items():
    if r["ended"] == "young" and r["answer"] == "refused stopped":
        cells[(r["phase"], r["pool"], r["snap"])].add((r["action"], r["report"]))
print(f"  {'phase':6} {'pool':13} {'look saw':18} action / report")
for (phase, pool, snap), got in sorted(cells.items()):
    mark = "" if len(got) == 1 else "   <- differs by time or flag"
    for action, rep in sorted(got):
        print(f"  {phase:6} {pool:13} {snap:18} {action} / {rep}{mark}")
rw = collections.Counter()
for k, r in head.items():
    if r["answer"] == "refused stopped" and r["ended"] == "young":
        other = head[k[:6] + ("refused rewound",)]
        rw[(r["action"] == other["action"])] += 1
print(f"  against refused rewound at the same cell: same {rw[True]}, differs {rw[False]}")

print("\n3. Released or given up, not too old, by answer")
now = collections.Counter(r["answer"] for r in head.values() if r["ended"] == "young" and gives_up(r["action"]))
was = collections.Counter(r["answer"] for r in base.values() if r["ended"] == "young" and gives_up(r["action"]))
for a in sorted(set(now) | set(was)):
    print(f"    {a:20} cd3a67c {was[a]:4}  4ff4549 {now[a]:4}" + ("   <- new" if now[a] != was[a] else ""))

print("\n4. The report")
ENDS = {"refused " + w for w in
        ["rewound", "stopped", "state", "attachment", "taken", "name", "manifest", "expiry",
         "version", "malformed", "peer", "store"]}
bad = []
told = collections.Counter()
for r in head.values():
    want = "None"
    if gives_up(r["action"]) and r["answer"] in ENDS and (
            r["ended"] != "young" or r["answer"] == "refused stopped"):
        want = "Some(Refused(" + r["answer"].split()[1].title().replace("-", "") + "))"
    if r["report"] != want:
        bad.append((r, want))
    if r["report"] != "None":
        told[(r["ended"], r["answer"], r["report"])] += 1
print(f"  cells where the report is not as L7 and the ruling say: {len(bad)}")
for r, want in bad[:10]:
    print("   ", r, "want", want)
print(f"  cells with a report: {sum(told.values())}; not too old: "
      f"{sum(n for k, n in told.items() if k[0] == 'young')}")
for k, n in sorted(told.items()):
    if k[0] == "young":
        print(f"    {n:4}  {' / '.join(k)}")
