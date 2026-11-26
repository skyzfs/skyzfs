import subprocess, sys, os, re, json
S = os.environ['SCRATCH']
WT = S + '/wt/pr167p'
P = 'crates/skyzfs-agent/src/program.rs'
E = 'crates/skyzfs-agent/src/engine.rs'
env = dict(os.environ, CARGO_TARGET_DIR=S + '/t167p', CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0')
PLANTS = [
 ('BASE', None, None, None),
 ('P1 a failed restart falls through to begin and settle', P,
  "                Err(e) => failed.push(e),\n            }\n        }\n        if !restart {",
  "                Err(e) => {\n                    failed.push(e);\n                    restart = false;\n                }\n            }\n        }\n        if !restart {"),
 ('P2 only Unmarked ends run', P, "matches!(e, Error::Corrupt(_) | Error::Volume(_) | Error::Unmarked(_))", "matches!(e, Error::Unmarked(_))"),
 ('P3 forget drops the stops of publishes under way', E, "self.stopped.retain(|name| under_way(name));", "self.stopped.clear();"),
 ('P4 Forget keeps the memory of the publish', E, "                self.memory.remove(&name);\n                self.unlink(&name)?;", "                self.unlink(&name)?;"),
 ('P5 restart keeps the memory', E, "        self.memory.clear();\n        self.swept = None;", "        self.swept = None;"),
 ('P6 a stop is not told to the sink', E, "        self.told(name, Outcome::Stopped(why));", "        self.outcomes.push((name, Outcome::Stopped(why)));"),
 ('P7 restart keeps the sweep done', E, "        self.swept = None;\n        self.stopped.clear();", "        self.stopped.clear();"),
 ('A4 no restart after a failed round', P, "        restart = restart || failed.iter().any(restarts);\n", ""),
 ('A3 a refused begin keeps the round from settling', P,
  "            match agent.settle_while(limit, go) {\n                Ok(()) | Err(Error::Unsettled(_)) => {}\n                Err(e) => failed.push(e),\n            }",
  "            if begun.is_some() {\n            match agent.settle_while(limit, go) {\n                Ok(()) | Err(Error::Unsettled(_)) => {}\n                Err(e) => failed.push(e),\n            }\n            }"),
 ('S1 snap told after the Forget commit', E,
  "                self.told(name.clone(), Outcome::Published(commit));\n                self.commit(Effect::Forget, |r| r.under_way.retain(|p| p.name != name))?;",
  "                self.commit(Effect::Forget, |r| r.under_way.retain(|p| p.name != name))?;\n                self.told(name.clone(), Outcome::Published(commit));"),
 ('FIX1 restart at once after a begin error that restarts, then settle', P,
  "                Err(e) => failed.push(e),\n            }\n            let next = due.saturating_add(every_ms);",
  "                Err(e) => {\n                    if restarts(&e) {\n                        if let Err(again) = agent.restart() {\n                            failed.push(again);\n                        }\n                    }\n                    failed.push(e);\n                }\n            }\n            let next = due.saturating_add(every_ms);"),
 ('FIX2 one round of the engine before the deadline counts', E,
  "        let end = self.clock.mono_ms().saturating_add(ms(limit));\n        loop {",
  "        let end = self.clock.mono_ms().saturating_add(ms(limit));\n        let mut first = true;\n        loop {"),
]
FIX2B = ("            if self.clock.mono_ms() >= end || !go() {", "            if (!first && self.clock.mono_ms() >= end) || !go() {")
FIX2C = ("            let mut moved = Moved::No;\n            for name in names {", "            first = false;\n            let mut moved = Moved::No;\n            for name in names {")
only = sys.argv[1:]
res = {}
for tag, f, old, new in PLANTS:
    if only and not any(tag.startswith(o) for o in only):
        continue
    edits = [] if f is None else [(f, old, new)]
    if tag.startswith('FIX2'):
        edits += [(E, *FIX2B), (E, *FIX2C)]
    ok = True
    for (ff, o, n) in edits:
        p = os.path.join(WT, ff); s = open(p).read()
        if s.count(o) != 1:
            print(tag, 'NOMATCH', ff, s.count(o)); ok = False; break
        open(p, 'w').write(s.replace(o, n))
    if ok:
        try:
            r = subprocess.run(['cargo', 'test', '-p', 'skyzfs-agent', '--lib', '--', '--test-threads', '4'], cwd=WT, env=env, capture_output=True, text=True, timeout=600)
            out = r.stdout + r.stderr; rc = r.returncode
        except subprocess.TimeoutExpired as e:
            out = (e.stdout or b'').decode() if isinstance(e.stdout, bytes) else (e.stdout or ''); rc = 'TIMEOUT'
        fails = re.findall(r'^test (\S+) \.\.\. FAILED', out, re.M)
        summ = re.findall(r'^test result: .*$', out, re.M)
        if 'error[' in out and not summ:
            fails = ['COMPILE ERROR: ' + '\n'.join(l for l in out.splitlines() if 'error' in l)[:800]]
        res[tag] = {'rc': rc, 'fails': fails, 'summary': summ}
        print(tag, 'rc=', rc, summ, flush=True)
        for x in fails: print('   FAILED', x, flush=True)
        open(S + f'/r167/plant-{tag.split()[0]}.log', 'w').write(out)
    subprocess.run(['git', 'checkout', '--', P, E], cwd=WT)
json.dump(res, open(S + '/r167/plants.json', 'w'), indent=1)
