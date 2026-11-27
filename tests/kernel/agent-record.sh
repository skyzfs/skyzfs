# shellcheck shell=bash
# The agent rig's courses (M2.T5 stage 1; design §5.2 and §5.4, the
# plan of M2.6): the real agent (bins/skyzfs-agent, built with its test
# hooks) publishes snapshots of a real pool whose device the real daemon
# serves with its uploads (cut-record.sh's CUT_UPLOAD=1), and the rig
# loses the agent, the daemon or both at each step of a publish; arun
# drives `skyzfs-agent run`. Sourced by cut-record.sh, which calls the
# courses by name (rec_aplain, rec_adaemon, rec_arare, rec_arun,
# rec_afail);
# agent_check.py (their CUT_CHECK) judges what
# each case left, and node B verifies every snapshot published.
#
# A case is one publish begun by the agent, under a scenario (below),
# with or without a kill of the agent at one of its points (`--kill-at
# EFFECT:SIDE:N`, SIGKILL at the Nth reach), and then what the next
# start or tick of the agent does: `settle` (no hooks), as long as it
# takes (600 s at most). Its files are in DIR/cases/ID:
#
# - agent/: the agent's directory of the volume, its own for the case
#   (a fresh directory, so that the case's first run enrols, and a kill
#   at `enrol` is a case like any other);
# - trace.tsv: the points the case's first run reached (--trace-points),
#   on CLOCK_MONOTONIC, the request log's clock;
# - publish.{out,err,rc}: that run; settle.{out,err,rc}; where node B
#   finds no snap/T after the settle, retry.{out,err,rc}: a new publish
#   in the same directory, which must publish (the milestone's
#   criterion: "snap/t absent and a retry publishes it");
# - record-before.{txt,err,rc} and record-after.{txt,err,rc}: the record
#   (`skyzfs-agent record`) before the settle and after the last run,
#   with its status: a record that does not read (exit 6) is a verdict;
# - first.{out,err,rc} (sweep): the publish that marks the directory;
# - both-kill.tsv (both): the daemon's status after the rig's SIGKILL;
# - snaprow.txt: node B's `snap-row` of T after the settle;
# - snapshots-after.txt: the pool's snapshots after the last run (a take
#   that the next start destroyed must be gone: pin_ID_moved);
# - holds.txt: every hold on the pool's snapshots after the last run,
#   read before the rig releases any it left (holds-released.txt);
# - manifests.txt: the files left in the agent's manifests directory;
# - assist.txt: `skyzfs:assist` on the pool and every descendant, with
#   its source;
# - names.txt: T, the case's own publish (the last name its first run's
#   trace gives; none when it was killed before it had one);
# - verdicts.tsv: what the rig saw the agent or the daemon do wrong while
#   it drove the case (PIN, what), which the check fails as
#   pin_ID_PIN: an agent that did not end, ask or withdraw in time, a
#   row that did not land, a daemon that did not stop on SIGTERM, had
#   ended by itself before the rig's kill, or ended before it was ready
#   when served again (D5 of #158's review and D10 of #159's: a verdict,
#   not the rig's error). The daemon's stop at the course's end goes to
#   DIR/cases/course/verdicts.tsv, a directory that is no case;
# - rig-error.txt: the rig's own error in the case (a snapshot it could
#   not destroy, no name before the kill): the check's ERROR for the
#   case, which it does not judge further; the next case still runs
#   (G6);
# - recovered.txt: what was left running or frozen when the scenario
#   ended, and ended (recover).
#
# The case's line in DIR/cases.tsv: ID, scenario, kill point (`-` for
# none), the first run's status, T (`-`). After each case the agent is
# killed, the store thawed and the daemon served again where the
# scenario left any of them otherwise (recover), so that a case that
# went wrong does not take the next with it. Where the rig fails outside
# a case (a pool command, node B, the writer, a command that failed
# under errexit), the check still judges the cases done before
# (agent_judged, cut-record.sh's ON_RIG_ERROR), with DIR/rig-error.tsv
# (the case under way or `none`, the error), once the agent is killed,
# the store thawed, the writer stopped and the pool exported, so that
# node B can look (agent_quiet); a verdict among them ends the recording
# 1, else it ends 2. A trace that gives no
# points is DIR/points-error.tsv (the traced case, the error), the
# check's ERROR beside the traced case's own pins. Each name published in the
# case (T where node B has it, the retry's, a scenario's own) is a line
# of DIR/published.tsv: ID, name, the snapshot's createtxg and guid on
# node A, and the commit the agent printed for it (`-` where it printed
# none: it was published by a run that was killed after); the SHA-256 of
# its files, read through .zfs/snapshot, are DIR/snap-NAME.sha256. A name
# published whose snapshot node A no longer has (one destroyed and
# published all the same) is a line of DIR/published-gone.tsv: ID, name;
# the check fails it.
#
# The scenarios (scen_NAME):
# - plain: the publish on the pool as it goes on (a writer beside it).
# - restart (d1; Look, Count, Unasked and asking anew): object storage
#   frozen (CUT_S3_DOWN, `docker stop`) before the agent starts, so that
#   no cut can commit and the publish pends; at `asked:after` the daemon
#   is killed (kill -9), node B reads snap/T with the daemon down
#   (between.txt: the premise that it had not landed), the store thaws
#   and the next instance serves. The agent goes on by itself.
# - assist: the store frozen; the agent with --assist-after-s 2 pends,
#   and from `asked:after` 10 s pass with its syncs going on; then the
#   store thaws.
# - destroy (Withdraw, Abandon, Unlink): the store frozen; at
#   `asked:after` the rig releases the agent's hold and destroys pool@T,
#   as a person might; the store thaws once the agent's withdrawal is
#   answered (or, the agent killed before it, once a settle has
#   withdrawn), so that no cut can publish T after the destroy, and the
#   agent then looks snap/T up.
# - tooold: the store frozen; the agent with --age-max-s 8 gives the
#   publish up as too old; the store thaws as in destroy.
# - sweep (Sweep): a publish first, which marks the directory, then a
#   hold of the agent's tag that no publish names, on a snapshot of the
#   rig's (skz-sweep-ID): the case's publish must release it.
# - gone: the agent killed at `holding:before` (the snapshot taken, no
#   hold yet), then pool@T destroyed: a snapshot destroyed before the
#   hold is not published (M2.6's criterion).
# - unmarked: a fresh directory, and a hold of the agent's tag on a
#   snapshot of the rig's: the publish must refuse to sweep (exit 6) and
#   release nothing.
# - dhook HOOK (d2 to d5): the daemon served again with `--kill-at-upload
#   HOOK` (carry-put:1, carry-swap:1, manifest:1, publish:1); the daemon
#   kills itself inside the publish, and the next instance serves. For
#   the carry hooks the store is frozen from 3 s before the agent starts
#   until the publish is accepted, so that the uploader reads no cut that
#   could carry it before (the hook does not choose such a cut). A
#   killed instance with no kill line in its log is the rig's error
#   (exit 2: the lead's ruling on #150).
# - late (d6): the store frozen; the agent stopped (SIGSTOP) at
#   `asked:after`; the store thaws, and once node B finds snap/T the
#   daemon is killed and served again; then the agent goes on
#   (SIGCONT), with the row landed and the answer never read.
# - both: the agent killed at its point, and the daemon killed as soon
#   as the agent has died; the next instance serves, then the settle.
# - run, runkill, runtwice, runheld, runbad (M2.7a): `skyzfs-agent run
#   --every-s 10` in the background, its round lines' arrivals on the
#   rig's clock in LABEL.rounds.tsv, the SIGTERM's and the end's times
#   in LABEL.term-at and LABEL.ended-at. run: three rounds, then SIGTERM
#   between two (exit 0). runkill: killed at the case's point, then a
#   new run in the same directory (run2.{out,err,rc}): two rounds, then
#   SIGTERM. runtwice: a SIGTERM and a SIGINT at once while the first
#   round works (exit 1). runheld: after the first round, `recover` and
#   `settle` of the volume (held, held-settle: exit 8 each), then the
#   third round and SIGTERM. runbad corrupt|volume: between two rounds
#   the record made corrupt or replaced by another volume's (other.*:
#   the agent's own, from a volume directory with another id), and the
#   daemon's socket moved away so that the next begin fails and the
#   engine reads the record again (exit 6); then both put back.
#   runrewind (M2.7d): the pool imported at an older group with the
#   daemon's cuts of the newer ones held (rewind-*), then a run whose
#   first round syncs past them and publishes; two rounds, then SIGTERM.
#
# The courses:
# - aplain (job A): a plain publish traced (case `trace`), then a case
#   at each point its trace reached (agent_check.py points), in the
#   shard CUT_AGENT_SHARD (K/N: every Nth point from the Kth; 1/1 by
#   default). CUT_AGENT_POINTS, where set, names the points instead.
# - adaemon (job D): w0 (plain), d1 (restart), d2 to d5 (dhook), d6
#   (late), and both lost at ask:after:1, asked:after:1, done:before:1.
# - arare (job K): `trace` (plain), then each of restart, assist,
#   destroy, tooold and sweep traced, and a case at each point its trace
#   reached from the first the plain trace did not reach (by effect,
#   side and reach) on, the steps it shares with a plain publish after
#   its own included (G1 of #158's review), each point at its first
#   three reaches and its last (agent_check.py points); then gone and
#   unmarked.
# - arun (job R): R-run (run), R-kill (runkill at intent:after:2, the
#   second round's intent), R-twice (runtwice), R-held (runheld),
#   R-corrupt and R-volume (runbad), R-rewind (runrewind).
# - afail (jobs f and fz): a volume that fails while `run` serves
#   it (M2.7c), cases U and W; below, at rec_afail.
#
# A stall (eight asks with no cut counted, then given up) is left to the
# agent's fakes: on a real daemon a stall needs a cut that is not
# counted while the assist writes groups, and no hook makes one (the
# lead, M2.T5's plan). So is a power cut under the agent's record
# (#144's model of a file system).
#
# Needs CUT_AGENT (bins/skyzfs-agent built with --features test-hooks),
# CUT_S3_DOWN and CUT_S3_UP (the workflow freezes the store with
# `docker stop` and thaws it with `docker start`).
# The scenarios are called by name ("scen_$s"), which shellcheck cannot
# follow, and the names this file uses that cut-record.sh sets (RDIR,
# SERVE_RC and the others) are not visible to it.
# shellcheck disable=SC2329,SC2034,SC2153
AGENT=${CUT_AGENT:-}
AGENT_SHARD=${CUT_AGENT_SHARD:-1/1}
AGENT_POINTS=${CUT_AGENT_POINTS:-}
# The agent rig's daemon: a lease long enough for the store's freezes.
AGENT_SERVE=(--lease-ttl-s 300)
AGENT_HOOKS="carry-put:1 carry-swap:1 manifest:1 publish:1"
AGENT_BOTH="ask:after:1 asked:after:1 done:before:1"
AGENT_RARE="restart assist destroy tooold sweep"
HOLD_TAG=skyzfs-publish
CDIR=""; ADIR=""; SERVE_K=1; SCEN=""; KILL=""
[[ $AGENT_SHARD =~ ^([1-9][0-9]*)/([1-9][0-9]*)$ ]] && [ "${BASH_REMATCH[1]}" -le "${BASH_REMATCH[2]}" ] \
    || { echo "CUT_AGENT_SHARD is K/N with 1 <= K <= N, not $AGENT_SHARD" >&2; exit 64; }
