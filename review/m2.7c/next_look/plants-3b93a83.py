# Plants at 3b93a83 (PR 173, M2.7c, the next look), each alone in a
# scratch worktree of the head (wt173pl), run against the agent's lib
# tests (`cargo test -p skyzfs-agent --lib`), 600 s per plant, restored
# after each with `git checkout`. W.. the watch, V.. the verdict table,
# P.. the round and the codes, X.. the error paths and earlier
# survivors. SCRATCH names the scratch directory. Usage: plants.py [TAG ...]
import subprocess, sys, os, re
S = os.environ['SCRATCH']
WT = S + '/wt173pl'
L = 'crates/skyzfs-agent/src/engine/lost.rs'
P = 'crates/skyzfs-agent/src/program.rs'
E = 'crates/skyzfs-agent/src/engine.rs'
D = 'crates/skyzfs-agent/src/daemon.rs'
C = 'crates/skyzfs-agent/src/zfs/cli.rs'
env = dict(os.environ, CARGO_TARGET_DIR=S + '/t173p', CARGO_INCREMENTAL='0',
           CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0')

WATCH_SET = "        agent.watch_note(read.as_ref().ok().copied());\n"
UNREAD_ARM = "            Some(Step::Reopened {\n                unread: Some(_), ..\n            }) => Verdict::Retry,"
ROUND_FAILED = "            failed: !failed.is_empty() || lost_verdict == Some(Verdict::Retry),"
EXIT_LOST = "        if lost_verdict == Some(Verdict::Lost) {"
OUT_LINES = ("        if let Some((line, _)) = &lost {\n            let _ = out.write_all(line.as_bytes());\n        }\n"
             "        let _ = out.write_all(round_line(&round).as_bytes());\n")
RETRY_SAID = """        Verdict::Retry => {
            let _ = writeln!(
                err,
                "skyzfs-agent: round {n}: the volume is lost for this boot (note={} reason={} \\
                 held={}), and the pool was not shown suspended: the next round tries again",
                note_word(note),
                note.reason_word(),
                note.held_word()
            );
        }"""
FAILED_SAID = """        Verdict::Failed => {
            let _ = writeln!(
                err,
                "skyzfs-agent: round {n}: the volume failed in this boot (note={} reason={} \\
                 held={}), and the pool is not imported: nothing is suspended; run goes on",
                note_word(note),
                note.reason_word(),
                note.held_word()
            );
        }"""
LOOK_ERR = """                Err(e) => {
                    return Ok(Step::Reopened {
                        error,
                        state: None,
                        unread: Some(e.to_string()),
                        looked_ms,
                    })
                }"""
REOPEN_EXEC = "        let output = self.exec(\"zpool\", &args, &[], self.limits.reopen)?;\n        if output.code == Some(0) {"
STATE_ERR = "            Err(e) => return Err(ZfsError::Other(format!(\"{}: {e}\", path.display()))),"
LOSE_CALL = "                let step = match agent.lose() {"
SYNC_NOTED = "            if self.note_came() {\n                return behind(Stuck::Noted);\n            }\n"

PLANTS = [
 ('BASE', []),
 # ---- the watch
 ('W1 the note not read in the wait for the tick', [(P, "        while agent.clock().mono_ms() < due && !agent.note_came() {", "        while agent.clock().mono_ms() < due {")]),
 ('W2 the watch not asked in the settle', [(E, "            if !go() || self.note_came() {", "            if !go() {")]),
 ('W3 the watch not asked in the syncs', [(E, SYNC_NOTED, "")]),
 ('W3b the syncs stopped by news named Told', [(E, "                return behind(Stuck::Noted);", "                return behind(Stuck::Told);")]),
 ('W3c the watch asked before go in the syncs', [(E, "            if !go() {\n                return behind(Stuck::Told);\n            }\n" + SYNC_NOTED,
   SYNC_NOTED + "            if !go() {\n                return behind(Stuck::Told);\n            }\n")]),
 ('W4 NOTE_EVERY 5 s', [(L, "pub const NOTE_EVERY: Duration = Duration::from_secs(1);", "pub const NOTE_EVERY: Duration = Duration::from_secs(5);")]),
 ('W4b NOTE_EVERY 3 s', [(L, "pub const NOTE_EVERY: Duration = Duration::from_secs(1);", "pub const NOTE_EVERY: Duration = Duration::from_secs(3);")]),
 ('W5 a read at each ask', [(L, "pub const NOTE_EVERY: Duration = Duration::from_secs(1);", "pub const NOTE_EVERY: Duration = Duration::from_millis(0);")]),
 ('W6 news not kept', [(L, "            Some(watch) if watch.came => return true,", "            Some(watch) if watch.came && false => return true,")]),
 ('W7 seen ignored (any lost note is news)', [(L, "            watch.came = watch.seen != Some(now_failed);", "            let _ = now_failed;\n            watch.came = true;")]),
 ('W8 a note no client held is news', [(L, "        if let Ok(now_failed @ Failed::Lost(_)) = read {", "        if let Ok(now_failed @ (Failed::Lost(_) | Failed::Unheld(_))) = read {")]),
 ('W9 a failed read is news', [(L, "        if let Ok(now_failed @ Failed::Lost(_)) = read {", "        if read.is_err() {\n            watch.came = true;\n        }\n        if let Ok(now_failed @ Failed::Lost(_)) = read {")]),
 ('W10 the first read at once', [(L, "        let next_ms = self.clock.mono_ms().saturating_add(every_ms());", "        let next_ms = self.clock.mono_ms();")]),
 ('W11 news kept across watch_note', [(L, "            came: false,", "            came: self.watch.as_ref().is_some_and(|w| w.came),")]),
 ('W12 the watch set only at the first round', [(P, WATCH_SET, "        if n == 1 {\n            agent.watch_note(read.as_ref().ok().copied());\n        }\n")]),
 ('W13 seen none (the round read forgotten)', [(P, WATCH_SET, "        agent.watch_note(None);\n")]),
 ('W14 seen no note (the round own lost note is news)', [(P, WATCH_SET, "        agent.watch_note(Some(Failed::No));\n")]),
 ('W15 a news round not early: the wait goes on', [(P, "        while agent.clock().mono_ms() < due && !agent.note_came() {", "        while agent.clock().mono_ms() < due && (!agent.note_came() || true) {")]),
 # ---- the verdict table
 ('V1 not imported lost', [(L, "            Some(Step::NotImported) => Verdict::Failed,", "            Some(Step::NotImported) => Verdict::Lost,")]),
 ('V2 not imported retry', [(L, "            Some(Step::NotImported) => Verdict::Failed,", "            Some(Step::NotImported) => Verdict::Retry,")]),
 ('V3 suspended retry', [(L, "            Some(Step::Suspended) => Verdict::Lost,", "            Some(Step::Suspended) => Verdict::Retry,")]),
 ('V3b suspended failed', [(L, "            Some(Step::Suspended) => Verdict::Lost,", "            Some(Step::Suspended) => Verdict::Failed,")]),
 ('V4 unread lost', [(L, UNREAD_ARM, UNREAD_ARM.replace("Verdict::Retry", "Verdict::Lost"))]),
 ('V4b unread failed (the arm gone)', [(L, UNREAD_ARM, "")]),
 ('V5 reopened SUSPENDED retry', [(L, "            }) if state.suspended() => Verdict::Lost,", "            }) if state.suspended() => Verdict::Retry,")]),
 ('V5b reopened SUSPENDED failed', [(L, "            }) if state.suspended() => Verdict::Lost,", "            }) if state.suspended() => Verdict::Failed,")]),
 ('V6 reopened another state lost', [(L, "            Some(Step::Reopened { state: Some(_), .. }) => Verdict::Retry,", "            Some(Step::Reopened { state: Some(_), .. }) => Verdict::Lost,")]),
 ('V6b reopened another state failed', [(L, "            Some(Step::Reopened { state: Some(_), .. }) => Verdict::Retry,", "            Some(Step::Reopened { state: Some(_), .. }) => Verdict::Failed,")]),
 ('V7 reopened gone lost', [(L, "            Some(Step::Reopened { state: None, .. }) => Verdict::Failed,", "            Some(Step::Reopened { state: None, .. }) => Verdict::Lost,")]),
 ('V7b reopened gone retry', [(L, "            Some(Step::Reopened { state: None, .. }) => Verdict::Failed,", "            Some(Step::Reopened { state: None, .. }) => Verdict::Retry,")]),
 ('V8 step failed lost', [(L, "            None => Verdict::Retry,\n            Some(Step::NotImported)", "            None => Verdict::Lost,\n            Some(Step::NotImported)")]),
 ('V8b step failed failed', [(L, "            None => Verdict::Retry,\n            Some(Step::NotImported)", "            None => Verdict::Failed,\n            Some(Step::NotImported)")]),
 ('V9 word of retry', [(L, "            Verdict::Retry => \"retry\",", "            Verdict::Retry => \"failed\",")]),
 # ---- the round and the codes
 ('P1 a retry round not failed', [(P, ROUND_FAILED, "            failed: !failed.is_empty(),")]),
 ('P2 a failed-verdict round failed', [(P, ROUND_FAILED, "            failed: !failed.is_empty() || lost_verdict.is_some_and(|v| v != Verdict::Lost),")]),
 ('P2b a lost round failed', [(P, ROUND_FAILED, "            failed: !failed.is_empty() || lost_verdict.is_some_and(|v| v != Verdict::Failed),")]),
 ('P3 an end on retry', [(P, EXIT_LOST, "        if lost_verdict.is_some_and(|v| v != Verdict::Failed) {")]),
 ('P4 an end on failed', [(P, EXIT_LOST, "        if lost_verdict.is_some_and(|v| v != Verdict::Retry) {")]),
 ('P5 a lost round goes on to the engine', [(P, "        if lost.is_none() && !unread {", "        if !unread {")]),
 ('P5b a retry round goes on to the engine', [(P, "        if lost.is_none() && !unread {", "        if lost_verdict.is_none_or(|v| v == Verdict::Retry) && !unread {")]),
 ('P5c a failed-verdict round goes on to the engine', [(P, "        if lost.is_none() && !unread {", "        if lost_verdict.is_none_or(|v| v == Verdict::Failed) && !unread {")]),
 ('P6 an unread-note round goes on', [(P, "        if lost.is_none() && !unread {", "        if lost.is_none() {")]),
 ('P7 an unread-note round not failed', [(P, "        if let Err(e) = read {\n            failed.push(e);\n        }\n", "        drop(read);\n")]),
 ('P8 the lost line after the round line', [(P, OUT_LINES,
   "        let _ = out.write_all(round_line(&round).as_bytes());\n        if let Some((line, _)) = &lost {\n            let _ = out.write_all(line.as_bytes());\n        }\n")]),
 ('P9 the retry words left out', [(P, RETRY_SAID, "        Verdict::Retry => {}")]),
 ('P10 the failed words left out', [(P, FAILED_SAID, "        Verdict::Failed => {}")]),
 ('P11 the reopen error not said', [(P, "    if let Some(Step::Reopened {\n        error: Some(why), ..\n    }) = step\n", "    if let Some(Step::Reopened {\n        error: Some(why), ..\n    }) = step.filter(|_| false)\n")]),
 ('P12 the look error not said', [(P, "    if let Some(Step::Reopened {\n        unread: Some(why), ..\n    }) = step\n", "    if let Some(Step::Reopened {\n        unread: Some(why), ..\n    }) = step.filter(|_| false)\n")]),
 ('P13 LOST back at 9', [(P, "    pub const LOST: i32 = 11;", "    pub const LOST: i32 = 9;")]),
 ('P14 MOVED back at 10', [(P, "    pub const MOVED: i32 = 12;", "    pub const MOVED: i32 = 10;")]),
 ('P15 unread written as -', [(P, "            if unread.is_some() {\n                \"unread\".to_string()", "            if unread.is_some() && false {\n                \"unread\".to_string()")]),
 ('P16 a begin before the step (A-P5)', [(P, LOSE_CALL, "                let _ = agent.begin();\n" + LOSE_CALL)]),
 ('P17 a settle in the lost round', [(P, LOSE_CALL, "                let _ = agent.settle_while(Duration::from_secs(5), go);\n" + LOSE_CALL)]),
 ('P18 a restart in the lost round', [(P, LOSE_CALL, "                let _ = agent.restart();\n" + LOSE_CALL)]),
 ('P19 bound set by a Noted stop', [(P, "                        bound = why == Stuck::Bound;", "                        bound = why == Stuck::Bound || why == Stuck::Noted;")]),
 ('P20 synced zeroed by a Noted stop', [(P, "                        synced = k;\n                        bound = why == Stuck::Bound;", "                        synced = if why == Stuck::Noted { 0 } else { k };\n                        bound = why == Stuck::Bound;")]),
 ('P21 the step error ends nothing but is not said', [(P, "                    Err(e) => {\n                        failed.push(e);\n                        None\n                    }", "                    Err(_) => None,")]),
 # ---- the error paths and earlier survivors
 ('X1 a look error after the reopen is the step error (F3 back)', [(L, LOOK_ERR, "                Err(e) => return Err(e.into()),")]),
 ('X2 the reopen outcome dropped at a look error', [(L, LOOK_ERR, LOOK_ERR.replace("                        error,\n", "                        error: None,\n"))]),
 ('X3 the look error dropped (unread none)', [(L, LOOK_ERR, LOOK_ERR.replace("unread: Some(e.to_string())", "unread: { let _ = e; None }"))]),
 ('X4 a state error before the reopen read as not imported (R-L5)', [(L, "        match self.zfs.pool_state(&self.pool)? {", "        match self.zfs.pool_state(&self.pool).unwrap_or(None) {")]),
 ('X5 the kstat read error as not imported (A-C6b)', [(C, STATE_ERR, "            Err(_) => return Ok(None),")]),
 ('X6 the note read error as no note (R-D1)', [(D, "            Err(e) => return Err(e),", "            Err(_) => return Ok(None),")]),
 ('X7 the boot id read error as no note', [(D, "        let text = self.host.read(Path::new(BOOT_ID))?;", "        let Ok(text) = self.host.read(Path::new(BOOT_ID)) else {\n            return Ok(None);\n        };")]),
 ('X8 publish and settle take an unread note as none (R-P3)', [(P, "        Err(e) => Some(failed(&e, err)),", "        Err(_) => None,")]),
 ('X9 a reopen ended by a signal read as done (R-C2)', [(C, REOPEN_EXEC, "        let output = self.exec(\"zpool\", &args, &[], self.limits.reopen)?;\n        if output.code.unwrap_or(0) == 0 {")]),
 ('X10 a pool gone during the look not the end (R-L1)', [(L, "            let done = state.as_ref().is_none_or(PoolState::suspended);", "            let done = state.as_ref().is_some_and(PoolState::suspended);")]),
 ('X11 LOOK_MAX 60 s (R-L2)', [(L, "pub const LOOK_MAX: Duration = Duration::from_secs(10);", "pub const LOOK_MAX: Duration = Duration::from_secs(60);")]),
]

def run(cmd, timeout):
    try:
        p = subprocess.run(cmd, cwd=WT, env=env, capture_output=True, text=True, timeout=timeout)
        return p.returncode, p.stdout + p.stderr
    except subprocess.TimeoutExpired as e:
        return 'TIMEOUT', (e.stdout or b'').decode(errors='replace') if isinstance(e.stdout, bytes) else (e.stdout or '')

def main():
    tags = sys.argv[1:]
    plants = [p for p in PLANTS if not tags or p[0].split(' ')[0] in tags]
    for name, edits in plants:
        subprocess.run(['git', 'checkout', '-q', '--', '.'], cwd=WT, check=True)
        ok = True
        for path, old, new in edits:
            full = os.path.join(WT, path)
            text = open(full).read()
            n = text.count(old)
            if n != 1:
                print(f'{name}: PLANT DOES NOT APPLY ({path}: {n} matches)', flush=True)
                ok = False
                break
            open(full, 'w').write(text.replace(old, new))
        if not ok:
            continue
        rc, out = run(['cargo', 'test', '-p', 'skyzfs-agent', '--lib'], 600)
        if rc != 'TIMEOUT' and re.search(r'^error(\[E\d+\])?: ', out, re.M) and 'test result' not in out:
            first = [l for l in out.splitlines() if l.startswith('error')][:3]
            print(f'{name}: DOES NOT BUILD: {first}', flush=True)
            continue
        failed = sorted(set(re.findall(r'^test (\S+) \.\.\. FAILED', out, re.M)))
        passed = sum(int(x) for x in re.findall(r'test result: \w+\. (\d+) passed', out))
        if rc == 'TIMEOUT':
            verdict = 'TIMEOUT (600 s)'
        elif failed:
            verdict = f'KILLED by {len(failed)}: ' + ', '.join(f.split('::')[-1] for f in failed[:4]) + (' ...' if len(failed) > 4 else '')
        elif rc != 0:
            verdict = f'FAILED rc={rc} with no failing test named'
        else:
            verdict = f'SURVIVED ({passed} passed)'
        print(f'{name}: {verdict}', flush=True)
    subprocess.run(['git', 'checkout', '-q', '--', '.'], cwd=WT, check=True)

main()
