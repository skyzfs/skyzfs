#!/usr/bin/env python3
"""Which case of the T12 test (a_give_up_is_told_at_least_once_whatever_the_stop)
fails first under each of the plants Q4, Q7, Q7b and Q16 of plants157.py; and
Q7b with the test's double-stop cases taken out. usage: python3 -I t12cases157.py WORKTREE"""
import pathlib, re, subprocess, sys, importlib.util
ROOT = pathlib.Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("p", pathlib.Path(__file__).with_name("plants157.py"))
src = pathlib.Path(__file__).with_name("plants157.py").read_text().replace("\nmain()\n", "\n")
ns = {"__name__": "p"}
sys.argv = [sys.argv[0], str(ROOT)]
exec(compile(src, "plants157.py", "exec"), ns)
by_id = {p[0]: p for p in ns["P"]}
ENGT = "crates/skyzfs-agent/src/engine/tests.rs"
DOUBLE = ("    for stop in about.into_iter().filter(|p| p.effect != Effect::Destroy) {\n"
          "        cases.push((true, vec![at(Effect::Failed, true), stop]));\n    }\n")
CMD = ["cargo", "test", "-p", "skyzfs-agent", "--lib", "a_give_up_is_told_at_least_once", "--", "--nocapture"]
def go(label, files):
    saved = {}
    try:
        for rel, subs in files:
            path = ROOT / rel
            saved.setdefault(rel, path.read_text())
            t = path.read_text()
            for old, new in subs:
                assert t.count(old) == 1, (label, rel, old[:40])
                t = t.replace(old, new)
            path.write_text(t)
        out = subprocess.run(CMD, cwd=ROOT, capture_output=True, text=True, timeout=600)
        text = out.stdout + out.stderr
        m = re.search(r"not as before (true|false), stops (\[[^\]]*\])", text)
        res = "PASSED" if out.returncode == 0 else ("FAILED first at: not as before %s, stops %s" % m.groups() if m else "FAILED (no case line)")
        print(f"{label}: {res}", flush=True)
    finally:
        for rel, t in saved.items():
            (ROOT / rel).write_text(t)
for pid in ["Q4", "Q7", "Q7b", "Q16"]:
    go(pid, by_id[pid][2])
go("Q7b without the double-stop cases", by_id["Q7b"][2] + [(ENGT, [(DOUBLE, "")])])
go("Q4 without the double-stop cases", by_id["Q4"][2] + [(ENGT, [(DOUBLE, "")])])
go("Q16 without the double-stop cases", by_id["Q16"][2] + [(ENGT, [(DOUBLE, "")])])
go("head without the double-stop cases (control)", [(ENGT, [(DOUBLE, "")])])
st = subprocess.run(["git", "status", "--short"], cwd=ROOT, capture_output=True, text=True)
print("worktree after:", st.stdout.strip() or "clean")