for r in $RECS; do
    [[ " $AGENT_NAMED " == *" $r "* ]] || continue
    [ -x "$AGENT" ] || { echo "$r needs CUT_AGENT, the agent's binary built with its test hooks" >&2; exit 64; }
    [ -n "$S3_DOWN" ] && [ -n "$S3_UP" ] || { echo "$r needs CUT_S3_DOWN and CUT_S3_UP" >&2; exit 64; }
done
for p in $AGENT_POINTS; do
    [[ $p =~ ^[a-z]+:(before|after):[1-9][0-9]*$ ]] || { echo "CUT_AGENT_POINTS: not EFFECT:SIDE:N: $p" >&2; exit 64; }
done

# The agent's own records and the pool's state for the check.
acheck() { python3 -I "$HERE/agent_check.py" "$@"; }
# PIN TEXT: a verdict on the agent or the daemon in the case, for the
# check to fail (pin_ID_PIN).
verdict() {
    printf '%s\t%s\n' "$1" "$(printf '%s' "$2" | tr '\t\n' '  ')" >> "$CDIR/verdicts.tsv"
    log "$NAME: case $(basename "$CDIR"): verdict $1: $2"
}
# TEXT: the rig's own error in the case; the scenario returns after it.
case_error() {
    printf '%s\n' "$(printf '%s' "$*" | tr '\t\n' '  ')" >> "$CDIR/rig-error.txt"
    log "$NAME: case $(basename "$CDIR"): the rig's own error: $*"
}
# LABEL CMD [OPTION...]: the agent, as the next start or tick runs it,
# in the case's directory; its output in CDIR/LABEL.{out,err,rc}.
# The agent's RUN is the daemon's --run-dir, as on a node; the agent finds
# the volume's socket under it itself. vrun is that socket's directory, as
# the daemon names it (RUN/v-<dev>-<ino> of VOL in hex,
# skyzfs_control::socket_path), for the rig's own check that it is there.
vrun() { printf '%s/v-%x-%x' "$RUN" "$(stat -c %d "$VOL")" "$(stat -c %i "$VOL")"; }
acmd() {
    local label=$1 cmd=$2 rc=0
    shift 2
    timeout 900 "$AGENT" "$cmd" --volume "$VOL" --run "$RUN" --device "$DEV" --pool "$POOL" --dir "$ADIR" "$@" \
        > "$CDIR/$label.out" 2> "$CDIR/$label.err" || rc=$?
    echo "$rc" > "$CDIR/$label.rc"
}
# LABEL CMD [OPTION...]: acmd in the background (AGENT_PID), with no
# timeout(1) between, so that a SIGSTOP reaches the agent itself.
abg() {
    local label=$1 cmd=$2
    shift 2
    "$AGENT" "$cmd" --volume "$VOL" --run "$RUN" --device "$DEV" --pool "$POOL" --dir "$ADIR" "$@" \
        > "$CDIR/$label.out" 2> "$CDIR/$label.err" &
    AGENT_PID=$!
    echo "$AGENT_PID" > "$CDIR/$label.pid"
}
# LABEL SECONDS: wait for the agent in the background to end; its status
# in CDIR/LABEL.rc. One that has not ended is killed (status `timeout`),
# and that is a verdict.
await_agent() {
    local _i rc=0
    for _i in $(seq 1 $(($2 * 4))); do kill -0 "$AGENT_PID" 2>/dev/null || break; sleep 0.25; done
    if kill -0 "$AGENT_PID" 2>/dev/null; then
        kill -9 "$AGENT_PID" 2>/dev/null; wait "$AGENT_PID" 2>/dev/null || true; AGENT_PID=""
        verdict "$1_ended" "the agent's $1 had not ended $2 s on: $(tail -n 2 "$CDIR/$1.err")"
        echo timeout > "$CDIR/$1.rc"
        return 0
    fi
    wait "$AGENT_PID" 2>/dev/null || rc=$?
    AGENT_PID=""
    echo "$rc" > "$CDIR/$1.rc"
}
# EFFECT SIDE SECONDS: until the case's trace has reached that point, or
# the agent in the background has ended, for up to SECONDS. 0 when the
# point was reached (a kill at an earlier point never reaches it).
point_reached() { awk -F '\t' -v e="$1" -v s="$2" '$2 == e && $3 == s { f = 1 } END { exit !f }' "$CDIR/trace.tsv" 2>/dev/null; }
wait_point() {
    local t0=$SECONDS
    while :; do
        point_reached "$1" "$2" && return 0
        kill -0 "$AGENT_PID" 2>/dev/null || { point_reached "$1" "$2"; return; }
        [ $((SECONDS - t0)) -lt "$3" ] || return 1
        sleep 0.02
    done
}
# The case's publish: the last name the trace gives, or nothing. A
# sweep's points name the snapshot whose hold it releases, not a publish.
case_name() { awk -F '\t' 'NF >= 4 && $4 != "" && $2 != "sweep" { t = $4 } END { print t }' "$CDIR/trace.tsv" 2>/dev/null; }
# The store frozen and thawed (CUT_S3_DOWN, CUT_S3_UP). The watcher is
# paused a second first, so that no call of its straddles the freeze.
freeze() {
    touch "$WORK/watch.pause"
    sleep 1
    S3_IS_DOWN=1
    bash -c "$S3_DOWN" >> "$RDIR/s3.log" 2>&1 || rig_error "CUT_S3_DOWN failed: $(tail -n 2 "$RDIR/s3.log")"
    mark "freeze-$(basename "$CDIR")"
}
thaw() {
    bash -c "$S3_UP" >> "$RDIR/s3.log" 2>&1 || rig_error "CUT_S3_UP failed: $(tail -n 2 "$RDIR/s3.log")"
    S3_IS_DOWN=""
    mark "thaw-$(basename "$CDIR")"
    rm -f "$WORK/watch.pause"
}
# The next instance of the daemon on the volume, with AGENT_SERVE and
# the options given.
serve_again() {
    SERVE_K=$((SERVE_K + 1))
    SERVE_ARGS=("${AGENT_SERVE[@]}" "$@")
    serve_start "$SERVE_K"
}
# The daemon ended on SIGTERM (status 0), then served again with the
# options given: how a hook is armed between cases.
reserve() {
    # A daemon that had ended before the SIGTERM ended unasked: a verdict.
    if ! kill -TERM "$SERVE_PID" 2>/dev/null; then
        local st=unknown
        if serve_end 5; then st=$SERVE_RC; fi
        verdict daemon_stopped "skyzfsd serve $SERVE_K had ended before SIGTERM (status $st): $(tail -n 2 "$(serve_log "$SERVE_K")")"
    elif ! serve_end 60; then
        verdict daemon_stopped "skyzfsd serve $SERVE_K did not end 60 s after SIGTERM"
        kill -9 "$SERVE_PID" 2>/dev/null || true
        # Unreaped, it is not to be taken later for a daemon that ended
        # by itself.
        serve_end 10 || { SERVE_PID=""; rig_error "skyzfsd serve $SERVE_K did not end 10 s after SIGKILL"; }
    elif [ "$SERVE_RC" != 0 ]; then
        verdict daemon_stopped "skyzfsd serve $SERVE_K ended with $SERVE_RC on SIGTERM: $(tail -n 2 "$(serve_log "$SERVE_K")")"
    fi
    serve_again "$@"
}
# The daemon killed by the rig (restart, late). One that had ended by
# itself before the kill is a verdict, with its status (D10 of #159's
# review). The kill's own status cannot say so: bash has reaped a child
# that ended, so the kill fails, but it fails as well for other
# reasons. The status wait gives does: 137 for the rig's kill.
akill_serve() {
    local rc=0 k=$SERVE_K
    kill -9 "$SERVE_PID" 2>/dev/null || true
    wait "$SERVE_PID" 2>/dev/null || rc=$?
    SERVE_PID=""
    [ "$rc" = 137 ] || verdict daemon_serving "skyzfsd serve $k had ended (status $rc) before the rig's kill: $(tail -n 2 "$(serve_log "$k")")"
}
# K: the log of the daemon's Kth instance.
serve_log() { if [ "$1" = 1 ]; then echo "$RDIR/serve.log"; else echo "$RDIR/serve-$1.log"; fi; }
# After a case's scenario: whatever it left running or frozen is ended,
# so that the next case starts as every case does (G6 of #158's review):
# the agent killed, the store thawed, the daemon serving. A daemon that
# had ended without a kill of the rig's or its hook's is a verdict; in a
# dhook case the rig's own error about the daemon's end comes first, and
# only there and only until the hook's end is recorded in kill.tsv: the
# instance served after it has no hook armed (D9 of #159's review: in
# the other scenarios the rig never stops the daemon, so one that ended
# did so by itself).
recover() {
    if [ -n "$AGENT_PID" ]; then
        kill -CONT "$AGENT_PID" 2>/dev/null || true
        kill -9 "$AGENT_PID" 2>/dev/null || true
        wait "$AGENT_PID" 2>/dev/null || true
        AGENT_PID=""
        echo "the agent killed" >> "$CDIR/recovered.txt"
    fi
    if [ -n "$S3_IS_DOWN" ]; then
        thaw
        echo "the store thawed" >> "$CDIR/recovered.txt"
    fi
    if [ -z "$SERVE_PID" ] || ! kill -0 "$SERVE_PID" 2>/dev/null; then
        local k=$SERVE_K st=unknown
        if [ -n "$SERVE_PID" ] && serve_end 5; then st=$SERVE_RC; fi
        SERVE_PID=""
        if [ "$SCEN" = dhook ] && [ -s "$CDIR/rig-error.txt" ] && [ ! -e "$CDIR/kill.tsv" ]; then
            echo "skyzfsd serve $k had ended (status $st), after the rig's own error in the case" >> "$CDIR/recovered.txt"
        else
            verdict daemon_serving "skyzfsd serve $k had ended (status $st) with no kill of the rig's: $(tail -n 2 "$(serve_log "$k")")"
        fi
        echo "the daemon served again" >> "$CDIR/recovered.txt"
        serve_again
    fi
}
# A writer beside the publishes: 32 KiB to one of eight files every
# half second, synced, so that groups and the intent log go on.
writer_on() {
    rm -f "$WORK/writer.stop"
    ( k=0
      while [ ! -e "$WORK/writer.stop" ]; do
          k=$((k + 1))
          dd if=/dev/urandom of="$MNT/w$((k % 8))" bs=32K count=1 conv=fsync status=none
          sleep 0.5
      done
      echo "$k writes" ) > "$RDIR/writer.log" 2>&1 &
    WRITER=$!
}
writer_off() {
    local _i
    touch "$WORK/writer.stop"
    for _i in $(seq 1 120); do kill -0 "$WRITER" 2>/dev/null || break; sleep 0.5; done
    kill -0 "$WRITER" 2>/dev/null && { kill -9 "$WRITER" 2>/dev/null; rig_error "the writer did not end 60 s after its stop"; }
    wait "$WRITER" || rig_error "the writer failed: $(tail -n 2 "$RDIR/writer.log")"
    WRITER=""
}

