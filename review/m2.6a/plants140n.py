"""Textual plants for the next look at #140 (M2.6a), one at a time, each
file restored from its saved text in `finally`. The Rust at 72a3b5e is
the Rust at 7d1152a. Run from a worktree:
WORKTREE=<path> OUT=<json> python3 plants140n.py [name prefixes...]"""
import json, os, subprocess, sys, time

W = os.environ.get("WORKTREE", ".")
OUT = os.environ.get("OUT", "plants140n.json")
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
FEAT = "skyzfsd/test-hooks,skyzfs-local/test-hooks"
SUITES = {
    "l": ["cargo", "test", "-p", "skyzfs-local", "--", "--skip", "review"],
    "u": ["cargo", "test", "-p", "skyzfs-upload", "--", "--skip", "review"],
    "d": ["cargo", "test", "-p", "skyzfsd", "--features", FEAT, "--", "--skip", "review"],
    "c": ["cargo", "test", "-p", "skyzfs-control"],
}
CU = "crates/skyzfs-local/src/inplace/cuts.rs"
PB = "crates/skyzfs-upload/src/publish.rs"
UL = "crates/skyzfs-upload/src/uploader.rs"
LK = "crates/skyzfs-upload/src/look.rs"
SE = "bins/skyzfsd/src/serve.rs"
HD = "bins/skyzfsd/src/control/handler.rs"
CC = "crates/skyzfs-control/src/lib.rs"
P = [
 # Rule (R'): the running maximum.
 ("Q1 the top's group in another state is no rewind", CU,
  "state.txg < top.txg || (state.txg == top.txg && top.state != Some(*state))",
  "state.txg < top.txg", "l"),
 ("Q2 the top's own group is a rewind (<=)", CU,
  "state.txg < top.txg || (state.txg == top.txg && top.state != Some(*state))",
  "state.txg <= top.txg", "l"),
 ("Q3 the rings at the start raise no top", CU,
  "                if uberblock.verified() && uberblock.counts() {\n                    self.saw(uberblock.state);\n",
  "                if uberblock.verified() && uberblock.counts() {\n                    let _ = uberblock.state;\n", "l"),
 ("Q4 an applied uberblock raises no top", CU,
  "            let state = uberblock.state;\n            self.saw(state);\n",
  "            let state = uberblock.state;\n", "l"),
 ("Q5 a second state of the top's group not remembered", CU,
  "                if top.state != Some(state) {\n                    top.state = None;\n                }\n",
  "", "l"),
 ("Q6 the top follows the last seen, not the highest", CU,
  "            Some(top) if state.txg < top.txg => {}\n",
  "            Some(top) if state.txg < top.txg => {\n                *top = Top {\n                    txg: state.txg,\n                    state: Some(state),\n                };\n            }\n", "l"),
 ("Q7 unverified uberblocks in the rings raise the top", CU,
  "                if uberblock.verified() && uberblock.counts() {\n                    self.saw(uberblock.state);\n",
  "                if uberblock.counts() {\n                    self.saw(uberblock.state);\n", "l"),
 ("Q8 REWIND_BY_PREVIOUS by hand", CU,
  "        } else {\n            self.follows_a_rewind(&state)\n        };\n",
  "        } else {\n            self.cuts\n                .newest\n                .is_some_and(|before| cut.group <= before.group)\n        };\n", "l"),
 # The live mark (B3).
 ("B3a the uploader takes no mark at the ask", UL,
  "                self.publishing().set_rewound(rewound);\n                let answer = self.publishing().ask(request);\n",
  "                let _ = rewound;\n                let answer = self.publishing().ask(request);\n", "ud"),
 ("B3b the handler sends no mark", HD,
  "                    rewound: now.rewound,\n", "                    rewound: 0,\n", "d"),
 ("B3d the mark given after the ask", UL,
  "                self.publishing().set_rewound(rewound);\n                let answer = self.publishing().ask(request);\n",
  "                let answer = self.publishing().ask(request);\n                self.publishing().set_rewound(rewound);\n", "ud"),
 # (a) The shared limit.
 ("Wa each wait has the whole limit", HD,
  "        let limit = self.limit.saturating_sub(self.began.elapsed());\n",
  "        let limit = self.limit;\n", "d"),
 ("Wb the limit counted from the handler's making", HD,
  "    Ok(Box::new(move |request: &Request, manifest: Vec<u8>| {\n        let waiter = Waiter {\n            runtime: &runtime,\n            limit,\n            began: std::time::Instant::now(),\n        };\n",
  "    let began = std::time::Instant::now();\n    Ok(Box::new(move |request: &Request, manifest: Vec<u8>| {\n        let waiter = Waiter {\n            runtime: &runtime,\n            limit,\n            began,\n        };\n", "d"),
 # Protocol v2.
 ("V1 the greeting back to version 1", CC,
  "pub const HELLO: &str = \"skyzfs-control 2\";\n", "pub const HELLO: &str = \"skyzfs-control 1\";\n", "c"),
 # The prior survivors and N2/N3/T1.
 ("L8 the row's read error taken as no row (N2)", LK,
  "    let row = refs.meta_get(vol, &snap_row_key(name)).await?;\n",
  "    let row = refs.meta_get(vol, &snap_row_key(name)).await.ok().flatten();\n", "u"),
 ("W1 the client parses group as number (T1)", CC,
  "                    number: number(&p, \"number\")?,\n                    group: number(&p, \"group\")?,\n",
  "                    number: number(&p, \"group\")?,\n                    group: number(&p, \"number\")?,\n", "c"),
 ("L4 another volume looked up (N3)", SE,
  "        vol: remote.id,\n", "        vol: skynbd_types::VolumeId(remote.id.0.increment().unwrap_or_default()),\n", "d"),
 ("R8a the daemon's start reads no mark (N3)", SE,
  "    let rewound = served.cuts().rewound;\n", "    let rewound = 0;\n", "d"),
 # Opens as itself, now that prewind no longer exercises it.
 ("C1 itself not required", PB,
  "            && cut.itself\n", "            && (cut.itself || true)\n", "u"),
]
only = sys.argv[1:]
res = []
for name, f, old, new, suites in P:
    if only and not any(name.startswith(o + " ") or name == o for o in only):
        continue
    path = os.path.join(W, f)
    if not os.path.exists(path):
        res.append((name, "NOT APPLIED (no file)"))
        print(f"{name}: NOT APPLIED (no file)", flush=True)
        continue
    with open(path) as h:
        text = h.read()
    n = text.count(old)
    if n != 1:
        res.append((name, f"NOT APPLIED (found {n})"))
        print(f"{name}: NOT APPLIED ({n})", flush=True)
        continue
    t = time.time()
    verdict = "SURVIVED"
    try:
        with open(path, "w") as h:
            h.write(text.replace(old, new))
        for s in suites:
            try:
                r = subprocess.run(SUITES[s], cwd=W, env=ENV, capture_output=True, text=True, timeout=1500)
            except subprocess.TimeoutExpired:
                verdict = f"KILLED ({s}: timeout, a hang)"
                break
            out = r.stdout + r.stderr
            if "error[E" in out or "could not compile" in out:
                verdict = "BUILD ERROR"
                print(out[-3000:])
                break
            if r.returncode != 0:
                failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
                verdict = f"KILLED ({s}): " + ", ".join(failed[:4]) + (f" and {len(failed) - 4} more" if len(failed) > 4 else "")
                break
    finally:
        with open(path, "w") as h:
            h.write(text)
    res.append((name, verdict))
    print(f"{name}: {verdict} ({time.time() - t:.0f}s)", flush=True)
    with open(OUT, "w") as h:
        json.dump(res, h, indent=1)
print("done")
