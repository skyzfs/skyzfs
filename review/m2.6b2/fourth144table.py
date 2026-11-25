"""Reads the rows `review4_the_ended_arm_in_full` prints (fourth144plan.rs)
and sums up the ended arm of `onward` at cf53e94, with `report`:
python3 -I fourth144table.py ROWS.tsv

1. For each answer, the actions it leads to in the ended arm.
2. Each cell that releases the hold or gives up, by what says that no
   request pends there.
3. Each cell that waits, by answer: is any an end (L7's list)?
4. Look: only where nothing was looked at yet (no loop on a sight)?
5. The report: made exactly where the arm gives up on an end, with the
   code; never for a publish not too old.
6. Too old against eight stalls: the same actions but the reason."""
import collections, sys

ENDS = {"refused " + w for w in
        ["rewound", "state", "attachment", "taken", "name", "manifest", "expiry",
         "version", "malformed", "peer", "store"]}
FLAG = {"withdrawn", "unknown", "expired"}
AWAY = {"(none: away)", "refused stopped", "refused ended", "refused no-answer",
        "refused no-upload", "refused busy", "accepted", "cuts", "snap", "unreadable"}

rows = []
for line in open(sys.argv[1]):
    f = line.rstrip("\n").split("\t")
    if len(f) != 10 or f[0] != "TABLE" or f[1] == "ended":
        continue
    f[8] = f[8].replace(", ".join(["192"] * 32), "COMMIT").replace(", ".join(["9"] * 32), "OTHER")
    rows.append(dict(zip(["t", "ended", "phase", "time", "pool", "flag", "snap", "answer",
                          "action", "report"], f)))
old = [r for r in rows if r["ended"] != "young"]
young = [r for r in rows if r["ended"] == "young"]
print(f"{len(rows)} cells: {len(old)} too old or stalled eight times, {len(young)} not (control)")

def gives_up(a):
    return a == "Release" or a.startswith("Abandon")

print("\n1. Actions by answer, too old or stalled (all states)")
by = collections.OrderedDict()
for r in old:
    by.setdefault(r["answer"], collections.Counter())[r["action"]] += 1
for answer, acts in by.items():
    print(f"  {answer:22} " + ", ".join(f"{a} x{n}" for a, n in sorted(acts.items())))

print("\n2. Release or give-up cells, by what allows them")
why = collections.Counter()
odd = []
for r in old:
    a = r["action"]
    if not gives_up(a):
        continue
    if r["answer"] in FLAG and r["flag"] == "true" and r["snap"] != "-":
        why["A: a withdraw answered withdrawn/unknown/expired, then look"] += 1
    elif r["answer"] in ENDS and r["snap"] in {"absent", "not-ours", "unreadable"}:
        why["B: an end of L7's list, then look (absent, another's, unreadable)"] += 1
    elif r["answer"] in AWAY and r["phase"] == "asked" and r["time"] == "at-until" and a == "Release":
        why["C: asked, at held_until, an answer that cannot tell (L5)"] += 1
    else:
        odd.append(r)
for k, n in sorted(why.items()):
    print(f"  {n:5}  {k}")
print(f"  {len(odd):5}  anything else")
for r in odd[:20]:
    print("        ", r)
print("  B by answer and action:")
b = collections.Counter((r["answer"], r["snap"], r["action"]) for r in old
                        if gives_up(r["action"]) and r["answer"] in ENDS)
for k, n in sorted(b.items()):
    print(f"    {n:4}  {' / '.join(k)}")

print("\n3. Wait cells, by answer (too old or stalled)")
w = collections.Counter(r["answer"] for r in old if r["action"] == "Wait")
for k, n in sorted(w.items()):
    flag = "  <- AN END OF L7'S LIST" if k in ENDS else ""
    print(f"  {n:5}  {k}{flag}")
print("  held, whole and held, Wait in every state of flag, snap and time:")
for answer in sorted({r["answer"] for r in old}):
    cells = [r for r in old if r["answer"] == answer and r["phase"] == "held"
             and r["pool"] == "whole-held"]
    if cells and all(r["action"] == "Wait" for r in cells):
        print(f"    {answer}")

print("\n4. Look where a sight is already there (a loop)?")
loops = [r for r in old if r["action"] == "Look" and r["snap"] != "-"]
print(f"  {len(loops)} cells")
for r in loops[:10]:
    print("   ", r)

print("\n5. The report")
bad = []
told = collections.Counter()
for r in rows:
    want = "None"
    if r["ended"] != "young" and gives_up(r["action"]) and r["answer"] in ENDS:
        want = "Some(Refused(" + r["answer"].split()[1].title().replace("-", "") + "))"
    if r["report"] != want:
        bad.append((r, want))
    if r["report"] != "None":
        told[(r["ended"], r["answer"], r["action"], r["report"])] += 1
print(f"  cells where the report is not as L7 says: {len(bad)}")
for r, want in bad[:10]:
    print("   ", r, "want", want)
print(f"  cells with a report: {sum(told.values())}; not too old: "
      f"{sum(n for k, n in told.items() if k[0] == 'young')}")
for k, n in sorted(told.items()):
    print(f"    {n:4}  {' / '.join(k)}")

print("\n6. Too old against eight stalls")
key = lambda r: (r["phase"], r["time"], r["pool"], r["flag"], r["snap"], r["answer"])
a = {key(r): r for r in rows if r["ended"] == "too-old"}
s = {key(r): r for r in rows if r["ended"] == "stalls-8"}
diff = [(k, a[k]["action"], s[k]["action"]) for k in a
        if a[k]["action"].replace("TooOld", "X") != s[k]["action"].replace("NoProgress", "X")]
print(f"  cells that differ but for the reason: {len(diff)}")
for d in diff[:10]:
    print("   ", d)

print("\n7. done, by what look saw (too old or stalled)")
for snap in ["-", "absent", "ours", "ours-other-commit", "not-ours", "unreadable"]:
    acts = collections.Counter(r["action"] for r in old if r["answer"] == "done" and r["snap"] == snap)
    print(f"  {snap:18} {dict(acts)}")