# NAME: node A's facts of a snapshot published in the case: its files'
# digests, read through .zfs/snapshot, its createtxg and guid, and the
# commit the agent printed for it (or -).
published() {
    local n=$1 printed
    [ -e "$RDIR/snap-$n.sha256" ] && return 0
    grep -qxF "$(printf '%s\t%s' "$(basename "$CDIR")" "$n")" "$RDIR/published-gone.tsv" 2>/dev/null && return 0
    zfs list -H -o name -t snapshot -r "$POOL" > "$CDIR/snapshots.txt" 2> "$CDIR/snapshots.err" \
        || rig_error "zfs list of $POOL's snapshots failed: $(tail -n 2 "$CDIR/snapshots.err")"
    if ! grep -qxF "$POOL@$n" "$CDIR/snapshots.txt"; then
        # Published, and node A has no such snapshot: the check's verdict
        # (a snapshot destroyed before the hold is never published).
        printf '%s\t%s\n' "$(basename "$CDIR")" "$n" >> "$RDIR/published-gone.tsv"
        return 0
    fi
    (cd "$MNT/.zfs/snapshot/$n" && find . -maxdepth 1 -type f -printf '%P\n' | LC_ALL=C sort | xargs -r sha256sum --) \
        > "$RDIR/snap-$n.sha256" || rig_error "reading the files of $POOL@$n failed"
    printed=$(cat "$CDIR"/*.out 2>/dev/null | awk -v n="snap/$n" '$1 == n { c = $2 } END { print c }')
    printf '%s\t%s\t%s\t%s\t%s\n' "$(basename "$CDIR")" "$n" "$(zfs get -Hp -o value createtxg "$POOL@$n")" \
        "$(zfs get -Hp -o value guid "$POOL@$n")" "${printed:--}" >> "$RDIR/published.tsv"
}
# NAME FILE: node B's snap-row of NAME into CDIR/FILE; 0 when it has the
# ref.
snaprow() {
    nodeb snap-row --snapshot "$1" > "$CDIR/$2" 2> "$CDIR/$2.err" || rig_error "node B snap-row $1 failed: $(tail -n 2 "$CDIR/$2.err")"
    ! grep -qx "$(printf 'ref\tnone')" "$CDIR/$2"
}
# After the case's first run: the settle, as the next start of the agent
# does; node B's look at T; a retry where snap/T is absent; and the
# pool's state, for the check.
# LABEL: the record's dump into CDIR/LABEL.txt, its errors and status
# beside it.
adump() {
    local rc=0
    "$AGENT" record --dir "$ADIR" > "$CDIR/$1.txt" 2> "$CDIR/$1.err" || rc=$?
    echo "$rc" > "$CDIR/$1.rc"
}
asettle() {
    local t n rc
    adump record-before
    acmd settle settle --limit-s 600 "${CASE_AGE[@]}"
    t=$(case_name)
    echo "${t:--}" > "$CDIR/names.txt"
    if [ -n "$t" ] && snaprow "$t" snaprow.txt; then
        published "$t"
    else
        acmd retry publish --limit-s 600
        n=$(awk '$1 ~ /^snap\// { sub(/^snap\//, "", $1); print $1 }' "$CDIR/retry.out" | tail -n 1)
        [ -z "$n" ] || published "$n"
    fi
    # The other names the case's runs printed as published (a scenario's
    # own publish, the sweep's first).
    cat "$CDIR"/*.out 2>/dev/null | awk '$1 ~ /^snap\// { sub(/^snap\//, "", $1); print $1 }' | sort -u > "$CDIR/printed.txt"
    while read -r n; do published "$n"; done < "$CDIR/printed.txt"
    adump record-after
    zfs list -H -o name -t snapshot -r "$POOL" > "$CDIR/snapshots-after.txt" 2> "$CDIR/snapshots-after.err" \
        || rig_error "zfs list of $POOL's snapshots after the case failed: $(tail -n 2 "$CDIR/snapshots-after.err")"
    zfs list -H -t snapshot -o name -r "$POOL" | xargs -r zfs holds -H > "$CDIR/holds.txt" 2> "$CDIR/holds.err" \
        || rig_error "zfs holds failed: $(tail -n 2 "$CDIR/holds.err")"
    find "$ADIR/manifests" -mindepth 1 -printf '%P\n' > "$CDIR/manifests.txt" 2>/dev/null || : > "$CDIR/manifests.txt"
    zfs get -H -r -o name,value,source skyzfs:assist "$POOL" > "$CDIR/assist.txt" \
        || rig_error "zfs get skyzfs:assist failed"
    # A hold the case left would make the next case's fresh directory
    # refuse to sweep it (exit 6): the check judges holds.txt, and the
    # rig lets it go.
    : > "$CDIR/holds-released.txt"
    awk -F '\t' -v tag="$HOLD_TAG" '$2 == tag { print $1 }' "$CDIR/holds.txt" | while read -r n; do
        zfs release "$HOLD_TAG" "$n" && echo "$n" >> "$CDIR/holds-released.txt"
    done
    rc=$(cat "$CDIR/publish.rc" 2>/dev/null || echo -)
    printf '%s\t%s\t%s\t%s\t%s\n' "$(basename "$CDIR")" "$SCEN" "${KILL:--}" "$rc" "${t:--}" >> "$RDIR/cases.tsv"
    log "$NAME: case $(basename "$CDIR") ($SCEN${KILL:+, killed at $KILL}): first run $rc, T ${t:-none}, settle $(cat "$CDIR/settle.rc")"
}
# ID SCENARIO [KILL [ARG]]: one case.
acase() {
    local hooks=()
    CDIR=$RDIR/cases/$1; ADIR=$CDIR/agent; SCEN=$2; KILL=${3:-}
    # The bound on a publish's age, for every run of the case but the
    # retry (tooold's 8 s: a settle with the default bound publishes T).
    CASE_AGE=()
    rm -rf "${CDIR:?}"; mkdir -p "$ADIR"
    hooks=(--trace-points "$CDIR/trace.tsv")
    [ -z "$KILL" ] || hooks+=(--kill-at "$KILL")
    "scen_$SCEN" "${4:-}" "${hooks[@]}"
    recover
    asettle
    # No case under way: the rig's error after it names none.
    CDIR=""
}

scen_plain() { shift; acmd publish publish --limit-s 600 "$@"; }
scen_restart() {
    shift
    freeze
    abg publish publish --limit-s 600 "$@"
    if wait_point asked after 60; then
        akill_serve
        thaw
        local t; t=$(case_name)
        snaprow "$t" between.txt || true
        serve_again
    else
        # A kill before asked never reaches it; with none, the agent
        # must (as in late and dhook).
        [ -n "$KILL" ] || verdict asked "the agent did not reach asked:after in 60 s"
        thaw
    fi
    await_agent publish 700
}
scen_assist() {
    shift
    freeze
    abg publish publish --limit-s 600 --assist-after-s 2 "$@"
    if wait_point asked after 60; then
        local _i
        for _i in $(seq 1 40); do kill -0 "$AGENT_PID" 2>/dev/null || break; sleep 0.25; done
    fi
    thaw
    await_agent publish 700
}
scen_destroy() {
    local t id
    shift
    id=$(basename "$CDIR")
    freeze
    abg publish publish --limit-s 600 "$@"
    if wait_point asked after 60; then
        t=$(case_name)
        if ! { lim 60 zfs release "$HOLD_TAG" "$POOL@$t" && lim 60 zfs destroy "$POOL@$t"; } > "$CDIR/destroy.log" 2>&1; then
            case_error "releasing and destroying $POOL@$t failed: $(tail -n 2 "$CDIR/destroy.log")"
            mv "$CDIR/destroy.log" "$CDIR/destroy-failed.log"
            return 0
        fi
        mark "destroyed-$id"
        frozen_until_withdrawn
    else
        [ -n "$KILL" ] || verdict asked "the agent did not reach asked:after in 60 s"
    fi
    thaw
    await_agent publish 700
}
# Frozen until the agent's withdrawal is answered, so that no cut after
# the loss can carry T; then its look at snap/T needs the store (run
# 45745575500: frozen, the agent looked for 300 s). An agent killed
# before it withdrew: its next start withdraws, the store still frozen
# (a settle of 30 s, which ends unsettled while it cannot look). An agent
# still running that has not withdrawn in 120 s is a verdict.
frozen_until_withdrawn() {
    wait_point withdraw after 120 && return 0
    if kill -0 "$AGENT_PID" 2>/dev/null; then
        verdict withdrew "the agent ran on and did not withdraw in 120 s"
        return 0
    fi
    acmd settle-frozen settle --limit-s 30 "${CASE_AGE[@]}"
}
scen_tooold() {
    shift
    CASE_AGE=(--age-max-s 8)
    freeze
    abg publish publish --limit-s 300 "${CASE_AGE[@]}" "$@"
    frozen_until_withdrawn
    thaw
    await_agent publish 400
}
# The stray snapshots of sweep and unmarked are named skz-*: the sweep
# reads the holds of those names alone (A4 of #165's review).
scen_sweep() {
    local id
    shift
    id=$(basename "$CDIR")
    # Its status is the check's to judge (pin_ID_first_publish).
    acmd first publish --limit-s 600
    z 60 zfs snapshot "$POOL@skz-sweep-$id"
    z 60 zfs hold "$HOLD_TAG" "$POOL@skz-sweep-$id"
    acmd publish publish --limit-s 600 "$@"
    zfs holds -H "$POOL@skz-sweep-$id" > "$CDIR/stray-holds.txt"
}
scen_gone() {
    local t
    shift
    acmd publish publish --limit-s 600 "$@"
    t=$(case_name)
    [ -n "$t" ] || { case_error "no name in the trace before the kill"; return 0; }
    if ! lim 60 zfs destroy "$POOL@$t" > "$CDIR/destroy.log" 2>&1; then
        case_error "destroying $POOL@$t failed: $(tail -n 2 "$CDIR/destroy.log")"
        mv "$CDIR/destroy.log" "$CDIR/destroy-failed.log"
    fi
}
scen_unmarked() {
    local id
    shift
    id=$(basename "$CDIR")
    z 60 zfs snapshot "$POOL@skz-stray-$id"
    z 60 zfs hold "$HOLD_TAG" "$POOL@skz-stray-$id"
    acmd publish publish --limit-s 120 "$@"
    zfs holds -H "$POOL@skz-stray-$id" > "$CDIR/stray-holds.txt"
    # The rig's own hold: let go, so that the retry can publish.
    z 60 zfs release "$HOLD_TAG" "$POOL@skz-stray-$id"
}
scen_dhook() {
    local hook=$1 killed=$SERVE_K
    shift
    reserve --kill-at-upload "$hook"
    killed=$SERVE_K
    case $hook in
    carry-*)
        # The hook never chooses a cut whose reading began before the
        # publish was accepted, though it may carry it (kill.rs): the
        # snapshot's own cut, read at once by an idle uploader while the
        # agent holds, did so in run 45747041657 (no kill). The store
        # frozen 3 s before the agent starts holds the uploader on a cut
        # taken before the agent's count, a step it makes again after the
        # thaw and does not read anew (uploader.rs); the store thaws once
        # the publish is accepted, so the first cut read after it is the
        # first that can carry it.
        freeze
        sleep 3
        abg publish publish --limit-s 600 "$@"
        wait_point asked after 60 || verdict asked "the agent did not reach asked:after in 60 s"
        thaw
        ;;
    *) abg publish publish --limit-s 600 "$@" ;;
    esac
    local _i
    for _i in $(seq 1 1200); do
        kill -0 "$SERVE_PID" 2>/dev/null || break
        kill -0 "$AGENT_PID" 2>/dev/null || break
        sleep 0.25
    done
    if ! kill -0 "$SERVE_PID" 2>/dev/null; then
        serve_end 5 || { case_error "the daemon's pid went away and serve_end did not see it end"; return 0; }
        printf 'killed_rc\t%s\nserve\t%s\n' "$SERVE_RC" "$killed" > "$CDIR/kill.tsv"
        cp "$RDIR/serve-$killed.log" "$CDIR/serve-killed.log"
        serve_again
    else
        printf 'killed_rc\tnone\nserve\t%s\n' "$killed" > "$CDIR/kill.tsv"
        cp "$RDIR/serve-$killed.log" "$CDIR/serve-killed.log"
        # The hook still armed: the next case's daemon must not have it.
        reserve
    fi
    await_agent publish 700
}
scen_late() {
    local t
    shift
    freeze
    abg publish publish --limit-s 600 "$@"
    wait_point asked after 60 || { verdict asked "the agent did not reach asked:after in 60 s"; return 0; }
    kill -STOP "$AGENT_PID"
    mark "stopped-$(basename "$CDIR")"
    adump record-stopped
    thaw
    t=$(case_name)
    local _i
    for _i in $(seq 1 240); do snaprow "$t" landed.txt && break; sleep 0.5; done
    snaprow "$t" landed.txt || { verdict landed "snap/$t had not landed 120 s after the thaw"; return 0; }
    akill_serve
    serve_again
    mark "continued-$(basename "$CDIR")"
    kill -CONT "$AGENT_PID"
    await_agent publish 700
}
scen_both() {
    local rc=0
    shift
    abg publish publish --limit-s 600 "$@"
    await_agent publish 700
    # As kill_serve, with the daemon's status kept: 137 where the SIGKILL
    # ended it, anything else where it had ended by itself.
    kill -9 "$SERVE_PID" 2>/dev/null || true
    wait "$SERVE_PID" 2>/dev/null || rc=$?
    SERVE_PID=""
    printf 'killed_rc\t%s\n' "$rc" > "$CDIR/both-kill.tsv"
    serve_again
}

# `skyzfs-agent run` (M2.7a): one process, a round every RUN_EVERY s.
RUN_EVERY=10
# LABEL N SECONDS: until the run in the background has printed N round
# lines on CDIR/LABEL.out, or has ended, for up to SECONDS. Each round
# line's arrival, on the rig's clock (its realtime, read every 0.1 s),
# is a line of CDIR/LABEL.rounds.tsv: the line's number, the time. 0
# when N lines came.
await_rounds() {
    local label=$1 want=$2 t0=$SECONDS k have
    k=$(wc -l 2>/dev/null < "$CDIR/$label.rounds.tsv") || k=0
    while :; do
        have=$(grep -c '^round ' "$CDIR/$label.out" 2>/dev/null) || have=0
        while [ "$k" -lt "$have" ]; do
            k=$((k + 1))
            printf '%s\t%s\n' "$k" "$EPOCHREALTIME" >> "$CDIR/$label.rounds.tsv"
        done
        [ "$have" -lt "$want" ] || return 0
        kill -0 "$AGENT_PID" 2>/dev/null || return 1
        [ $((SECONDS - t0)) -lt "$3" ] || return 1
        sleep 0.1
    done
}
# LABEL: the ms left to the next tick after the last round line of
# CDIR/LABEL.out (--every-s less its took_ms and late_ms; below 0 where
# it overran its tick, and the next round begins at once); 0 where there
# is no round line.
round_left() {
    awk -v e="$RUN_EVERY" '/^round / { for (i = 3; i <= NF; i++) { split($i, f, "="); v[f[1]] = f[2] }; l = e * 1000 - v["took_ms"] - v["late_ms"] } END { print int(l) }' "$CDIR/$1.out" 2>/dev/null || echo 0
}
# LABEL N: N round lines came (await_rounds), then more until one came
# with 3 s or more left to its next tick (three more at most), and the
# run is told to end with one SIGTERM there: between two rounds, while
# it waits for the tick. A round that overran its tick is followed at
# once by the next (program::schedule), so a SIGTERM right after its
# line would come while the engine works (Codex on #171). Round lines
# are stamped until the run ends, so that one printed after the signal
# has its arrival (the check's pin_ID_no_round_after_sigterm); the end
# within 60 s (await_agent). The signal's time and the end's, on the
# rig's clock, are CDIR/LABEL.term-at and LABEL.ended-at. Fewer than N
# lines in 120 s per line, or a run that ended before a line with 3 s
# left, is a verdict; a run alive with no such line in three more is the
# rig's error.
run_rounds_then_term() {
    local k=$2 limit=$((120 * $2)) left=0 have
    while await_rounds "$1" "$k" "$limit"; do
        left=$(round_left "$1")
        [ "$left" -lt 3000 ] && [ "$k" -lt $(($2 + 3)) ] || break
        k=$((k + 1)) limit=120
    done
    have=$(grep -c '^round ' "$CDIR/$1.out" 2>/dev/null) || have=0
    if [ "$have" -ge "$2" ] && [ "$left" -ge 3000 ] && kill -0 "$AGENT_PID" 2>/dev/null; then
        # Every line read so far is stamped before the signal (a line
        # that came after the last stamp, and before round_left read
        # it, would otherwise be stamped after the signal).
        await_rounds "$1" 0 0 || true
        kill -TERM "$AGENT_PID" 2>/dev/null || true
        echo "$EPOCHREALTIME" > "$CDIR/$1.term-at"
        await_rounds "$1" 1000000 60 || true
    elif [ "$have" -lt "$2" ]; then
        verdict rounds "the run printed $have round lines, not $2: $(tail -n 2 "$CDIR/$1.err")"
    elif ! kill -0 "$AGENT_PID" 2>/dev/null; then
        verdict rounds "the run ended after $have round lines, before one with 3 s left to its next tick: $(tail -n 2 "$CDIR/$1.err")"
    else
        # A run whose rounds overrun its tick again and again does what
        # its docs allow, and the case cannot signal it between two: the
        # rig's error, and the run ended with a SIGTERM that no pin reads.
        case_error "no round line with 3 s left to its next tick in $have rounds at --every-s $RUN_EVERY: no time between two rounds to signal the run in"
        kill -TERM "$AGENT_PID" 2>/dev/null || true
    fi
    await_agent "$1" 60
    # A line printed just before the end is stamped too, after the signal.
    await_rounds "$1" 1000000 0 || true
    echo "$EPOCHREALTIME" > "$CDIR/$1.ended-at"
}
# run: three rounds, then SIGTERM between two (exit 0).
scen_run() {
    shift
    abg publish run --every-s "$RUN_EVERY" "$@"
    run_rounds_then_term publish 3
}
# runkill: the run killed at the case's point (a point of its second
# round), then a new run in the same directory, as after a reboot: two
# rounds, then SIGTERM.
scen_runkill() {
    shift
    abg publish run --every-s "$RUN_EVERY" "$@"
    await_agent publish 300
    abg run2 run --every-s "$RUN_EVERY"
    run_rounds_then_term run2 2
}
# runtwice: two signals while the first round works, a SIGTERM and a
# SIGINT, sent at once: two signals of one kind sent together can be
# merged into one while both are pending, two kinds cannot. The second
# ends the run at once with 1. bash starts a command in the background
# with SIGINT ignored; the agent's handler (signal-hook, sigaction
# whatever the disposition was) takes its place.
scen_runtwice() {
    shift
    abg publish run --every-s "$RUN_EVERY" "$@"
    if wait_point asked after 60; then
        kill -TERM "$AGENT_PID" 2>/dev/null || true
        kill -INT "$AGENT_PID" 2>/dev/null || true
        echo "$EPOCHREALTIME" > "$CDIR/publish.term-at"
    else
        verdict asked "the agent did not reach asked:after in 60 s"
    fi
    await_agent publish 60
    echo "$EPOCHREALTIME" > "$CDIR/publish.ended-at"
}
# runheld: after the first round, `recover` and `settle` of the volume
# while the run holds its directory (exit 8 each); the run goes on to
# its third round, then SIGTERM. held.rounds is how many round lines
# the run had printed when the refusals ended.
scen_runheld() {
    local t
    shift
    abg publish run --every-s "$RUN_EVERY" "$@"
    if await_rounds publish 1 120; then
        t=$(case_name)
        acmd held recover --name "${t:-none}" --retry --limit-s 30
        acmd held-settle settle --limit-s 30
        grep -c '^round ' "$CDIR/publish.out" > "$CDIR/held.rounds" || true
    fi
    run_rounds_then_term publish 3
}
# The id of `remote` with its last character changed: another volume's.
other_remote() {
    awk '$1 == "id" { c = substr($2, length($2)); $2 = substr($2, 1, length($2) - 1) (c == "0" ? "1" : "0") } { print }' "$1"
}
# runbad corrupt|volume: between two rounds the rig makes the record
# corrupt, or puts another volume's record in its place, and moves the
# daemon's socket away, so that the next round's begin fails at the
# daemon's count and the engine reads the record again (Agent::restart):
# the run must end 6 after that round's line. Then the socket comes
# back and the record as it was, as a person who judged it would, so
# that the settle goes on. Another volume's record is the agent's own:
# a publish in a fresh directory of a volume directory that has only a
# `remote` of another id enrols the pool, saves the record, and finds
# no daemon there (other.{out,err,rc}). The rig acts after a round whose
# line came with 3 s or more left to the next tick.
scen_runbad() {
    local kind=$1 sock k left
    shift
    echo "$kind" > "$CDIR/bad.kind"
    if [ "$kind" = volume ]; then
        mkdir -p "$CDIR/other-vol" "$CDIR/other"
        other_remote "$VOL/remote" > "$CDIR/other-vol/remote"
        local adir=$ADIR vol=$VOL
        ADIR=$CDIR/other VOL=$CDIR/other-vol
        acmd other publish --limit-s 30
        ADIR=$adir VOL=$vol
        [ -s "$CDIR/other/record" ] || { case_error "no record of another volume: $(tail -n 2 "$CDIR/other.err")"; return 0; }
    fi
    abg publish run --every-s "$RUN_EVERY" "$@"
    for k in 1 2 3; do
        await_rounds publish "$k" 120 || break
        left=$(round_left publish)
        [ "${left:-0}" -lt 3000 ] || break
    done
    if ! kill -0 "$AGENT_PID" 2>/dev/null || [ "${left:-0}" -lt 3000 ]; then
        verdict rounds "no round line with 3 s left to its next tick in three rounds: $(tail -n 2 "$CDIR/publish.err")"
        await_agent publish 60
        return 0
    fi
    grep -c '^round ' "$CDIR/publish.out" > "$CDIR/bad.rounds"
    cp -p "$ADIR/record" "$CDIR/record.saved"
    if [ "$kind" = corrupt ]; then
        printf 'not a record\n' > "$ADIR/record"
    else
        cp "$CDIR/other/record" "$ADIR/record"
    fi
    sock=$(vrun)/control
    mv "$sock" "$sock.away"
    await_agent publish 60
    mv "$sock.away" "$sock"
    cp -p "$CDIR/record.saved" "$ADIR/record"
}
# runrewind (M2.7d): `run` after a rewind import. The writer stopped and
# the store frozen, so that no cut is uploaded until the store thaws
# (the daemon holds two at most, D7: the cut being uploaded, which
# predates the rewind, and its newest; the abandoned top is replaced by
# the import's first cut, so this case does not show design §9's
# publish with cuts of the abandoned history still held); groups written and synced (rewind_groups)
# until REWIND_BACK + 1 have committed since; the pool exported and
# imported at the group REWIND_BACK from the newest (`zpool import -T`).
# Its next groups are below those of the history it abandoned, and no
# cut opens as itself until they pass them (design §5.2). The store
# thaws, and the daemon's newest cut is read: the premise is one that
# opens only as a rewind (rewind-cuts.txt; rewind-top.txt the daemon's
# newest cut after the export, rewind-txg the import's group). Then
# `run`: its first round must sync past and begin a publish, which node
# B verifies; the writer comes back after that round; two rounds, then
# SIGTERM.
REWIND_BACK=8
# FIRST: written and synced, a group at a time, until REWIND_BACK + 1
# groups above FIRST have committed with something written (the txgs
# kstat's groups_written: one that wrote nothing may have left no
# uberblock to import at), for twelve syncs at most; those
# groups in REWIND_GROUPS (a global, not output: the syncs' rig_error
# must not run in a command substitution's subshell). Not `load`: its
# snapshots are node B's (snaps.tsv), and those of the abandoned
# history would be gone.
REWIND_GROUPS=()
rewind_groups() {
    local k
    REWIND_GROUPS=()
    for k in $(seq 1 12); do
        dd if=/dev/urandom of="$MNT/rewind-$k" bs=64K count=4 status=none
        z 60 zpool sync "$POOL"
        mapfile -t REWIND_GROUPS < <(groups_written | awk -v g="$1" '$1 > g')
        [ "${#REWIND_GROUPS[@]}" -le "$REWIND_BACK" ] || break
    done
}
scen_runrewind() {
    local first t
    shift
    writer_off
    freeze
    first=$(committed | tail -n 1)
    rewind_groups "${first:-0}"
    local groups=("${REWIND_GROUPS[@]}")
    if [ "${#groups[@]}" -le "$REWIND_BACK" ]; then
        case_error "${#groups[@]} groups committed in twelve syncs with the store frozen, not $((REWIND_BACK + 1))"
        writer_on
        return 0
    fi
    t=${groups[$(( ${#groups[@]} - REWIND_BACK ))]}
    echo "$t" > "$CDIR/rewind-txg"
    export_pool
    pagent ask cuts > "$CDIR/rewind-top.txt" 2>> "$RDIR/agent.err" \
        || rig_error "the daemon's cuts after the export: $(tail -n 2 "$RDIR/agent.err")"
    log "$NAME: case $(basename "$CDIR"): importing at group $t of ${groups[-1]} (the daemon's newest cut: $(cat "$CDIR/rewind-top.txt"))"
    import_pool -T "$t"
    thaw
    pagent ask cuts > "$CDIR/rewind-cuts.txt" 2>> "$RDIR/agent.err" \
        || rig_error "the daemon's cuts after the rewind: $(tail -n 2 "$RDIR/agent.err")"
    abg publish run --every-s "$RUN_EVERY" "$@"
    await_rounds publish 1 300 || true
    writer_on
    run_rounds_then_term publish 2
}

# The courses.
agent_begin() {
    SERVE_K=1
    create_pool
    load 1
    writer_on
    : > "$RDIR/cases.tsv"; : > "$RDIR/published.tsv"; : > "$RDIR/published-gone.tsv"
    rm -f "$RDIR/points-error.tsv" "$RDIR/rig-error.tsv"
    [ -S "$(vrun)/control" ] || rig_error "no socket control in $(vrun): under $RUN are $(find "$RUN" -maxdepth 2 | tr '\n' ' ')"
    printf 'run-dir\t%s\nagent-run\t%s\n' "$RUN" "$(vrun)" > "$RDIR/agent-run.tsv"
    ON_RIG_ERROR=agent_judged
    ON_DAEMON_WRONG=agent_daemon_wrong
}
# PIN TEXT: a verdict on the daemon in a step the courses share
# (cut-record.sh's daemon_wrong): the case's under way, or the course's
# (DIR/cases/course) after the cases, at the daemon's stop.
agent_daemon_wrong() {
    local c=${CDIR:-$RDIR/cases/course}
    mkdir -p "$c"
    CDIR=$c verdict "$1" "$2"
}
# MESSAGE: the rig failed outside a case's own steps (cut-record.sh's
# rig_error): the check judges the cases done so far, with the error in
# DIR/rig-error.tsv; its status is the recording's (1 where it found a
# verdict, else 2).
agent_judged() {
    local rc=0
    printf '%s\t%s\n' "$(basename "${CDIR:-none}")" "$1" > "$RDIR/rig-error.tsv"
    [ "$CHECK" != none ] || return 2
    agent_quiet
    # shellcheck disable=SC2086 # CHECK is a command line
    $CHECK "$NAME" "$RDIR" "$RDIR/requests.tsv" - $((VDEV_MB << 20)) || rc=$?
    printf '%s\tchecked\texit %s\n' "$NAME" "$rc" >> "$OUT/recordings.tsv"
    return "$rc"
}
# Before the check after the rig's error: the agent killed, the store
# thawed, the writer stopped and the pool exported, as agent_end does, so
# that node B's stages and the controls are judged (D7 of #159's review:
# node B refuses while node A has the pool). Each step is bounded and
# none is an error of its own: the error is said already, and a pool
# still imported leaves node B's stages to the check's own error.
agent_quiet() {
    local _i
    if [ -n "$AGENT_PID" ]; then
        kill -CONT "$AGENT_PID" 2>/dev/null || true
        kill -9 "$AGENT_PID" 2>/dev/null || true
        for _i in $(seq 1 20); do kill -0 "$AGENT_PID" 2>/dev/null || break; sleep 0.25; done
        kill -0 "$AGENT_PID" 2>/dev/null || wait "$AGENT_PID" 2>/dev/null || true
        AGENT_PID=""
    fi
    if [ -n "$S3_IS_DOWN" ]; then
        bash -c "$S3_UP" >> "$RDIR/s3.log" 2>&1 && S3_IS_DOWN=""
        rm -f "$WORK/watch.pause"
    fi
    if [ -n "$WRITER" ]; then
        touch "$WORK/writer.stop"
        for _i in $(seq 1 120); do kill -0 "$WRITER" 2>/dev/null || break; sleep 0.5; done
        kill -9 "$WRITER" 2>/dev/null || true
        kill -0 "$WRITER" 2>/dev/null || wait "$WRITER" 2>/dev/null || true
        WRITER=""
    fi
    session_end
    # A daemon that had ended with no kill of the rig's is a verdict, as
    # in recover (G5 of #163's review). In a dhook case, until the hook's
    # end is recorded (kill.tsv), the rig's own error about that end
    # comes first; the instance served after it has no hook armed.
    if [ -n "$SERVE_PID" ] && ! kill -0 "$SERVE_PID" 2>/dev/null; then
        local k=$SERVE_K st=unknown
        if serve_end 5; then st=$SERVE_RC; fi
        SERVE_PID=""
        if [ -n "$CDIR" ] && [ "$SCEN" = dhook ] && [ ! -e "$CDIR/kill.tsv" ]; then
            log "$NAME: skyzfsd serve $k had ended (status $st) in a dhook case, after the rig's own error"
        else
            agent_daemon_wrong daemon_serving "skyzfsd serve $k had ended (status $st) with no kill of the rig's, seen after the rig's error: $(tail -n 2 "$(serve_log "$k")" 2>/dev/null)"
        fi
    fi
    # With no daemon (the error came while the rig had it down, as in a
    # restart case), the export cannot write the pool's labels and waits
    # its whole limit, leaving a command behind on a device with no
    # server (run 45828678468). Said, and node B's stages are left to
    # the check's own error, as when the export fails.
    if [ -z "$SERVE_PID" ] || ! kill -0 "$SERVE_PID" 2>/dev/null; then
        log "$NAME: $POOL not exported for the check: no daemon serves its device"
        return 0
    fi
    if lim 120 zpool export "$POOL" > "$RDIR/judged-export.log" 2>&1; then
        log "$NAME: $POOL exported for the check after the rig's error"
    else
        log "$NAME: $POOL did not export for the check: $(tail -n 2 "$RDIR/judged-export.log")"
    fi
}
agent_end() {
    writer_off
    export_pool
}
rec_aplain() {
    local p points k=0 i n
    agent_begin
    # The traced publish's status is the check's (pin_trace_first_run):
    # the points come from its trace either way.
    acase trace plain
    i=${AGENT_SHARD%/*}; n=${AGENT_SHARD#*/}
    # A trace that gives no point runs no A case: said, as the check's
    # error, as is a trace the check cannot read.
    if ! points=${AGENT_POINTS:-$(acheck points "$RDIR/cases/trace/trace.tsv" 2> "$RDIR/points.err")}; then
        printf 'trace\t%s\n' "$(tr '\t\n' '  ' < "$RDIR/points.err")" >> "$RDIR/points-error.tsv"
        points=""
    elif [ -z "$points" ]; then
        printf 'trace\tthe trace gives no kill point: no A case ran\n' >> "$RDIR/points-error.tsv"
    fi
    for p in $points; do
        k=$((k + 1))
        [ $(((k - 1) % n)) = $((i - 1)) ] || continue
        acase "A-${p//:/-}" plain "$p"
    done
    agent_end
}
rec_adaemon() {
    local h p
    agent_begin
    acase w0 plain
    acase d1 restart
    for h in $AGENT_HOOKS; do acase "d-${h%%:*}" dhook "" "$h"; done
    acase d6 late
    for p in $AGENT_BOTH; do acase "b-${p//:/-}" both "$p"; done
    agent_end
}
rec_arare() {
    local s p pts
    agent_begin
    acase trace plain
    for s in $AGENT_RARE; do
        acase "$s" "$s"
        # A scenario that adds no point to the plain trace's runs no K
        # case: said, as the check's error (K-destroy ran none on 1a40bfc
        # and the summary did not say so).
        if ! pts=$(acheck points "$RDIR/cases/$s/trace.tsv" "$RDIR/cases/trace/trace.tsv" 2> "$RDIR/points.err"); then
            printf '%s\t%s\n' "$s" "$(tr '\t\n' '  ' < "$RDIR/points.err")" >> "$RDIR/points-error.tsv"
            pts=""
        elif [ -z "$pts" ]; then
            printf '%s\tits trace adds no kill point to the plain trace'"'"'s: no K-%s case ran\n' "$s" "$s" >> "$RDIR/points-error.tsv"
        fi
        for p in $pts; do acase "K-$s-${p//:/-}" "$s" "$p"; done
    done
    acase gone gone holding:before:1
    acase unmarked unmarked
    agent_end
}
rec_arun() {
    agent_begin
    acase R-run run
    acase R-kill runkill intent:after:2
    acase R-twice runtwice
    acase R-held runheld
    acase R-corrupt runbad "" corrupt
    acase R-volume runbad "" volume
    acase R-rewind runrewind
    agent_end
}

