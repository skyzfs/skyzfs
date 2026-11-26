# shellcheck shell=bash
# The agent rig's courses (M2.T5 stage 1; design §5.2 and §5.4, the
# plan of M2.6): the real agent (bins/skyzfs-agent, built with its test
# hooks) publishes snapshots of a real pool whose device the real daemon
# serves with its uploads (cut-record.sh's CUT_UPLOAD=1), and the rig
# loses the agent, the daemon or both at each step of a publish. Sourced
# by cut-record.sh, which calls the courses by name (rec_aplain,
# rec_adaemon, rec_arare); agent_check.py (their CUT_CHECK) judges what
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
# - holds.txt: every hold on the pool's snapshots after the last run,
#   read before the rig releases any it left (holds-released.txt);
# - manifests.txt: the files left in the agent's manifests directory;
# - assist.txt: `skyzfs:assist` on the pool and every descendant, with
#   its source;
# - names.txt: T, the case's own publish (the last name its first run's
#   trace gives; none when it was killed before it had one).
#
# The case's line in DIR/cases.tsv: ID, scenario, kill point (`-` for
# none), the first run's status, T (`-`). Each name published in the
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
#   rig's (sweep-ID): the case's publish must release it.
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
#   reached that the plain trace did not (by effect, side and reach, the
#   first three reaches of each); then gone and unmarked.
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
# in CDIR/LABEL.rc. One that has not ended is killed, and that is the
# rig's error.
await_agent() {
    local _i rc=0
    for _i in $(seq 1 $(($2 * 4))); do kill -0 "$AGENT_PID" 2>/dev/null || break; sleep 0.25; done
    if kill -0 "$AGENT_PID" 2>/dev/null; then
        kill -9 "$AGENT_PID" 2>/dev/null; wait "$AGENT_PID" 2>/dev/null || true; AGENT_PID=""
        rig_error "the agent's $1 in case $(basename "$CDIR") had not ended $2 s on: $(tail -n 2 "$CDIR/$1.err")"
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
    kill -TERM "$SERVE_PID" 2>/dev/null || true
    serve_end 60 || rig_error "skyzfsd serve did not end 60 s after SIGTERM"
    [ "$SERVE_RC" = 0 ] || rig_error "skyzfsd serve ended with $SERVE_RC on SIGTERM: $(tail -n 2 "$RDIR"/serve*.log)"
    serve_again "$@"
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
    asettle
}

