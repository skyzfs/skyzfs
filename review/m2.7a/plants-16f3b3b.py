import subprocess, sys, os, re, json
S = os.environ['SCRATCH']
WT = S + '/wt/pr167p'
P = 'crates/skyzfs-agent/src/program.rs'
E = 'crates/skyzfs-agent/src/engine.rs'
env = dict(os.environ, CARGO_TARGET_DIR=S + '/t167p', CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0')

BEGIN_ARM = """                Err(e) => {
                    // A begin that may have left the engine unlike what it
                    // saved (a save whose rename landed) restarts at once,
                    // before the engine settles anything: a save of the
                    // publishes under way from its own copy would write the
                    // new intent out of the record (P2 of Codex on #167).
                    // A refusal (`Full`, `Premise`) changed nothing.
                    let reload = restarts(&e);
                    failed.push(e);
                    if reload {
                        if let Err(again) = agent.restart() {
                            failed.push(again);
                            restart = true;
                        }
                    }
                }"""
SETTLE_ARM = """                Err(e) => {
                    restart = restarts(&e);
                    failed.push(e);
                }"""
PLANTS = [
 ('BASE', []),
 # The fix planted out: the restart after a begin error left to the next
 # round, after the settle (the code at 99e00ae).
 ('N1 the begin error restarts at the next round, after the settle', [
   (P, BEGIN_ARM, "                Err(e) => failed.push(e),"),
   (P, SETTLE_ARM, "                Err(e) => failed.push(e),"),
   (P, "        agent.forget();\n        tick =", "        restart = restart || failed.iter().any(restarts);\n        agent.forget();\n        tick ="),
 ]),
 ('N2 the limit asked before the first pass', [
   (E, "if (passes > 0 && self.clock.mono_ms() >= end) || !go() {", "if self.clock.mono_ms() >= end || !go() {"),
 ]),
 ('N3 a failed restart after a begin error lets the round settle', [
   (P, "                            failed.push(again);\n                            restart = true;\n", "                            failed.push(again);\n"),
 ]),
 ('N4 a failed restart at the round top falls through', [
   (P, "                Ok(()) => restart = false,\n                Err(e) => failed.push(e),", "                Ok(()) => restart = false,\n                Err(e) => {\n                    failed.push(e);\n                    restart = false;\n                }"),
 ]),
 ('N5 a settle error does not restart', [
   (P, "                    restart = restarts(&e);\n                    failed.push(e);", "                    failed.push(e);"),
 ]),
 ('N6 every begin error restarts at once, refusals too', [
   (P, "                    let reload = restarts(&e);", "                    let reload = true;"),
 ]),
 ('N7 a begin error that restarts skips the settle', [
   (P, "                            failed.push(again);\n                            restart = true;\n                        }\n                    }", "                            failed.push(again);\n                        }\n                        restart = true;\n                    }"),
 ]),
 ('P2 only Unmarked ends run', [(P, "matches!(e, Error::Corrupt(_) | Error::Volume(_) | Error::Unmarked(_))", "matches!(e, Error::Unmarked(_))")]),
 ('P3 forget drops the stops of publishes under way', [(E, "self.stopped.retain(|name| under_way(name));", "self.stopped.clear();")]),
 ('P4 Forget keeps the memory of the publish', [(E, "                self.memory.remove(&name);\n                self.unlink(&name)?;", "                self.unlink(&name)?;")]),
 ('P5 restart keeps the memory', [(E, "        self.memory.clear();\n        self.swept = None;", "        self.swept = None;")]),
 ('P7 restart keeps the sweep done', [(E, "        self.swept = None;\n        self.stopped.clear();", "        self.stopped.clear();")]),
]
only = sys.argv[1:]
res = {}
for tag, edits in PLANTS:
    if only and not any(tag.split()[0] == o for o in only):
        continue
    ok = True
    for (ff, o, n) in edits:
        p = os.path.join(WT, ff); s = open(p).read()
        if s.count(o) != 1:
            print(tag, 'NOMATCH', ff, s.count(o), flush=True); ok = False; break
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
        open(S + f'/r167/plant2-{tag.split()[0]}.log', 'w').write(out)
    subprocess.run(['git', 'checkout', '--', P, E], cwd=WT)
json.dump(res, open(S + '/r167/plants2.json', 'w'), indent=1)
print('ALLDONE', flush=True)
