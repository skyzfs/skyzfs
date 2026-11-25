"""Plants of the next look at #146, at 0213a6c: the plants of
`plants146b.py` (the look at 4ff4549), read from that file, and this
look's N8* plants of the fix (26e83c5). Each is applied alone in a clean
worktree of the head, then `cargo test -p skyzfs-agent --lib` (no
scratch test), then the file restored, as `plants146b.py` does.

WORKTREE=<path> OUT=<json> python3 -I plants146c.py [name prefixes...]
(DRY=1 only checks that each plant applies)."""
import json, os, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = open(os.path.join(HERE, "plants146b.py")).read()
# The definitions only: the runner of plants146b.py is below `want =`.
NS = {"__name__": "plants146b", "__file__": os.path.join(HERE, "plants146b.py")}
exec(compile(SRC.split("\nwant = sys.argv[1:]\n")[0], "plants146b.py", "exec"), NS)
P, EN, W, ENV, SUITE = NS["P"], NS["EN"], NS["W"], NS["ENV"], NS["SUITE"]
OUT = os.environ.get("OUT", "plants146c.json")

KEEP = ("        if !matches!(action, Action::Look | Action::Release) {\n"
        "            let memory = self.memory.entry(name.clone()).or_default();\n"
        "            memory.answer = None;\n"
        "            memory.snap = None;\n"
        "        }\n")
SEE_KEPT = "        let kept = memory.answer.clone();\n"
P = [p for p in P if not p[0].startswith("Z1 ")] + [
    ("N8r the fix reverted: the answer and the sight cleared at a release (4ff4549)",
     [(EN, "        if !matches!(action, Action::Look | Action::Release) {\n",
       "        if action != Action::Look {\n")]),
    ("N8a the sight kept across a release, the answer not",
     [(EN, KEEP, KEEP + "        if action == Action::Release {\n"
       "            self.memory.entry(name.clone()).or_default().answer = None;\n        }\n")]),
    ("N8s the answer kept across a release, the sight not (one more look)",
     [(EN, KEEP, KEEP + "        if action == Action::Release {\n"
       "            self.memory.entry(name.clone()).or_default().snap = None;\n        }\n")]),
    ("N8d the answer dropped at the look instead (kept across a release only)",
     [(EN, KEEP, KEEP.replace("Action::Look | Action::Release", "Action::Release")
       + "        if action == Action::Look {\n"
       "            self.memory.entry(name.clone()).or_default().answer = None;\n        }\n")]),
    ("N8e the kept answer read once (taken by `see`)",
     [(EN, SEE_KEPT, "        let kept = memory.answer.take();\n")]),
    ("N8f the answer and the sight never cleared",
     [(EN, KEEP, "")]),
    ("N8g the answer and the sight kept across a hold too",
     [(EN, KEEP, KEEP.replace("Action::Look | Action::Release", "Action::Look | Action::Release | Action::Hold"))]),
    ("N8h kept across a release only after `stopped` (a narrower fix)",
     [(EN, KEEP, KEEP.replace(
         "        if !matches!(action, Action::Look | Action::Release) {\n",
         "        let stopped = matches!(\n"
         "            self.memory.get(&name).and_then(|m| m.answer.as_ref()),\n"
         "            Some(Reply::Refused(skyzfs_control::Code::Stopped, _))\n"
         "        );\n"
         "        if !(action == Action::Look || (action == Action::Release && stopped)) {\n"))]),
]

want = sys.argv[1:]
res = []
for name, edits in P:
    if want and not any(name.startswith(w + " ") or name == w for w in want):
        continue
    files = sorted({os.path.join(W, f) for f, _, _ in edits})
    saved = {p: open(p).read() for p in files}
    texts = dict(saved)
    bad = None
    for f, old, new in edits:
        path = os.path.join(W, f)
        n = texts[path].count(old)
        if n != 1:
            bad = f"NOT APPLIED ({f}: found {n})"
            break
        texts[path] = texts[path].replace(old, new)
    if bad:
        res.append((name, bad))
        print(f"{name}: {bad}", flush=True)
        continue
    if os.environ.get("DRY"):
        print(f"{name}: applies", flush=True)
        continue
    t = time.time()
    verdict = "SURVIVED"
    try:
        for path, text in texts.items():
            open(path, "w").write(text)
        try:
            r = subprocess.run(SUITE, cwd=W, env=ENV, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired:
            r = None
            verdict = "KILLED (timeout, a hang)"
        if r is not None:
            out = r.stdout + r.stderr
            if "error[E" in out or "could not compile" in out:
                verdict = "BUILD ERROR"
                print(out[-3000:])
            elif r.returncode != 0:
                failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
                verdict = "KILLED: " + ", ".join(failed[:4]) + (f" and {len(failed) - 4} more" if len(failed) > 4 else "")
    finally:
        for path, text in saved.items():
            open(path, "w").write(text)
    res.append((name, verdict))
    print(f"{name}: {verdict} ({time.time() - t:.0f}s)", flush=True)
    json.dump(res, open(OUT, "w"), indent=1)
print("done")