# The failure course (afail, part f; M2.7c, #76): `skyzfs-agent run` on
# its default schedule (no --every-s: 60 s) on a volume that fails while
# it is served. The test build's SIGUSR1 fails the volume: the daemon
# writes the failure note beside its socket ($(vrun)/failed), deletes
# the device and ends with 3. Every line the run prints, on standard
# output and on standard error, is stamped with its arrival on the rig's
# clock (astamp). Two cases, in this order, on one volume:
#
# - U: the run's first round with the pool imported; then the pool
#   exported, and the volume failed. Nothing holds the device, and the
#   daemon's exclusive open that finds no holder proves none
#   (skyzfs_ublk_sys::control::client_holds): the note says
#   held=unknown, which makes the volume lost. The run's wait ends, its
#   next round takes the step at once, finds the pool not imported
#   (verdict failed) and goes on. A start without --acknowledge-failure
#   is refused (4) and settles the note to held=no: the round at the
#   next tick says that no client held the device and touches nothing.
#   A start that acknowledges the failure serves the volume afresh (a
#   new attachment), the pool is imported on its device, and the round
#   at the tick after ends the run with 12.
# - W: a writer on the pool and the store frozen; a new run (traced)
#   whose first round begins a publish that waits for its cut, the
#   agent's assist syncs going (FAIL_AFTER s after `asked:after`, past
#   the agent's ASSIST_AFTER of 10 s); the volume fails with the pool
#   imported on the device (held=yes). The run's wait ends, and its next
#   round reopens the pool and looks at it until it reads SUSPENDED:
#   exit 11. Then `run` again in the same directory (again): 11 again,
#   and no reopen.
#
# The pool is left suspended, with no device under it: the course has
# no stop of a daemon at its end (DAEMON_GONE), the cleanup does not
# export the pool (POOL_LOST), and afail is the last recording of its
# job. Its files, in DIR/cases/U and DIR/cases/W:
#
# - LABEL.{out,err,rc} (publish, again): the run; LABEL.out.tsv and
#   LABEL.err.tsv the arrival of each line (its number, the rig's
#   realtime), LABEL.started-at and LABEL.ended-at the rig's times of
#   its start and of the end it saw;
# - fail-at: the signal's time on the rig's realtime and on
#   CLOCK_MONOTONIC in ns (the trace's clock); failed.rc: the daemon's
#   status after it (`timeout` where it had not ended in 60 s);
# - note.tsv: the note as the rig found it after each step (LABEL, its
#   mtime on the realtime clock, its size; `none -` where there was
#   none): `failed` after the daemon's end, `refused` after the start
#   that was refused (U);
# - refused.{out,err,rc}, refused-at (U): the start without
#   --acknowledge-failure, and when it ended; served-at (U): the
#   acknowledged start served and the pool imported on it;
# - pool-state.txt, pool-state-again.txt (W): the pool's kstat state
#   after each run; zpool-status.txt, zpool-events.txt (W, bounded);
# - trace.tsv (W): the first run's points.
#
# DIR/zed.tsv says whether zed runs (CUT_AGENT_ZED: on or off, the
# workflow's choice; the check holds the two against each other), and
# DIR/zed.log is zed's journal over the course, where it runs.
# agent_fail_check.py judges the course.
AGENT_ZED=${CUT_AGENT_ZED:-off}
[[ $AGENT_ZED =~ ^(on|off)$ ]] || { echo "CUT_AGENT_ZED is on or off, not $AGENT_ZED" >&2; exit 64; }
FAIL_AFTER=12
FAILED_PID=""
# LABEL: the lines CDIR/LABEL.out and LABEL.err hold now, each stamped
# once, on arrival, in LABEL.out.tsv and LABEL.err.tsv. A line is
# counted once its newline is written.
astamp_now() {
    local f k have
    for f in out err; do
        k=$(wc -l 2>/dev/null < "$CDIR/$1.$f.tsv") || k=0
        have=$(wc -l 2>/dev/null < "$CDIR/$1.$f") || have=0
        while [ "$k" -lt "$have" ]; do
            k=$((k + 1))
            printf '%s\t%s\n' "$k" "$EPOCHREALTIME" >> "$CDIR/$1.$f.tsv"
        done
    done
}
# LABEL SECONDS [ERE]: the run in the background (AGENT_PID), its lines
# stamped every 0.05 s, until a line of its output or errors matches
# ERE (0), it ends (1; the time it was seen to end in LABEL.ended-at),
# or SECONDS pass (2). A line that came between the stamps and the
# match is stamped before the return, so that the line matched always
# has its stamp from before whatever the caller does next.
astamp() {
    local t0=$SECONDS
    while :; do
        astamp_now "$1"
        if [ -n "${3:-}" ] && grep -Eq -- "$3" "$CDIR/$1.out" "$CDIR/$1.err" 2>/dev/null; then
            astamp_now "$1"
            return 0
        fi
        if ! kill -0 "$AGENT_PID" 2>/dev/null; then
            echo "$EPOCHREALTIME" > "$CDIR/$1.ended-at"
            astamp_now "$1"
            return 1
        fi
        [ $((SECONDS - t0)) -lt "$2" ] || return 2
        sleep 0.05
    done
}
# LABEL SECONDS: the run's lines stamped until it ends, for up to
# SECONDS; then its status (await_agent: one still running is killed,
# and that is a verdict).
afinish() {
    astamp "$1" "$2" || true
    await_agent "$1" 1
}
# The volume failed by the test build's SIGUSR1 to the daemon serving
# it, the signal's time on both clocks in CDIR/fail-at. From then on the
# daemon is FAILED_PID and not SERVE_PID, so that no step of the rig's
# stops it or takes its end for a verdict.
fail_now() {
    python3 -I -c 'import os, signal, sys, time
r, m = time.time(), time.monotonic_ns()
os.kill(int(sys.argv[1]), signal.SIGUSR1)
print(f"{r:.6f}\t{m}")' "$SERVE_PID" > "$CDIR/fail-at" 2>&1 \
        || rig_error "SIGUSR1 to skyzfsd serve $SERVE_K failed: $(tail -n 1 "$CDIR/fail-at")"
    FAILED_PID=$SERVE_PID SERVE_PID=""
    log "$NAME: case $(basename "$CDIR"): the volume failed (SIGUSR1 to skyzfsd serve $SERVE_K)"
}
# The failed daemon's end, for up to 60 s: its status in CDIR/failed.rc.
# One that has not ended is a verdict (`timeout`).
failed_end() {
    local _i rc=0
    for _i in $(seq 1 240); do kill -0 "$FAILED_PID" 2>/dev/null || break; sleep 0.25; done
    if kill -0 "$FAILED_PID" 2>/dev/null; then
        echo timeout > "$CDIR/failed.rc"
        verdict daemon_failed "skyzfsd serve $SERVE_K had not ended 60 s after SIGUSR1: $(tail -n 2 "$(serve_log "$SERVE_K")")"
        return 0
    fi
    wait "$FAILED_PID" 2>/dev/null || rc=$?
    FAILED_PID=""
    echo "$rc" > "$CDIR/failed.rc"
}
# LABEL: the failure note as it is now, a line of CDIR/note.tsv.
note_stat() {
    local f
    f=$(vrun)/failed
    if [ -e "$f" ]; then
        printf '%s\t%s\t%s\n' "$1" "$(stat -c %.9Y "$f")" "$(stat -c %s "$f")" >> "$CDIR/note.tsv"
    else
        printf '%s\tnone\t-\n' "$1" >> "$CDIR/note.tsv"
    fi
}
# ID SCENARIO: a case of the failure course (no recover and no settle
# after it: the volume is not served as before).
fcase() {
    CDIR=$RDIR/cases/$1; ADIR=$CDIR/agent; SCEN=$2; KILL=""
    rm -rf "${CDIR:?}"; mkdir -p "$ADIR"
    "fail_$2" --trace-points "$CDIR/trace.tsv"
    printf '%s\t%s\t-\t%s\t-\n' "$1" "$2" "$(cat "$CDIR/publish.rc" 2>/dev/null || echo -)" >> "$RDIR/cases.tsv"
    log "$NAME: case $1 ($2): run ended $(cat "$CDIR/publish.rc" 2>/dev/null || echo -)"
    CDIR=""
}
# The run in the background on the default schedule, its start stamped.
frun() {
    local label=$1
    shift
    abg "$label" run "$@"
    echo "$EPOCHREALTIME" > "$CDIR/$label.started-at"
}
# U: lost while the pool is not imported, then held=no, then a new
# attachment (12). The pool is exported only after a round 1 that left
# 20 s or more to its tick, so that no round works while the rig
# exports.
fail_unknown() {
    local left rc=0
    frun publish "$@"
    if ! astamp publish 180 '^round 1 '; then
        verdict rounds "no round line 180 s after the run's start: $(tail -n 2 "$CDIR/publish.err")"
        afinish publish 1
        return 0
    fi
    left=$(awk '/^round 1 / { for (i = 3; i <= NF; i++) { split($i, f, "="); v[f[1]] = f[2] }
        print 60000 - v["took_ms"] - v["late_ms"]; exit }' "$CDIR/publish.out")
    if [ "${left:-0}" -lt 20000 ]; then
        case_error "round 1 left ${left:-0} ms to its tick, not 20 s or more: no time to export the pool between two rounds"
        afinish publish 1
        return 0
    fi
    writer_off
    export_pool
    udevadm settle -t 30 > /dev/null 2>&1 || true
    fail_now
    astamp publish 90 '^lost ' || true
    failed_end
    note_stat failed
    # Without the acknowledgement and without the request log: a refused
    # start is no instance of the recording.
    timeout 60 "$SKYZFSD" serve --dir "$VOL" --run-dir "$RUN" "${AGENT_SERVE[@]}" > "$CDIR/refused.out" \
        2> "$CDIR/refused.err" || rc=$?
    echo "$rc" > "$CDIR/refused.rc"
    echo "$EPOCHREALTIME" > "$CDIR/refused-at"
    note_stat refused
    astamp publish 150 '^round 3 ' || true
    SERVE_K=$((SERVE_K + 1))
    SERVE_ARGS=("${AGENT_SERVE[@]}")
    serve_start "$SERVE_K" acknowledged
    import_pool
    echo "$EPOCHREALTIME" > "$CDIR/served-at"
    note_stat served
    writer_on
    afinish publish 150
}
# W: lost while a publish waits for its cut, the pool imported and the
# device held: 11, and 11 again.
fail_wait() {
    freeze
    frun publish "$@"
    if ! wait_point asked after 120; then
        verdict asked "the run did not reach asked:after in 120 s: $(tail -n 2 "$CDIR/publish.err")"
        afinish publish 1
        return 0
    fi
    local st=0
    astamp publish "$FAIL_AFTER" || st=$?
    if [ "$st" != 2 ]; then
        verdict publish_waiting "the run ended within $FAIL_AFTER s of asked:after, before the failure: $(tail -n 2 "$CDIR/publish.err")"
        afinish publish 1
        return 0
    fi
    fail_now
    afinish publish 90
    failed_end
    note_stat failed
    cat "/proc/spl/kstat/zfs/$POOL/state" > "$CDIR/pool-state.txt" 2>&1 || true
    frun again
    afinish again 90
    cat "/proc/spl/kstat/zfs/$POOL/state" > "$CDIR/pool-state-again.txt" 2>&1 || true
    # Bounded, and into files: a command on a suspended pool may never
    # return (tests/lib/lim.sh).
    lim 30 zpool status -v "$POOL" > "$CDIR/zpool-status.txt" 2>&1 || true
    lim 30 bash -c 'zpool events -H 2>&1 | head -n 4000' > "$CDIR/zpool-events.txt" 2>&1 || true
}
# Whether zed runs, against the workflow's choice; zed's journal over
# the course at its end.
afail_zed() {
    local pid
    pid=$(pgrep -x zed | head -n 1) || pid=""
    printf 'want\t%s\nruns\t%s\npid\t%s\n' "$AGENT_ZED" "$([ -n "$pid" ] && echo on || echo off)" "${pid:--}" > "$RDIR/zed.tsv"
    echo "$EPOCHREALTIME" > "$RDIR/zed-since"
}
afail_zed_log() {
    local since
    since=$(cut -d. -f1 "$RDIR/zed-since")
    # shellcheck disable=SC2016 # the inner shell expands $1
    lim 30 bash -c 'journalctl -u zfs-zed --no-pager -o short-precise --since "@$1" 2>&1 | head -n 2000' _ "$since" \
        > "$RDIR/zed.log" 2>&1 || true
}
# The course's end: the writer killed and not waited for (it may be
# stuck in the suspended pool), the store thawed, the watcher stopped;
# no daemon to stop and no pool to export.
afail_end() {
    local _i
    POOL_LOST=1 DAEMON_GONE=1
    if [ -n "$WRITER" ]; then
        touch "$WORK/writer.stop"
        kill -9 "$WRITER" 2>/dev/null || true
        WRITER=""
    fi
    [ -z "$S3_IS_DOWN" ] || thaw
    if [ -n "$WATCH_PID" ]; then
        touch "$WORK/watch.stop"
        for _i in $(seq 1 60); do kill -0 "$WATCH_PID" 2>/dev/null || break; sleep 1; done
        kill "$WATCH_PID" 2>/dev/null || true
        wait "$WATCH_PID" 2>/dev/null || true
        WATCH_PID=""
    fi
    afail_zed_log
}
rec_afail() {
    afail_zed
    agent_begin
    fcase U unknown
    fcase W wait
    afail_end
}
