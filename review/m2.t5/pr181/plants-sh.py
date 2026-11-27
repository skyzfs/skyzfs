import os, shutil, subprocess, sys, tempfile
SRC = sys.argv[1]  # the repo's tests directory (kernel and lib are copied)
TMP = sys.argv[2] if len(sys.argv) > 2 else None
M = [
 ('rcase: no recover', "    \"ret_$SCEN\" \"${hooks[@]}\"\n    recover\n", "    \"ret_$SCEN\" \"${hooks[@]}\"\n"),
 ('rt_after: rig hold kept', "        zfs release \"$RIG_TAG\" \"$RIG_HOLD\" || rig_error \"zfs release $RIG_TAG $RIG_HOLD failed\"\n        echo \"the rig's hold", "        echo \"the rig's hold"),
 ('rt_after: agent holds kept', "        zfs release \"$HOLD_TAG\" \"$n\" && echo \"$n\" >> \"$CDIR/holds-released.txt\"\n    done\n    rc=$(cat \"$CDIR/publish.rc\" 2>/dev/null || echo -)\n    printf '%s\\t%s\\t%s\\t%s\\t-\\n'", "        echo \"$n\" >> \"$CDIR/holds-released.txt\"\n    done\n    rc=$(cat \"$CDIR/publish.rc\" 2>/dev/null || echo -)\n    printf '%s\\t%s\\t%s\\t%s\\t-\\n'"),
 ('rt_after: abandoned names not asked', "$1 == \"abandoned\" { print $2 }", "$1 == \"abandonedX\" { print $2 }"),
 ('K: keep 3', "--every-s \"$RUN_EVERY\" --keep 2 \"$@\"", "--every-s \"$RUN_EVERY\" --keep 3 \"$@\""),
 ('S: dump after round 3', "if ! await_rounds publish 4 240; then", "if ! await_rounds publish 3 240; then"),
 ('S: release before the dump', "    adump stuck-dump\n    rt_snaps stuck-snaps\n    snaprow \"$t1\" stuck-row.txt || true\n    zfs release \"$RIG_TAG\" \"$RIG_HOLD\" || rig_error \"zfs release $RIG_TAG $RIG_HOLD failed\"\n", "    zfs release \"$RIG_TAG\" \"$RIG_HOLD\" || rig_error \"zfs release $RIG_TAG $RIG_HOLD failed\"\n    adump stuck-dump\n    rt_snaps stuck-snaps\n    snaprow \"$t1\" stuck-row.txt || true\n"),
 ('X: run2 keep 2', "    abg run2 run --every-s \"$RUN_EVERY\" --keep 1\n    run_rounds_then_term run2 2", "    abg run2 run --every-s \"$RUN_EVERY\" --keep 2\n    run_rounds_then_term run2 2"),
 ('B: nine rounds', "run_rounds_then_term publish 10", "run_rounds_then_term publish 9"),
 ('A-old: no wait for abandoned', "        astamp publish 60 '^abandoned ' || true\n", ""),
 ('A-person: no give-up', "    acmd giveup recover --name \"$t\" --give-up --limit-s 120\n", ""),
 ('rec: X-destroy point', "rcase X-destroy retkill destroy-member:after:1", "rcase X-destroy retkill destroy-member:after:2"),
 ('cases.tsv line dropped', "    printf '%s\\t%s\\t%s\\t%s\\t-\\n' \"$(basename \"$CDIR\")\" \"$SCEN\" \"${KILL:--}\" \"$rc\" >> \"$RDIR/cases.tsv\"\n", ""),
]
M.insert(0, ('BASE (no plant)', None, None))
for name, old, new in M:
    d = tempfile.mkdtemp(prefix='msh', dir=TMP)
    shutil.copytree(os.path.join(SRC, 'kernel'), os.path.join(d, 'tests', 'kernel'))
    shutil.copytree(os.path.join(SRC, 'lib'), os.path.join(d, 'tests', 'lib'))
    k = os.path.join(d, 'tests', 'kernel')
    p = os.path.join(k, 'agent-record.sh'); s = open(p).read(); n = 1 if old is None else s.count(old)
    if n != 1:
        print(f'PLANT-ERROR {name} count={n}'); shutil.rmtree(d); continue
    if old is not None:
        open(p, 'w').write(s.replace(old, new))
    r = subprocess.run(['bash', os.path.join(k, 'test_agent_record.sh')], capture_output=True, text=True, timeout=600)
    fl = [ln for ln in r.stdout.splitlines() if ln.startswith(('FAIL', 'not ok', 'NOT'))]
    print(('caught ' if r.returncode else 'MISSED ') + name + (': ' + fl[0][:110] if fl else ''), flush=True)
    shutil.rmtree(d)
