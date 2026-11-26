# Plants at 02af425 (PR 170, M2.7d, the second head), each alone against the agent crates'
# tests (`cargo test -p skyzfs-agent -p skyzfs-agent-cli --tests`: the
# library's unit tests, the binary's and its integration tests), restored
# after each, 600 s per plant. A1..A17 are the author's seventeen, as I
# read their one-line descriptions; R1.. are the review's own. Usage:
# plants.py [extra-tests-file] [TAG ...]
import subprocess, sys, os, re, json
S = os.environ['SCRATCH']
WT = S + '/pr170p'
R = S + '/r170'
P = 'crates/skyzfs-agent/src/program.rs'
E = 'crates/skyzfs-agent/src/engine.rs'
F = 'crates/skyzfs-agent/src/engine/fake.rs'
C = 'crates/skyzfs-agent/src/zfs/cli.rs'
env = dict(os.environ, CARGO_TARGET_DIR=S + '/t170p', CARGO_INCREMENTAL='0',
           CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0')

ASSIST = "            self.zfs.assist(&self.pool, value)?;\n        }\n    }"
NOCUT = "            if count_before.is_some_and(|before| cuts.count <= before) {"
BOUND = "            if self.past_synced >= SYNC_PAST_MAX {"
TOLD = "            if !go() {\n                return behind(Stuck::Told);"
PREMISE = "            if synced == 0 {\n                let props = self.zfs.props(&self.pool)?;"
RESET = "                Some((_, true)) | None => {\n                    self.past_synced = 0;"
GO_TOP = """            if !go() {
                return self.unsettled();
            }
"""
SWEEP = "            self.sweep()?;\n            let names = self.unstopped();\n"
SP_ARM = """                Err(e) => {
                    // As a begin's error ([`restarts`]); the syncs change
                    // nothing that the engine saves, so a restart only
                    // costs a reading of the record.
                    let reload = restarts(&e);
                    failed.push(e);
                    if reload {
                        if let Err(again) = agent.restart() {
                            failed.push(again);
                            restart = true;
                        }
                    }
                }"""
PLANTS = [
 ('BASE', []),
 # The author's two plants of 02af425, as its tests' docs name them.
 ('N1 the count raised before the sync returns', [
   (E, """            self.zfs.assist(&self.pool, value)?;
            synced = synced.saturating_add(1);
            self.past_synced = self.past_synced.saturating_add(1);""",
       """            synced = synced.saturating_add(1);
            self.past_synced = self.past_synced.saturating_add(1);
            self.zfs.assist(&self.pool, value)?;""")]),
 ('N2 no go asked between the syncs and the begin', [
   (P, "                    clear = k == 0 || go();", "                    clear = true;")]),
 # The review's.
 ('N3 go asked before every begin, a round that synced nothing too', [
   (P, "                    clear = k == 0 || go();", "                    clear = go();")]),
 ('N4 only the bound count raised after the sync returns', [
   (E, """            self.zfs.assist(&self.pool, value)?;
            synced = synced.saturating_add(1);
            self.past_synced = self.past_synced.saturating_add(1);""",
       """            self.past_synced = self.past_synced.saturating_add(1);
            self.zfs.assist(&self.pool, value)?;
            synced = synced.saturating_add(1);""")]),
]

def extra(path):
    s = open(path).read()
    for target, marker in (('engine', '//// ENGINE TESTS'), ('program', '//// PROGRAM TESTS')):
        pass
    eng, prog = s.split('//// PROGRAM TESTS', 1)
    eng = eng.split('//// ENGINE TESTS', 1)[1]
    open(os.path.join(WT, 'crates/skyzfs-agent/src/engine/tests.rs'), 'a').write(eng)
    open(os.path.join(WT, 'crates/skyzfs-agent/src/program/tests.rs'), 'a').write(prog)

args = sys.argv[1:]
mode = 'author'
if args and args[0].endswith('.rs'):
    extra(args[0]); mode = 'both'; args = args[1:]
only = args
res = {}
for tag, edits in PLANTS:
    if only and tag.split()[0] not in only:
        continue
    ok = True
    for (ff, o, n) in edits:
        p = os.path.join(WT, ff); s = open(p).read()
        if s.count(o) != 1:
            print(tag, 'NOMATCH', ff, s.count(o), repr(o[:60]), flush=True); ok = False; break
        open(p, 'w').write(s.replace(o, n))
    if ok:
        try:
            r = subprocess.run(['cargo', 'test', '-p', 'skyzfs-agent', '-p', 'skyzfs-agent-cli', '--tests'],
                               cwd=WT, env=env, capture_output=True, text=True, timeout=600)
            out = r.stdout + r.stderr; rc = r.returncode
        except subprocess.TimeoutExpired as e:
            out = (e.stdout or b'').decode() if isinstance(e.stdout, bytes) else (e.stdout or ''); rc = 'TIMEOUT'
        fails = re.findall(r'^test (\S+) \.\.\. FAILED', out, re.M)
        summ = re.findall(r'^test result: .*$', out, re.M)
        if 'error[' in out and not summ:
            fails = ['COMPILE ERROR: ' + '\n'.join(l for l in out.splitlines() if 'error' in l)[:800]]
        res[tag] = {'rc': rc, 'fails': fails, 'summary': summ}
        # In "both" mode rp1 and rp2 fail at the head by design (they show
        # findings L1 and L2); they do not count as a kill.
        known = ('program::tests::review170::rp1_', 'program::tests::review170::rp2_')
        if mode == 'both':
            fails = [x for x in fails if not x.startswith(known)]
            verdict = 'KILLED' if fails or 'COMPILE' in ''.join(fails) else 'SURVIVES'
        else:
            verdict = 'KILLED' if fails or rc not in (0,) else 'SURVIVES'
        print(tag, '->', verdict, 'rc=', rc, ' | '.join(summ), flush=True)
        for x in fails: print('   FAILED', x, flush=True)
        open(R + f'/plant2-{mode}-{tag.split()[0]}.log', 'w').write(out)
    subprocess.run(['git', 'checkout', '--', P, E, F, C], cwd=WT)
if mode == 'both':
    subprocess.run(['git', 'checkout', '--', 'crates/skyzfs-agent/src/program/tests.rs',
                    'crates/skyzfs-agent/src/engine/tests.rs'], cwd=WT)
json.dump(res, open(R + f'/plants2-{mode}.json', 'w'), indent=1)
print('ALLDONE', flush=True)
