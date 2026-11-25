"""Reads the rows `review3_the_ended_arm_in_full` prints (third144plan.rs)
and sums up the ended arm of `onward` at 1b5e5f9:
python3 third144table.py ROWS.tsv

1. For each answer, the actions it leads to, and in which states.
2. Each cell that releases the hold or gives up, with what says that no
   request pends there.
3. Each cell that waits on an answer which says the request has ended
   (the uploader's own ends: `refused rewound`, `refused state`,
   `refused taken`), which the daemon keeps answering."""
import collections, sys

rows = []
for line in open(sys.argv[1]):
    f = line.rstrip("\n").split("\t")
    if len(f) != 9 or f[0] != "TABLE":
        continue
    f[8] = f[8].replace(", ".join(["192"] * 32), "COMMIT").replace(", ".join(["9"] * 32), "OTHER")
    rows.append(dict(zip(["t", "ended", "phase", "time", "pool", "flag", "snap", "answer", "action"], f)))
print(f"{len(rows)} cells")

# 1. By answer.
print("\n1. Actions by answer (all states)")
by = collections.OrderedDict()
for r in rows:
    by.setdefault(r["answer"], collections.Counter())[r["action"]] += 1
for answer, acts in by.items():
    print(f"  {answer:22} " + ", ".join(f"{a} x{n}" for a, n in sorted(acts.items())))

# 2. A release or a give-up: what allows it.
ENDS = {"withdrawn", "unknown", "expired"}
AWAY = {"(none: away)", "refused stopped", "refused ended", "refused no-answer", "refused no-upload"}
print("\n2. Release or give-up cells, by what allows them")
why = collections.Counter()
odd = []
for r in rows:
    a = r["action"]
    if not (a == "Release" or a.startswith("Abandon")):
        continue
    if r["answer"] in ENDS and r["flag"] == "true":
        why["a withdraw answered withdrawn/unknown/expired, then look"] += 1
    elif r["answer"] == "refused taken" and r["snap"] in {"not-ours", "unreadable"}:
        why["refused taken, and look sees another's snap/t (NameTaken)"] += 1
    elif r["phase"] == "asked" and r["time"] == "at-until" and a == "Release":
        why["asked, at held_until, an answer that cannot tell (L5)"] += 1
    else:
        odd.append(r)
for k, n in why.items():
    print(f"  {n:5}  {k}")
print(f"  {len(odd):5}  anything else")
for r in odd[:20]:
    print("        ", r)

# 3. Waits on an end the daemon keeps answering.
print("\n3. Waits on an answer that says the request ended (kept by the uploader)")
stuck = collections.Counter()
for r in rows:
    if r["action"] != "Wait":
        continue
    if r["answer"] in {"refused rewound", "refused state"} or (
        r["answer"] == "refused taken" and r["snap"] == "absent"
    ):
        held = "hold kept" if r["pool"] == "whole-held" and not (r["phase"] == "asked" and r["time"] == "at-until") else "no hold"
        stuck[(r["answer"], r["phase"], r["time"], r["pool"], held)] += 1
for k, n in sorted(stuck.items()):
    print(f"  {n:4}  {' / '.join(k)}")
# And the same answers that do end: which states.
print("\n   ... and where those answers lead elsewhere:")
other = collections.Counter()
for r in rows:
    if r["answer"] in {"refused rewound", "refused state", "refused taken"} and r["action"] != "Wait":
        other[(r["answer"], r["action"])] += 1
for k, n in sorted(other.items()):
    print(f"  {n:4}  {' -> '.join(k)}")

# 4. Away answers held without end, by phase.
print("\n4. Answers of a daemon away (and none) at Held, whole and held: always Wait?")
for answer in sorted(AWAY):
    acts = {r["action"] for r in rows if r["answer"] == answer and r["phase"] == "held" and r["pool"] == "whole-held"}
    print(f"  {answer:22} {sorted(acts)}")

# 5. done that look does not confirm.
print("\n5. done, by what look saw")
for snap in ["-", "absent", "ours", "ours-other-commit", "not-ours", "unreadable"]:
    acts = collections.Counter(r["action"] for r in rows if r["answer"] == "done" and r["snap"] == snap)
    print(f"  {snap:18} {dict(acts)}")
