#!/usr/bin/env python3
"""The plants of the review of #179 at its fourth head, e6530f1 (the code
of 5caf2ef): the stuck names' turn (Q1-Q9, and Q2x/Q7x with the test's
exact-once assertion taken out too), then plants of plants2.json and
plants.json again where they survived or bear on the turn.

Usage: mk3.py TREE OUT.json  (checks that each old text occurs once)
"""
import json, os, sys

AG = ["cargo", "test", "-p", "skyzfs-agent", "--lib"]
RET = "crates/skyzfs-agent/src/engine/retain.rs"
RT = "crates/skyzfs-agent/src/engine/retain/tests.rs"
ENG = "crates/skyzfs-agent/src/engine.rs"
P = []

def plant(pid, what, cmd, *edits):
    P.append({"id": pid, "what": what, "cmd": cmd, "edits": [list(e) for e in edits]})

START = "            Some(last) => stuck.iter().position(|(n, _)| n > last).unwrap_or(0),"
ONCE = '    assert!(more.iter().all(|m| *m == 1), "{more:?}");\n'
plant("Q1", "the turn begun again at the first name each round", AG,
      (RET, START, "            Some(_) => 0,"))
plant("Q2", "the turn begins at the last name tried, not after it", AG,
      (RET, START, START.replace("n > last", "n >= last")))
plant("Q2x", "Q2, and the test's exact-once assertion taken out", AG,
      (RET, START, START.replace("n > last", "n >= last")), (RT, ONCE, ""))
plant("Q3", "the cap one high: 9 a round", AG,
      (RET, "        stuck.truncate(STUCK_STEPS);", "        stuck.truncate(STUCK_STEPS + 1);"))
plant("Q4", "the cap one low: 7 a round", AG,
      (RET, "        stuck.truncate(STUCK_STEPS);", "        stuck.truncate(STUCK_STEPS - 1);"))
plant("Q5", "no cap: every name stuck each round", AG,
      (RET, "        stuck.truncate(STUCK_STEPS);\n", ""))
plant("Q6", "the turn moves only past a name whose destroy succeeded", AG,
      (RET, "            self.retention.stuck_last = Some(name.clone());\n            if self.destroy_members(&name, &members)? {\n",
            "            if self.destroy_members(&name, &members)? {\n                self.retention.stuck_last = Some(name.clone());\n"))
Q7 = (RET, "        stuck.truncate(STUCK_STEPS);\n        for (name, members) in stuck {\n            self.at.clone_from(&name);\n            self.retention.stuck_last = Some(name.clone());\n",
      "        stuck.truncate(STUCK_STEPS);\n        self.retention.stuck_last = stuck.first().map(|(n, _)| n.clone());\n        for (name, members) in stuck {\n            self.at.clone_from(&name);\n")
plant("Q7", "the turn moves one name a round (past the first of the window)", AG, Q7)
plant("Q7x", "Q7, and the test's exact-once assertion taken out", AG, Q7, (RT, ONCE, ""))
plant("Q8", "no sort: the list's own order (the oldest stuck first)", AG,
      (RET, "        stuck.sort_by(|a, b| a.0.cmp(&b.0));\n", ""))
plant("Q9", "the turn begun again at an engine restart within the process", AG,
      (ENG, "        self.retention.told.clear();\n        self.assists = 0;",
            "        self.retention.told.clear();\n        self.retention.stuck_last = None;\n        self.assists = 0;"))

here = os.path.dirname(os.path.abspath(__file__))
old = json.load(open(os.path.join(here, "plants2.json"))) + json.load(open(os.path.join(here, "plants.json")))
for pid in ["N1", "N10", "N11", "N12", "M5", "M7", "P2", "M8b", "R21", "R22", "E3", "E4", "E4b", "E8", "G11"]:
    x = next(p for p in old if p["id"] == pid)
    y = dict(x)
    y["id"] = pid + "@e65"
    y["what"] = x["what"] + " (again, at e6530f1)"
    P.append(y)

tree, out = sys.argv[1], sys.argv[2]
bad = 0
for p in P:
    texts = {}
    for f, old_text, new in p["edits"]:
        t = texts.setdefault(f, open(os.path.join(tree, f)).read())
        n = t.count(old_text)
        if n != 1:
            print(f"{p['id']}: {f}: {n} matches", file=sys.stderr)
            bad += 1
        texts[f] = t.replace(old_text, new)
json.dump(P, open(out, "w"), indent=1)
print(f"{len(P)} plants, {bad} bad")
sys.exit(1 if bad else 0)
