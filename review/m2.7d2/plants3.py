# Plants at edf9a17 (PR 172, M2.7d-2), each alone against the agent
# crates' tests (`cargo test -p skyzfs-agent -p skyzfs-agent-cli --tests`:
# the library's unit tests, the binary's and its integration tests),
# restored after each, 600 s per plant.
#
# R*: the review of #170's plants that survived 02af425's suite (R6 is
# the code since 02af425 and is left out). L2a, L2b, B1, B2: the author's
# four new ones, as the description names them; with R1, R3, R5, R7-R12,
# R19 and R20 they are the author's fifteen. F*: this review's, on the
# new logic.
#
# SCRATCH names the scratch directory, which holds the worktree pr172p at
# edf9a17. Usage: plants3.py [extra-tests-file] [TAG ...]. With an
# extra-tests file (scratch-tests.rs) its two parts are appended to the
# engine's and the program's tests first; tests named in KNOWN fail at
# the head by design (they show this review's findings) and do not count
# as a kill.
import subprocess, sys, os, re, json
S = os.environ['SCRATCH']
WT = S + '/pr172p'
R = S + '/r172/out'
P = 'crates/skyzfs-agent/src/program.rs'
E = 'crates/skyzfs-agent/src/engine.rs'
F = 'crates/skyzfs-agent/src/engine/fake.rs'
env = dict(os.environ, CARGO_TARGET_DIR=S + '/t172p', CARGO_INCREMENTAL='0',
           CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0')

NOCUT = "            if count_before.is_some_and(|before| cuts.count <= before) {\n                return behind(Stuck::NoCut);\n            }\n"
BOUND = "            if self.past_synced >= SYNC_PAST_MAX {\n                return behind(Stuck::Bound);\n            }\n"
TOLD = "            if !go() {\n                return behind(Stuck::Told);\n            }\n"
PREMISE = "            if synced == 0 {\n                let props = self.zfs.props(&self.pool)?;"
RESET = "                Some((_, true)) | None => {\n                    self.past_synced = 0;"
LASTS = "            synced = synced.saturating_add(1);\n            self.last_synced = synced;\n"
SP_ARM = """                Err(e) => {
                    synced = agent.last_synced();
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
BOUNDSET = "                    bound = why == Stuck::Bound;\n"
PLANTS = [
 ('BASE', []),
 # ---- the review of #170's survivors
 ('R1 the count reset only by a cut that opens, not by no cut', [
   (E, RESET, "                Some((_, true)) | None => {\n                    if cuts.newest.is_some() { self.past_synced = 0; }")]),
 ('R3 the daemon unread: no begin', [(P, "                Ok(Past::Unseen) => clear = true,", "                Ok(Past::Unseen) => {}")]),
 ('R5 NoCut against the first count, not the last', [
   (E, "            count_before = Some(cuts.count);", "            count_before = count_before.or(Some(cuts.count));")]),
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
 ('R16 the bound checked after go (order)', [(E, BOUND + TOLD, TOLD + BOUND)]),
 ('R19 the unsettled names dropped at an end (Ok always)', [
   (E, "            if !go() {\n                return self.unsettled();", "            if !go() {\n                return Ok(());")]),
 ('R20 a round whose syncs did not clear settles nothing', [
   (P, "        if !restart {\n            let next = due.saturating_add(every_ms);",
       "        if !restart && clear {\n            let next = due.saturating_add(every_ms);")]),
 # ---- the author's four new ones
 ('L2a the count lost on the error', [(P, "                    synced = agent.last_synced();\n", "")]),
 ('L2b last_synced not reset at a call\'s start', [(E, "        self.last_synced = 0;\n", "")]),
 ('B1 bound never set', [(P, BOUNDSET, "")]),
 ('B2 bound set for every cause a round is behind', [(P, BOUNDSET, "                    bound = true;\n")]),
 # ---- this review's, on the new logic
 ('F1 bound also for an end (Told)', [(P, BOUNDSET, "                    bound = why != Stuck::NoCut;\n")]),
 ('F2 bound=1 on an error of the syncs', [(P, "                    synced = agent.last_synced();\n",
                                           "                    synced = agent.last_synced();\n                    bound = true;\n")]),
 ('F3 last_synced counts the sync that failed', [
   (E, "            self.zfs.assist(&self.pool, value)?;\n" + LASTS,
       "            self.last_synced = synced.saturating_add(1);\n            self.zfs.assist(&self.pool, value)?;\n            synced = synced.saturating_add(1);\n")]),
 ('F4 last_synced reset where nothing is behind, not at the call\'s start', [
   (E, "        self.last_synced = 0;\n", ""),
   (E, RESET, RESET + "\n                    self.last_synced = 0;")]),
 ('F5 bound kept across rounds (declared outside the round loop)', [
   (P, "    let mut restart = false;\n    for n in 1u64.. {", "    let mut restart = false;\n    let mut bound = false;\n    for n in 1u64.. {"),
   (P, "        let mut synced = 0;\n        let mut bound = false;\n", "        let mut synced = 0;\n")]),
 ('F6 bound before synced in the line', [
   (P, "under_way={} synced={} \\\n         bound={}\\n\",", "under_way={} bound={} \\\n         synced={}\\n\","),
   (P, "        round.synced,\n        u8::from(round.bound)\n", "        u8::from(round.bound),\n        round.synced\n")]),
 ('F7 last_synced lags one sync', [(E, LASTS, "            self.last_synced = synced;\n            synced = synced.saturating_add(1);\n")]),
 ('F8 NoCut judged after the bound (order)', [(E, NOCUT + BOUND, BOUND + NOCUT)]),
 ('F9 last_synced cleared by a restart, read after it', [
   (E, "        self.assists = 0;\n        self.at.clear();\n        Ok(())",
       "        self.assists = 0;\n        self.last_synced = 0;\n        self.at.clear();\n        Ok(())"),
   (P, "                    synced = agent.last_synced();\n", ""),
   (P, "                    let reload = restarts(&e);\n                    failed.push(e);\n                    if reload {\n                        if let Err(again) = agent.restart() {\n                            failed.push(again);\n                            restart = true;\n                        }\n                    }\n                }\n            }\n        }\n        if !restart && clear {",
       "                    let reload = restarts(&e);\n                    failed.push(e);\n                    if reload {\n                        if let Err(again) = agent.restart() {\n                            failed.push(again);\n                            restart = true;\n                        }\n                    }\n                    synced = agent.last_synced();\n                }\n            }\n        }\n        if !restart && clear {")]),
 ('F10 bound=1 only in a round that synced nothing', [(P, BOUNDSET, "                    bound = why == Stuck::Bound && k == 0;\n")]),
]

KNOWN = ('program::tests::review172::s1_', 'program::tests::review172::s1b_')

def extra(path):
    s = open(path).read()
    eng, prog = s.split('//// PROGRAM TESTS', 1)
    eng = eng.split('//// ENGINE TESTS', 1)[1]
    open(os.path.join(WT, 'crates/skyzfs-agent/src/engine/tests.rs'), 'a').write(eng)
    open(os.path.join(WT, 'crates/skyzfs-agent/src/program/tests.rs'), 'a').write(prog)

args = sys.argv[1:]
mode = 'head'
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
            r = subprocess.run(['cargo', 'test', '-p', 'skyzfs-agent', '-p', 'skyzfs-agent-cli', '--tests',
                                '--no-fail-fast'],
                               cwd=WT, env=env, capture_output=True, text=True, timeout=600)
            out = r.stdout + r.stderr; rc = r.returncode
        except subprocess.TimeoutExpired as e:
            out = (e.stdout or b'').decode() if isinstance(e.stdout, bytes) else (e.stdout or ''); rc = 'TIMEOUT'
        fails = re.findall(r'^test (\S+) \.\.\. FAILED', out, re.M)
        summ = re.findall(r'^test result: .*$', out, re.M)
        if 'error[' in out and not summ:
            fails = ['COMPILE ERROR: ' + '\n'.join(l for l in out.splitlines() if 'error' in l)[:800]]
        real = [x for x in fails if not x.startswith(KNOWN)]
        res[tag] = {'rc': rc, 'fails': fails, 'summary': summ}
        if rc == 'TIMEOUT':
            verdict = 'KILLED (timeout)'
        elif mode == 'both':
            verdict = 'KILLED' if real else 'SURVIVES'
        else:
            verdict = 'KILLED' if fails or rc != 0 else 'SURVIVES'
        print(tag, '->', verdict, 'rc=', rc, ' | '.join(summ), flush=True)
        for x in fails: print('   FAILED', x, flush=True)
        open(R + f'/plant-{mode}-{tag.split()[0]}.log', 'w').write(out)
    subprocess.run(['git', 'checkout', '--', P, E, F], cwd=WT)
if mode == 'both':
    subprocess.run(['git', 'checkout', '--', 'crates/skyzfs-agent/src/program/tests.rs',
                    'crates/skyzfs-agent/src/engine/tests.rs'], cwd=WT)
json.dump(res, open(R + f'/plants3-{mode}.json', 'w'), indent=1)
print('ALLDONE', flush=True)