scen_plain() { shift; acmd publish publish --limit-s 600 "$@"; }
scen_restart() {
    shift
    freeze
    abg publish publish --limit-s 600 "$@"
    if wait_point asked after 60; then
        kill_serve
        thaw
        local t; t=$(case_name)
        snaprow "$t" between.txt || true
        serve_again
    else
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
        { zfs release "$HOLD_TAG" "$POOL@$t" && zfs destroy "$POOL@$t"; } > "$CDIR/destroy.log" 2>&1 \
            || rig_error "releasing and destroying $POOL@$t failed: $(tail -n 2 "$CDIR/destroy.log")"
        mark "destroyed-$id"
        frozen_until_withdrawn
    fi
    thaw
    await_agent publish 300
}
# Frozen until the agent's withdrawal is answered, so that no cut after
# the loss can carry T; then its look at snap/T needs the store (run
# 45745575500: frozen, the agent looked for 300 s). An agent killed
# before it withdrew: its next start withdraws, the store still frozen
# (a settle of 30 s, which ends unsettled while it cannot look).
frozen_until_withdrawn() {
    wait_point withdraw after 120 && return 0
    ! kill -0 "$AGENT_PID" 2>/dev/null \
        || rig_error "case $(basename "$CDIR"): the agent did not withdraw in 120 s"
    acmd settle-frozen settle --limit-s 30 "${CASE_AGE[@]}"
}
scen_tooold() {
    shift
    CASE_AGE=(--age-max-s 8)
    freeze
    abg publish publish --limit-s 300 "${CASE_AGE[@]}" "$@"
    frozen_until_withdrawn
    thaw
    await_agent publish 300
}
scen_sweep() {
    local id
    shift
    id=$(basename "$CDIR")
    # Its status is the check's to judge (pin_ID_first_publish).
    acmd first publish --limit-s 600
    z 60 zfs snapshot "$POOL@sweep-$id"
    z 60 zfs hold "$HOLD_TAG" "$POOL@sweep-$id"
    acmd publish publish --limit-s 600 "$@"
    zfs holds -H "$POOL@sweep-$id" > "$CDIR/stray-holds.txt"
}
scen_gone() {
    local t
    shift
    acmd publish publish --limit-s 600 "$@"
    t=$(case_name)
    [ -n "$t" ] || rig_error "case $(basename "$CDIR"): no name in the trace before the kill"
    zfs destroy "$POOL@$t" > "$CDIR/destroy.log" 2>&1 || rig_error "destroying $POOL@$t failed: $(tail -n 2 "$CDIR/destroy.log")"
}
scen_unmarked() {
    local id
    shift
    id=$(basename "$CDIR")
    z 60 zfs snapshot "$POOL@stray-$id"
    z 60 zfs hold "$HOLD_TAG" "$POOL@stray-$id"
    acmd publish publish --limit-s 120 "$@"
    zfs holds -H "$POOL@stray-$id" > "$CDIR/stray-holds.txt"
    # The rig's own hold: let go, so that the retry can publish.
    z 60 zfs release "$HOLD_TAG" "$POOL@stray-$id"
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
        wait_point asked after 60 || { thaw; rig_error "case $(basename "$CDIR"): the agent did not reach asked:after in 60 s"; }
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
        serve_end 5 || rig_error "the daemon's pid went away and serve_end did not see it end"
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
    wait_point asked after 60 || { thaw; rig_error "case $(basename "$CDIR"): the agent did not reach asked:after in 60 s"; }
    kill -STOP "$AGENT_PID"
    mark "stopped-$(basename "$CDIR")"
    adump record-stopped
    thaw
    t=$(case_name)
    local _i
    for _i in $(seq 1 240); do snaprow "$t" landed.txt && break; sleep 0.5; done
    snaprow "$t" landed.txt || { kill -CONT "$AGENT_PID"; rig_error "case $(basename "$CDIR"): snap/$t had not landed 120 s after the thaw"; }
    kill_serve
    serve_again
    mark "continued-$(basename "$CDIR")"
    kill -CONT "$AGENT_PID"
    await_agent publish 700
}
scen_both() {
    local rc=0
    shift
    abg publish publish --limit-s 600 "$@"
    await_agent publish 300
    # As kill_serve, with the daemon's status kept: 137 where the SIGKILL
    # ended it, anything else where it had ended by itself.
    kill -9 "$SERVE_PID" 2>/dev/null || true
    wait "$SERVE_PID" 2>/dev/null || rc=$?
    SERVE_PID=""
    printf 'killed_rc\t%s\n' "$rc" > "$CDIR/both-kill.tsv"
    serve_again
}

# The courses.
agent_begin() {
    SERVE_K=1
    create_pool
    load 1
    writer_on
    : > "$RDIR/cases.tsv"; : > "$RDIR/published.tsv"; : > "$RDIR/published-gone.tsv"
    [ -S "$(vrun)/control" ] || rig_error "no socket control in $(vrun): under $RUN are $(find "$RUN" -maxdepth 2 | tr '\n' ' ')"
    printf 'run-dir\t%s\nagent-run\t%s\n' "$RUN" "$(vrun)" > "$RDIR/agent-run.tsv"
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
    points=${AGENT_POINTS:-$(acheck points "$RDIR/cases/trace/trace.tsv")} || rig_error "no points from the trace"
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
        pts=$(acheck points "$RDIR/cases/$s/trace.tsv" "$RDIR/cases/trace/trace.tsv") \
            || rig_error "no points from $s's trace"
        for p in $pts; do
            # A point reached again and again (a look while the store is
            # frozen) is killed at its first three reaches.
            [ "${p##*:}" -le 3 ] || continue
            acase "K-$s-${p//:/-}" "$s" "$p"
        done
    done
    acase gone gone holding:before:1
    acase unmarked unmarked
    agent_end
}
