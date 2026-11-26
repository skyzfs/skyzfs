# Plants at bb03454 (PR 170, M2.7d), each alone against the agent crates'
# tests (`cargo test -p skyzfs-agent -p skyzfs-agent-cli --tests`: the
# library's unit tests, the binary's and its integration tests), restored
# after each, 600 s per plant. A1..A17 are the author's seventeen, as I
# read their one-line descriptions; R1.. are the review's own. A14b and
# A15b were added after the first run and run alone (`plants.py A14b
# A15b`); their lines are at the end of plants-author.out. The run with
# the review's tests was `plants.py scratch-tests.rs BASE A14b A15b R1 R3
# R5 R6 R7 R8 R9 R10 R11 R12 R16 R19 R20`. SCRATCH names the scratch
# directory, which holds the worktree pr170p at bb03454. Usage:
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
 # ---- the author's seventeen, re-made from their one-line descriptions
 ('A1 no syncs', [(E, ASSIST, "            let _ = value;\n        }\n    }")]),
 ('A2 one sync only', [(E, BOUND, "            if self.past_synced >= SYNC_PAST_MAX || synced >= 1 {")]),
 ('A3 a sync while the newest cut opens', [
   (E, "                Some((group, false)) => group,\n                Some((_, true)) | None => {",
       "                Some((group, opens)) if !opens || synced == 0 => group,\n                _ => {")]),
 ('A4 no bound', [(E, BOUND, "            if self.past_synced >= u32::MAX {")]),
 ('A5 the bound per call', [(E, BOUND, "            if synced >= SYNC_PAST_MAX {")]),
 ('A6 the count not reset by a cut that opens', [
   (E, RESET, "                Some((_, true)) | None => {\n                    let _ = 0;")]),
 ('A7 the count reset by a restart', [
   (E, "        self.assists = 0;\n        self.at.clear();\n        Ok(())",
       "        self.assists = 0;\n        self.past_synced = 0;\n        self.at.clear();\n        Ok(())")]),
 ('A8 no NoCut check', [(E, NOCUT, "            if false && count_before.is_some_and(|before| cuts.count <= before) {")]),
 ('A9 go not asked in the syncs', [(E, TOLD, "            if false && !go() {\n                return behind(Stuck::Told);")]),
 ('A10 the premise not checked', [(E, PREMISE, "            if false {\n                let props = self.zfs.props(&self.pool)?;")]),
 ('A11 a begin while behind', [(P, "        if !restart && clear {", "        if !restart {")]),
 ('A12 no sync before the begin', [(P, "            match agent.sync_past(go) {",
                                    "            match Ok::<Past, Error>(Past::Unseen) {")]),
 ('A13 synced left out of the line', [
   (P, "under_way={} synced={}\\n\",", "under_way={}\\n\","),
   (P, "        round.under_way,\n        round.synced\n", "        round.under_way\n")]),
 ('A14 go asked after the count and sweep (n1 undone)', [
   (E, GO_TOP, ""),
   (E, SWEEP, SWEEP + "            if !go() {\n                return self.unsettled();\n            }\n")]),
 ('A14b go asked where it was before the change (after the count, the sweep and the empty check)', [
   (E, GO_TOP, ""),
   (E, "            if passes > 0 && self.clock.mono_ms() >= end {",
       "            if (passes > 0 && self.clock.mono_ms() >= end) || !go() {")]),
 ('A15 N3a the advice dropped', [
   (C, "where a tag holds a newline, release that hold by hand\",",
       "where a tag holds a newline, release it\",")]),
 ('A15b N3a the advice dropped from the missing snapshot error', [
   (C, "names a snapshot that never was: {}; {ODD_HOLDS}\",", "names a snapshot that never was: {}\",")]),
 ('A16 A9e the fake filter takes skz- anywhere', [
   (F, ".filter(|t| t.starts_with(crate::names::PREFIX))", ".filter(|t| t.contains(crate::names::PREFIX))")]),
 ('A17 a round still behind marked failed', [
   (P, "                    synced = k;\n                    let _ = writeln!(",
       "                    synced = k;\n                    failed.push(Error::Full);\n                    let _ = writeln!(")]),
 # ---- the review's own
 ('R1 the count reset only by a cut that opens, not by no cut', [
   (E, RESET, "                Some((_, true)) | None => {\n                    if cuts.newest.is_some() { self.past_synced = 0; }")]),
 ('R2 no cut held taken as behind', [
   (E, "                Some((group, false)) => group,\n                Some((_, true)) | None => {",
       "                Some((group, false)) => group,\n                None if cuts.state.is_some() => 0,\n                Some((_, true)) | None => {")]),
 ('R3 the daemon unread: no begin', [(P, "                Ok(Past::Unseen) => clear = true,", "                Ok(Past::Unseen) => {}")]),
 ('R4 NoCut only where the count fell', [(E, NOCUT, "            if count_before.is_some_and(|before| cuts.count < before) {")]),
 ('R5 NoCut against the first count, not the last', [
   (E, "            count_before = Some(cuts.count);", "            count_before = count_before.or(Some(cuts.count));")]),
 ('R6 a failed sync not counted toward the bound', [
   (E, "            self.past_synced = self.past_synced.saturating_add(1);\n            self.zfs.assist(&self.pool, value)?;",
       "            self.zfs.assist(&self.pool, value)?;\n            self.past_synced = self.past_synced.saturating_add(1);")]),
 ('R7 the premise checked only while the bound count is 0 (once per process)', [
   (E, PREMISE, "            if synced == 0 && self.past_synced == 0 {\n                let props = self.zfs.props(&self.pool)?;")]),
 ('R8 the syncs error does not restart', [
   (P, SP_ARM, SP_ARM.replace("let reload = restarts(&e);", "let reload = false;"))]),
 ('R9 the syncs error still begins', [
   (P, SP_ARM, SP_ARM.replace("                Err(e) => {\n", "                Err(e) => {\n                    clear = true;\n", 1))]),
 ('R10 a failed restart after the syncs error still settles', [
   (P, SP_ARM, SP_ARM.replace("                            failed.push(again);\n                            restart = true;\n",
                              "                            failed.push(again);\n"))]),
 ('R11 NoCut resets the bound count', [
   (E, "                return behind(Stuck::NoCut);",
       "                self.past_synced = 0;\n                return Ok(Past::Behind { synced, group, why: Stuck::NoCut });")]),
 ('R12 the syncs error restarts even on a refusal', [
   (P, SP_ARM, SP_ARM.replace("let reload = restarts(&e);", "let reload = true;"))]),
 ('R13 synced of a round left at 0 where behind', [
   (P, "                    synced = k;\n                    let _ = writeln!(", "                    let _ = k;\n                    let _ = writeln!(")]),
 ('R14 the stderr line of a round behind not printed', [
   (P, "                    let _ = writeln!(\n                        err,\n                        \"skyzfs-agent: round {n}: no publish begun",
       "                    let _ = (n, group, why);\n                    let _ = writeln!(\n                        std::io::sink(),\n                        \"skyzfs-agent: round {n}: no publish begun")]),
 ('R15 go asked only before the first pass of a settle', [
   (E, GO_TOP, "            if passes == 0 && !go() {\n                return self.unsettled();\n            }\n")]),
 ('R16 the bound checked after go (order)', [
   (E, BOUND + "\n                return behind(Stuck::Bound);\n            }\n" + TOLD + "\n            }",
       TOLD + "\n            }\n" + BOUND + "\n                return behind(Stuck::Bound);\n            }")]),
 ('R17 the sync value not kept for the planner', [
   (E, "            self.assists = value;\n            count_before = Some(cuts.count);", "            count_before = Some(cuts.count);")]),
 ('R19 the unsettled names dropped at an end (Ok always)', [
   (E, "            if !go() {\n                return self.unsettled();", "            if !go() {\n                return Ok(());")]),
 ('R20 a round whose syncs did not clear settles nothing', [
   (P, "        if !restart {\n            let next = due.saturating_add(every_ms);",
       "        if !restart && clear {\n            let next = due.saturating_add(every_ms);")]),
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
        open(R + f'/plant-{mode}-{tag.split()[0]}.log', 'w').write(out)
    subprocess.run(['git', 'checkout', '--', P, E, F, C], cwd=WT)
if mode == 'both':
    subprocess.run(['git', 'checkout', '--', 'crates/skyzfs-agent/src/program/tests.rs',
                    'crates/skyzfs-agent/src/engine/tests.rs'], cwd=WT)
json.dump(res, open(R + f'/plants-{mode}.json', 'w'), indent=1)
print('ALLDONE', flush=True)
