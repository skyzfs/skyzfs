# Plants at 28782cd, each alone against `cargo test -p skyzfs-agent --lib`,
# restored after each, 600 s per plant. Mode "author": the head's suite as
# it is. Mode "both": the head's suite with the review's scratch tests of
# 16f3b3b added (module review167 and RH; their two error-text asserts
# moved to the head's text), to compare the ported tests with the
# originals.
import subprocess, sys, os, re, json
S = os.environ['SCRATCH']
WT = S + '/wt/pr167p'
P = 'crates/skyzfs-agent/src/program.rs'
E = 'crates/skyzfs-agent/src/engine.rs'
env = dict(os.environ, CARGO_TARGET_DIR=S + '/t167p', CARGO_INCREMENTAL='0',
           CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0')

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
ENDS = "matches!(e, Error::Corrupt(_) | Error::Volume(_) | Error::Unmarked(_))"
PLANTS = [
 ('BASE', []),
 ('N1 the begin error restarts at the next round, after the settle (fix of 16f3b3b out)', [
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
   (P, "                Ok(()) => restart = false,\n                Err(e) => failed.push(e),",
       "                Ok(()) => restart = false,\n                Err(e) => {\n                    failed.push(e);\n                    restart = false;\n                }"),
 ]),
 ('N5 a settle error does not restart', [
   (P, "                    restart = restarts(&e);\n                    failed.push(e);", "                    failed.push(e);"),
 ]),
 ('N6 every begin error restarts at once, refusals too', [
   (P, "                    let reload = restarts(&e);", "                    let reload = true;"),
 ]),
 ('N7 a begin error that restarts skips the settle', [
   (P, "                            failed.push(again);\n                            restart = true;\n                        }\n                    }",
       "                            failed.push(again);\n                        }\n                        restart = true;\n                    }"),
 ]),
 ('P2 only Unmarked ends run', [(P, ENDS, "matches!(e, Error::Unmarked(_))")]),
 ('P2v Volume does not end run', [(P, ENDS, "matches!(e, Error::Corrupt(_) | Error::Unmarked(_))")]),
 ('P2c Corrupt does not end run', [(P, ENDS, "matches!(e, Error::Volume(_) | Error::Unmarked(_))")]),
 ('P8 a restart takes another volume\'s record as its own', [
   (E, "        Loaded::Present(record) if record.volume != volume => {\n            return Err(Error::Volume(record.volume))\n        }\n", ""),
 ]),
]

def scratch_tests():
    """Add the review's scratch tests of 16f3b3b to the worktree's test files."""
    patch = open(os.environ['REVIEW'] + '/scratch-tests.patch').read()
    eng, prog = patch.split('diff --git a/crates/skyzfs-agent/src/program/tests.rs', 1)
    add = [l[1:] for l in prog.splitlines() if l.startswith('+') and not l.startswith('+++')]
    # The first added line is the blank line before the module.
    body = '\n'.join(add) + '\n'
    body = body.replace("a manifest's file: ", "the record's directory: ")
    pt = os.path.join(WT, 'crates/skyzfs-agent/src/program/tests.rs')
    open(pt, 'a').write(body)
    r = subprocess.run(['git', 'apply', '-'], cwd=WT, input=eng, text=True, capture_output=True)
    if r.returncode != 0:
        print('engine hunk did not apply', r.stderr, flush=True); sys.exit(9)

mode = sys.argv[1]
only = sys.argv[2:]
if mode == 'both':
    scratch_tests()
res = {}
for tag, edits in PLANTS:
    if only and tag.split()[0] not in only:
        continue
    ok = True
    for (ff, o, n) in edits:
        p = os.path.join(WT, ff); s = open(p).read()
        if s.count(o) != 1:
            print(tag, 'NOMATCH', ff, s.count(o), flush=True); ok = False; break
        open(p, 'w').write(s.replace(o, n))
    if ok:
        try:
            r = subprocess.run(['cargo', 'test', '-p', 'skyzfs-agent', '--lib', '--', '--test-threads', '4'],
                               cwd=WT, env=env, capture_output=True, text=True, timeout=600)
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
        open(S + f'/r167/nl-{mode}-{tag.split()[0]}.log', 'w').write(out)
    subprocess.run(['git', 'checkout', '--', P, E], cwd=WT)
if mode == 'both':
    subprocess.run(['git', 'checkout', '--', 'crates/skyzfs-agent/src/program/tests.rs',
                    'crates/skyzfs-agent/src/engine/tests.rs'], cwd=WT)
json.dump(res, open(S + f'/r167/nl-plants-{mode}.json', 'w'), indent=1)
print('ALLDONE', flush=True)
