# Plants at e3fe645 (PR 173, M2.7c, the next look, second head), each alone in a
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
 ('E1 the extra ask before the begin removed (P1)', [('crates/skyzfs-agent/src/program.rs', '            if !restart && clear && !agent.note_came() {', '            if !restart && clear {')]),
 ('E2 the unheld gate removed (P2)', [('crates/skyzfs-agent/src/program.rs', '        if lost.is_none() && !unread && !unheld {', '        if lost.is_none() && !unread {')]),
 ('E2b the unheld gate only before the begin', [('crates/skyzfs-agent/src/program.rs', '        if lost.is_none() && !unread && !unheld {', '        if lost.is_none() && !unread {'), ('crates/skyzfs-agent/src/program.rs', '            if !restart && clear && !agent.note_came() {', '            if !restart && clear && !agent.note_came() && !unheld {')]),
 ('E2c an unheld round failed', [('crates/skyzfs-agent/src/program.rs', '        let unheld = matches!(read, Ok(Failed::Unheld(_)));\n', '        let unheld = matches!(read, Ok(Failed::Unheld(_)));\n        if unheld {\n            failed.push(Error::Daemon("unheld".into()));\n        }\n')]),
 ('P5 a lost round goes on to the engine', [('crates/skyzfs-agent/src/program.rs', '        if lost.is_none() && !unread && !unheld {', '        if !unread && !unheld {')]),
 ('P5b a retry round goes on to the engine', [('crates/skyzfs-agent/src/program.rs', '        if lost.is_none() && !unread && !unheld {', '        if lost_verdict.is_none_or(|v| v == Verdict::Retry) && !unread && !unheld {')]),
 ('P5c a failed-verdict round goes on to the engine', [('crates/skyzfs-agent/src/program.rs', '        if lost.is_none() && !unread && !unheld {', '        if lost_verdict.is_none_or(|v| v == Verdict::Failed) && !unread && !unheld {')]),
 ('P6 an unread-note round goes on', [('crates/skyzfs-agent/src/program.rs', '        if lost.is_none() && !unread && !unheld {', '        if lost.is_none() && !unheld {')]),
 ('W12 the watch set only at the first round', [('crates/skyzfs-agent/src/program.rs', '        agent.watch_note(read.as_ref().ok().copied());\n', '        if n == 1 {\n            agent.watch_note(read.as_ref().ok().copied());\n        }\n')]),
 ('P7 an unread-note round not failed', [('crates/skyzfs-agent/src/program.rs', '        if let Err(e) = read {\n            failed.push(e);\n        }\n', '        drop(read);\n')]),
 ('P8 the lost line after the round line', [('crates/skyzfs-agent/src/program.rs', '        if let Some((line, _)) = &lost {\n            let _ = out.write_all(line.as_bytes());\n        }\n        let _ = out.write_all(round_line(&round).as_bytes());\n', '        let _ = out.write_all(round_line(&round).as_bytes());\n        if let Some((line, _)) = &lost {\n            let _ = out.write_all(line.as_bytes());\n        }\n')]),
 ('P9 the retry words left out', [('crates/skyzfs-agent/src/program.rs', '        Verdict::Retry => {\n            let _ = writeln!(\n                err,\n                "skyzfs-agent: round {n}: the volume is lost for this boot (note={} reason={} \\\n                 held={}), and the pool was not shown suspended: the next round tries again",\n                note_word(note),\n                note.reason_word(),\n                note.held_word()\n            );\n        }', '        Verdict::Retry => {}')]),
 ('P10 the failed words left out', [('crates/skyzfs-agent/src/program.rs', '        Verdict::Failed => {\n            let _ = writeln!(\n                err,\n                "skyzfs-agent: round {n}: the volume failed in this boot (note={} reason={} \\\n                 held={}), and the pool is not imported: nothing is suspended; run goes on",\n                note_word(note),\n                note.reason_word(),\n                note.held_word()\n            );\n        }', '        Verdict::Failed => {}')]),
 ('P11 the reopen error not said', [('crates/skyzfs-agent/src/program.rs', '    if let Some(Step::Reopened {\n        error: Some(why), ..\n    }) = step\n', '    if let Some(Step::Reopened {\n        error: Some(why), ..\n    }) = step.filter(|_| false)\n')]),
 ('P12 the look error not said', [('crates/skyzfs-agent/src/program.rs', '    if let Some(Step::Reopened {\n        unread: Some(why), ..\n    }) = step\n', '    if let Some(Step::Reopened {\n        unread: Some(why), ..\n    }) = step.filter(|_| false)\n')]),
 ('P13 LOST back at 9', [('crates/skyzfs-agent/src/program.rs', '    pub const LOST: i32 = 11;', '    pub const LOST: i32 = 9;')]),
 ('P14 MOVED back at 10', [('crates/skyzfs-agent/src/program.rs', '    pub const MOVED: i32 = 12;', '    pub const MOVED: i32 = 10;')]),
 ('P15 unread written as -', [('crates/skyzfs-agent/src/program.rs', '            if unread.is_some() {\n                "unread".to_string()', '            if unread.is_some() && false {\n                "unread".to_string()')]),
 ('P16 a begin before the step (A-P5)', [('crates/skyzfs-agent/src/program.rs', '                let step = match agent.lose() {', '                let _ = agent.begin();\n                let step = match agent.lose() {')]),
 ('P17 a settle in the lost round', [('crates/skyzfs-agent/src/program.rs', '                let step = match agent.lose() {', '                let _ = agent.settle_while(Duration::from_secs(5), go);\n                let step = match agent.lose() {')]),
 ('P18 a restart in the lost round', [('crates/skyzfs-agent/src/program.rs', '                let step = match agent.lose() {', '                let _ = agent.restart();\n                let step = match agent.lose() {')]),
 ('P19 bound set by a Noted stop', [('crates/skyzfs-agent/src/program.rs', '                        bound = why == Stuck::Bound;', '                        bound = why == Stuck::Bound || why == Stuck::Noted;')]),
 ('P20 synced zeroed by a Noted stop', [('crates/skyzfs-agent/src/program.rs', '                        synced = k;\n                        bound = why == Stuck::Bound;', '                        synced = if why == Stuck::Noted { 0 } else { k };\n                        bound = why == Stuck::Bound;')]),
 ('P21 the step error ends nothing but is not said', [('crates/skyzfs-agent/src/program.rs', '                    Err(e) => {\n                        failed.push(e);\n                        None\n                    }', '                    Err(_) => None,')]),
 ('X1 a look error after the reopen is the step error (F3 back)', [('crates/skyzfs-agent/src/engine/lost.rs', '                Err(e) => {\n                    return Ok(Step::Reopened {\n                        error,\n                        state: None,\n                        unread: Some(e.to_string()),\n                        looked_ms,\n                    })\n                }', '                Err(e) => return Err(e.into()),')]),
 ('X2 the reopen outcome dropped at a look error', [('crates/skyzfs-agent/src/engine/lost.rs', '                Err(e) => {\n                    return Ok(Step::Reopened {\n                        error,\n                        state: None,\n                        unread: Some(e.to_string()),\n                        looked_ms,\n                    })\n                }', '                Err(e) => {\n                    return Ok(Step::Reopened {\n                        error: None,\n                        state: None,\n                        unread: Some(e.to_string()),\n                        looked_ms,\n                    })\n                }')]),
 ('X3 the look error dropped (unread none)', [('crates/skyzfs-agent/src/engine/lost.rs', '                Err(e) => {\n                    return Ok(Step::Reopened {\n                        error,\n                        state: None,\n                        unread: Some(e.to_string()),\n                        looked_ms,\n                    })\n                }', '                Err(e) => {\n                    return Ok(Step::Reopened {\n                        error,\n                        state: None,\n                        unread: { let _ = e; None },\n                        looked_ms,\n                    })\n                }')]),
 ('X4 a state error before the reopen read as not imported (R-L5)', [('crates/skyzfs-agent/src/engine/lost.rs', '        match self.zfs.pool_state(&self.pool)? {', '        match self.zfs.pool_state(&self.pool).unwrap_or(None) {')]),
 ('X5 the kstat read error as not imported (A-C6b)', [('crates/skyzfs-agent/src/zfs/cli.rs', '            Err(e) => return Err(ZfsError::Other(format!("{}: {e}", path.display()))),', '            Err(_) => return Ok(None),')]),
 ('X6 the note read error as no note (R-D1)', [('crates/skyzfs-agent/src/daemon.rs', '            Err(e) => return Err(e),', '            Err(_) => return Ok(None),')]),
 ('X7 the boot id read error as no note', [('crates/skyzfs-agent/src/daemon.rs', '        let text = self.host.read(Path::new(BOOT_ID))?;', '        let Ok(text) = self.host.read(Path::new(BOOT_ID)) else {\n            return Ok(None);\n        };')]),
 ('X8 publish and settle take an unread note as none (R-P3)', [('crates/skyzfs-agent/src/program.rs', '        Err(e) => Some(failed(&e, err)),', '        Err(_) => None,')]),
 ('X9 a reopen ended by a signal read as done (R-C2)', [('crates/skyzfs-agent/src/zfs/cli.rs', '        let output = self.exec("zpool", &args, &[], self.limits.reopen)?;\n        if output.code == Some(0) {', '        let output = self.exec("zpool", &args, &[], self.limits.reopen)?;\n        if output.code.unwrap_or(0) == 0 {')]),
 ('X10 a pool gone during the look not the end (R-L1)', [('crates/skyzfs-agent/src/engine/lost.rs', '            let done = state.as_ref().is_none_or(PoolState::suspended);', '            let done = state.as_ref().is_some_and(PoolState::suspended);')]),
 ('X11 LOOK_MAX 60 s (R-L2)', [('crates/skyzfs-agent/src/engine/lost.rs', 'pub const LOOK_MAX: Duration = Duration::from_secs(10);', 'pub const LOOK_MAX: Duration = Duration::from_secs(60);')]),
 ('L1 the Clear arm asks no go before a begin', [('crates/skyzfs-agent/src/program.rs', '                        // #170). The settle below then ends at once.\n                        synced = k;\n                        clear = k == 0 || go();', '                        // #170). The settle below then ends at once.\n                        synced = k;\n                        clear = true;')]),
 ('L2 the Unseen arm asks no go before a begin', [('crates/skyzfs-agent/src/program.rs', '                        synced = k;\n                        clear = k == 0 || go();\n                    }\n                    Ok(Past::Behind {', '                        synced = k;\n                        clear = true;\n                    }\n                    Ok(Past::Behind {')]),
 ('L3 the Unseen arm count lost in run', [('crates/skyzfs-agent/src/program.rs', '                        synced = k;\n                        clear = k == 0 || go();\n                    }\n                    Ok(Past::Behind {', '                        synced = 0;\n                        clear = k == 0 || go();\n                    }\n                    Ok(Past::Behind {')]),
 ('L4 the engine Unseen count zeroed', [('crates/skyzfs-agent/src/engine.rs', '                Ok(_) | Err(_) => return Ok(Past::Unseen { synced }),', '                Ok(_) | Err(_) => return Ok(Past::Unseen { synced: 0 }),')]),
 ('L5 the syncs error arm count zeroed', [('crates/skyzfs-agent/src/program.rs', '                        synced = agent.last_synced();', '                        synced = 0;')]),
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
