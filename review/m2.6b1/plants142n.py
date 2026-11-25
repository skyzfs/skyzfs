"""Plants of the row `an_earlier_request_that_landed_is_this_publishs`
(#142 at 05d79f4), for the review's second brief: does the row catch a
planner that compares the request's count, and one that treats the hold
gone (`unheld`) otherwise, and what catches each when the row, or one of
its asserts, is dropped. Each plant is a list of textual edits, applied
one plant at a time with one test variant, and every file restored from
its saved text in `finally`. Run from a worktree of the head:
WORKTREE=<path> OUT=<json> python3 plants142n.py
The suite skips the review's own scratch tests (`--skip review`)."""
import json, os, subprocess, time

W = os.environ.get("WORKTREE", ".")
OUT = os.environ.get("OUT", "plants142n.json")
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
SUITE = ["cargo", "test", "-p", "skyzfs-agent", "--lib", "--", "--skip", "review"]
PL = "crates/skyzfs-agent/src/plan.rs"
TS = "crates/skyzfs-agent/src/plan/tests.rs"

LOOK_OURS = "        Some(SnapSeen::Ours(commit)) => Action::Done(commit),\n        Some(SnapSeen::NotOurs | SnapSeen::Unreadable) => give_up(pool, Why::NameTaken),\n"
# C1: the planner keeps the row `look` saw (here in a thread-local set by
# `snap_seen`; libtest runs each test on a thread of its own) and takes it
# as ours only where its cut is of the count recorded last.
TL_USE = "use crate::record::{Count, Member, Phase, Publish, Taken, Why};\n"
TL_DEF = TL_USE + "\nthread_local! {\n    static LAST_ROW: std::cell::Cell<Option<([u8; 16], u64)>> = const { std::cell::Cell::new(None) };\n}\n"
SNAP_OURS = "            {\n                SnapSeen::Ours(row.commit)\n"
SNAP_SET = "            {\n                LAST_ROW.with(|c| c.set(Some((row.state, row.number))));\n                SnapSeen::Ours(row.commit)\n"


def count_plant(cond):
    return [
        (PL, TL_USE, TL_DEF),
        (PL, SNAP_OURS, SNAP_SET),
        (PL, LOOK_OURS,
         "        Some(SnapSeen::Ours(commit)) if LAST_ROW.with(|c| c.get()).is_none_or(|(s, n)| " + cond + ") => Action::Done(commit),\n"
         "        Some(SnapSeen::Ours(_) | SnapSeen::NotOurs | SnapSeen::Unreadable) => give_up(pool, Why::NameTaken),\n"),
    ]


TAKEN = "            Code::Attachment | Code::State | Code::Taken => look(&|| anew(false)),\n"
PLANTS = [
    ("C1 the row's cut compared with the count recorded last (state and number)",
     count_plant("s == publish.count.state && n > publish.count.after")),
    ("C1n the row's number compared with the count recorded last",
     count_plant("n > publish.count.after")),
    ("C1s the row's state compared with the count recorded last",
     count_plant("s == publish.count.state")),
    ("C2 the daemon's `refused taken` trusted (its check compares the request, count and all)",
     [(PL, TAKEN, "            Code::Attachment | Code::State => look(&|| anew(false)),\n            Code::Taken => give_up(pool, Why::NameTaken),\n")]),
    ("U1 ours with the hold gone: the hold placed again before done",
     [(PL, LOOK_OURS,
       "        Some(SnapSeen::Ours(commit)) if pool.held => Action::Done(commit),\n        Some(SnapSeen::Ours(_)) => Action::Hold,\n"
       "        Some(SnapSeen::NotOurs | SnapSeen::Unreadable) => give_up(pool, Why::NameTaken),\n")]),
    ("U2 ours with the hold gone: the name given up",
     [(PL, LOOK_OURS,
       "        Some(SnapSeen::Ours(commit)) if pool.held => Action::Done(commit),\n"
       "        Some(SnapSeen::Ours(_) | SnapSeen::NotOurs | SnapSeen::Unreadable) => give_up(pool, Why::NameTaken),\n")]),
    ("U3 `refused taken` with the hold gone: asked anew (hold first) without a look",
     [(PL, TAKEN, "            Code::Taken if !pool.held => anew(false),\n" + TAKEN)]),
]
VARIANTS = [
    ("as is", []),
    ("row dropped", [(TS, "#[test]\nfn an_earlier_request_that_landed_is_this_publishs() {\n",
                      "#[allow(dead_code)]\nfn an_earlier_request_that_landed_is_this_publishs() {\n")]),
    ("row's unheld assert dropped", [(TS, "    assert_eq!(plan(&anew, &landed().unheld()), Action::Done(COMMIT));\n", "")]),
    ("row's held assert dropped", [(TS, "    assert_eq!(plan(&anew, &landed()), Action::Done(COMMIT));\n", "")]),
]

res = []


def run(name, edits):
    saved = {}
    for f, _, _ in edits:
        p = os.path.join(W, f)
        if p not in saved:
            with open(p) as h:
                saved[p] = h.read()
    texts = dict(saved)
    for f, old, new in edits:
        p = os.path.join(W, f)
        n = texts[p].count(old)
        if n != 1:
            return f"NOT APPLIED ({f}: found {n})"
        texts[p] = texts[p].replace(old, new)
    verdict = "SURVIVED"
    try:
        for p, t in texts.items():
            with open(p, "w") as h:
                h.write(t)
        try:
            r = subprocess.run(SUITE, cwd=W, env=ENV, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired:
            return "KILLED (timeout, a hang)"
        out = r.stdout + r.stderr
        if "error[E" in out or "could not compile" in out:
            print(out[-3000:])
            return "BUILD ERROR"
        if r.returncode != 0:
            failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
            verdict = "KILLED: " + ", ".join(f.removeprefix("plan::tests::") for f in failed)
        else:
            ok = [l for l in out.splitlines() if l.startswith("test result:")]
            verdict += " (" + (ok[0] if ok else "?") + ")"
    finally:
        for p, t in saved.items():
            with open(p, "w") as h:
                h.write(t)
    return verdict


for vname, vedits in VARIANTS:
    t = time.time()
    v = run(f"baseline, {vname}", vedits)
    res.append((f"(no plant) / {vname}", v))
    print(f"(no plant) / {vname}: {v} ({time.time() - t:.0f}s)", flush=True)
for pname, pedits in PLANTS:
    for vname, vedits in VARIANTS:
        t = time.time()
        v = run(pname, pedits + vedits)
        res.append((f"{pname} / {vname}", v))
        print(f"{pname} / {vname}: {v} ({time.time() - t:.0f}s)", flush=True)
        with open(OUT, "w") as h:
            json.dump(res, h, indent=1)
with open(OUT, "w") as h:
    json.dump(res, h, indent=1)
print("done")
