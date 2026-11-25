"""Reads the rows `review5_the_ended_arm_in_full` prints (plan146.rs) at
the head, and those of `review4_the_ended_arm_in_full` at cf53e94 (the
same grid; review/m2.6b2), and sums up what #146 changed:
python3 -I table146.py HEAD.tsv CF53E94.tsv

1. Every cell whose action or report differs between the two heads.
2. The ended arm at the head: each release or give-up cell by what
   allows it (as the fourth look's classes A, B, C, with `stopped` now
   in L7's list), and the wait cells by answer.
3. `refused stopped` against `refused rewound`, cell by cell, in every
   state (too old, eight stalls, not too old; held or asked; before or
   at `held_until`; the pool whole and held, whole and not held, or
   `pool@t` gone; the flag; what `look` saw).
4. The report: made exactly where the arm gives up on an end (L7's list
   with `stopped`), with the code; never for a publish not too old."""
import collections, sys

COLS = ["t", "ended", "phase", "time", "pool", "flag", "snap", "answer", "action", "report"]


def load(path):
    rows = {}
    for line in open(path):
        f = line.rstrip("\n").split("\t")
        if len(f) != 10 or f[0] != "TABLE" or f[1] == "ended":
            continue
        f[8] = f[8].replace(", ".join(["192"] * 32), "COMMIT").replace(", ".join(["9"] * 32), "OTHER")
        r = dict(zip(COLS, f))
        rows[tuple(f[1:8])] = r
    return rows


head, base = load(sys.argv[1]), load(sys.argv[2])
print(f"cells: {len(head)} at the head, {len(base)} at cf53e94")

print("\n1. Cells that differ (action or report)")
diff = [(k, base[k], head[k]) for k in head
        if k in base and (base[k]["action"], base[k]["report"]) != (head[k]["action"], head[k]["report"])]
print(f"  {len(diff)} cells; missing at either head: {len(set(head) ^ set(base))}")
by = collections.Counter((k[0], k[6], b["action"], h["action"], h["report"]) for k, b, h in diff)
for (ended, answer, was, now, rep), n in sorted(by.items()):
    print(f"    {n:4}  {ended:8} {answer:16} {was:22} -> {now:22} report {rep}")
print("  answers among them:", sorted({k[6] for k, _, _ in diff}))

ENDS = {"refused " + w for w in
        ["rewound", "stopped", "state", "attachment", "taken", "name", "manifest", "expiry",
         "version", "malformed", "peer", "store"]}
FLAG = {"withdrawn", "unknown", "expired"}
AWAY = {"(none: away)", "refused ended", "refused no-answer", "refused no-upload", "refused busy",
        "accepted", "cuts", "snap", "unreadable"}
old = [r for r in head.values() if r["ended"] != "young"]


def gives_up(a):
    return a == "Release" or a.startswith("Abandon")


print("\n2. The ended arm at the head: release or give-up cells, by what allows them")
why = collections.Counter()
odd = []
for r in old:
    if not gives_up(r["action"]):
        continue
    if r["answer"] in FLAG and r["flag"] == "true" and r["snap"] != "-":
        why["A: a withdraw answered withdrawn/unknown/expired, then look"] += 1
    elif r["answer"] in ENDS and r["snap"] in {"absent", "not-ours", "unreadable"}:
        why["B: an end of L7's list (stopped included), then look"] += 1
    elif r["answer"] in AWAY and r["phase"] == "asked" and r["time"] == "at-until" and r["action"] == "Release":
        why["C: asked, at held_until, an answer that cannot tell (L5)"] += 1
    else:
        odd.append(r)
for k, n in sorted(why.items()):
    print(f"  {n:5}  {k}")
print(f"  {len(odd):5}  anything else")
for r in odd[:10]:
    print("        ", r)
w = collections.Counter(r["answer"] for r in old if r["action"] == "Wait")
print("  wait cells by answer:")
for k, n in sorted(w.items()):
    print(f"    {n:5}  {k}" + ("  <- AN END" if k in ENDS else ""))
loops = [r for r in old if r["action"] == "Look" and r["snap"] != "-"]
print(f"  look where a sight is already there: {len(loops)} cells")

print("\n3. stopped against rewound, cell by cell")
pairs = collections.Counter()
for k, r in head.items():
    if r["answer"] != "refused stopped":
        continue
    rw = head[k[:6] + ("refused rewound",)]
    same = r["action"] == rw["action"]
    pairs[(r["ended"], r["pool"], r["snap"], same, r["action"], rw["action"])] += 1
for (ended, pool, snap, same, a, b), n in sorted(pairs.items()):
    mark = "same" if same else "DIFFERS"
    print(f"    {n:3}  {ended:8} {pool:12} {snap:17} {mark:7} stopped {a:22} rewound {b}")
stop_only = collections.Counter(
    (r["ended"], r["phase"], r["time"], r["pool"], r["action"]) for r in head.values()
    if r["answer"] == "refused stopped")
print("  stopped, by state (the flag and the look folded):")
for k, n in sorted(stop_only.items()):
    print(f"    {n:3}  {' / '.join(k)}")

print("\n4. The report")
bad = []
told = collections.Counter()
for r in head.values():
    want = "None"
    if r["ended"] != "young" and gives_up(r["action"]) and r["answer"] in ENDS:
        want = "Some(Refused(" + r["answer"].split()[1].title().replace("-", "") + "))"
    if r["report"] != want:
        bad.append((r, want))
    if r["report"] != "None":
        told[(r["ended"], r["answer"], r["report"])] += 1
print(f"  cells where the report is not as L7 says: {len(bad)}")
for r, want in bad[:10]:
    print("   ", r, "want", want)
print(f"  cells with a report: {sum(told.values())}; not too old: "
      f"{sum(n for k, n in told.items() if k[0] == 'young')}")
for k, n in sorted(told.items()):
    if k[1] in ("refused stopped", "refused rewound"):
        print(f"    {n:4}  {' / '.join(k)}")
