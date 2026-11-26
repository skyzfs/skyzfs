#!/bin/bash
# Tests of the agent rig's courses (agent-record.sh, sourced by
# cut-record.sh) with no kernel, ZFS, daemon or agent: the validation of
# its inputs, the shards of aplain's points, and the trace helpers.
# Run: bash tests/kernel/test_agent_record.sh
# The stand-ins' bodies are single-quoted: they expand in their own shell.
# shellcheck disable=SC2016
HERE=$(cd "$(dirname "$0")" && pwd)
SCRIPT=$HERE/agent-record.sh
if ! T=$(mktemp -d) || [ -z "$T" ] || [ ! -d "$T" ]; then echo "no temporary directory"; exit 2; fi
trap 'rm -rf "$T"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "skipped: not root and no sudo" ]; then echo "skip $1: $2"; return; fi
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}
# A function as the script has it: from `NAME() {` to the `}` that ends
# it, or the one line where it ends on that line.
fn() {
    awk -v h="$1() {" '!on && index($0, h) == 1 { on = 1; print; if ($0 ~ /}$/) exit; next }
        on { print; if ($0 == "}") exit }' "$SCRIPT"
}
for f in rec_aplain point_reached wait_point case_name; do
    is "$f is in the script" "$([ -n "$(fn "$f")" ] && echo yes)" yes
done

# The inputs: each refused before anything runs, with 64 and its reason.
# cut-record.sh refuses first to run without root: on a runner the tests
# are not root, and its sudo asks for no password.
# Where neither holds, these six are skipped and say so.
as_root=()
[ "$(id -u)" = 0 ] || as_root=(sudo -n)
refused() { # ENV... -> the status and the last line
    if [ ${#as_root[@]} -gt 0 ] && ! sudo -n true 2>/dev/null; then echo "skipped: not root and no sudo"; return; fi
    "${as_root[@]}" env "$@" CUT_SERVER=daemon CUT_UPLOAD=1 CUT_SKYZFSD=/bin/true CUT_NODEB=/bin/true SKZ_S3_ENDPOINT=x \
        SKZ_S3_BUCKET=b bash "$HERE/cut-record.sh" "$T/out" "$T/none" aplain > "$T/o" 2>&1
    echo "$? $(tail -n 1 "$T/o")"
}
is "an agent course with no agent" "$(refused CUT_AGENT=)" \
    "64 aplain needs CUT_AGENT, the agent's binary built with its test hooks"
is "an agent course with no way to freeze the store" "$(refused CUT_AGENT=/bin/true)" \
    "64 aplain needs CUT_S3_DOWN and CUT_S3_UP"
is "a shard past its count" "$(refused CUT_AGENT=/bin/true CUT_S3_DOWN=x CUT_S3_UP=y CUT_AGENT_SHARD=3/2)" \
    "64 CUT_AGENT_SHARD is K/N with 1 <= K <= N, not 3/2"
is "a shard of none" "$(refused CUT_AGENT=/bin/true CUT_S3_DOWN=x CUT_S3_UP=y CUT_AGENT_SHARD=0/2)" \
    "64 CUT_AGENT_SHARD is K/N with 1 <= K <= N, not 0/2"
is "a point that is not one" "$(refused CUT_AGENT=/bin/true CUT_S3_DOWN=x CUT_S3_UP=y CUT_AGENT_POINTS=ask:during:1)" \
    "64 CUT_AGENT_POINTS: not EFFECT:SIDE:N: ask:during:1"
is "a point's reach of 0" "$(refused CUT_AGENT=/bin/true CUT_S3_DOWN=x CUT_S3_UP=y CUT_AGENT_POINTS=ask:after:0)" \
    "64 CUT_AGENT_POINTS: not EFFECT:SIDE:N: ask:after:0"

# aplain's cases for a shard: the traced publish first, then every Nth
# point from the Kth, in the trace's order.
cases() { # SHARD [POINTS]
    # shellcheck disable=SC2034,SC2329
    (
        RDIR=$T/r; mkdir -p "$RDIR/cases/trace"; echo 0 > "$RDIR/cases/trace/publish.rc"
        AGENT_SHARD=$1 AGENT_POINTS=${2:-}
        agent_begin() { :; }; agent_end() { :; }
        rig_error() { echo "rig error: $*"; exit 2; }
        acheck() { printf 'ask:before:1\nask:after:1\nasked:after:1\ndone:before:1\ndone:before:2\n'; }
        acase() { printf '%s ' "$1${3:+@$3}"; }
        eval "$(fn rec_aplain)"
        rec_aplain
    )
}
is "shard 1/1: every point" "$(cases 1/1)" \
    "trace A-ask-before-1@ask:before:1 A-ask-after-1@ask:after:1 A-asked-after-1@asked:after:1 A-done-before-1@done:before:1 A-done-before-2@done:before:2 "
is "shard 1/2" "$(cases 1/2)" "trace A-ask-before-1@ask:before:1 A-asked-after-1@asked:after:1 A-done-before-2@done:before:2 "
is "shard 2/2" "$(cases 2/2)" "trace A-ask-after-1@ask:after:1 A-done-before-1@done:before:1 "
is "the points named in place of the trace's" "$(cases 1/1 'hold:after:1 look:before:2')" \
    "trace A-hold-after-1@hold:after:1 A-look-before-2@look:before:2 "

# The trace helpers, on a stand-in agent.
CDIR=$T/c; mkdir -p "$CDIR"
eval "$(fn point_reached)"; eval "$(fn wait_point)"; eval "$(fn case_name)"
printf '1\tenrol\tbefore\t\n2\tintent\tafter\tt1\n3\tsweep\tbefore\tstray\n' > "$CDIR/trace.tsv"
is "case_name: the last publish named, not a sweep's snapshot" "$(case_name)" t1
printf '4\tintent\tafter\tt2\n' >> "$CDIR/trace.tsv"
is "case_name: the publish's next name" "$(case_name)" t2
: > "$CDIR/trace.tsv"
is "case_name: no name" "$(case_name)" ""
sleep 30 & AGENT_PID=$!
(sleep 0.3; printf '5\tasked\tafter\tt\n' >> "$CDIR/trace.tsv") &
is "wait_point: the point reached while the agent runs" "$(wait_point asked after 20 && echo reached)" reached
kill "$AGENT_PID"; wait "$AGENT_PID" 2>/dev/null
t0=$SECONDS
is "wait_point: the agent ended before the point" "$(wait_point "done" before 20 || echo not)" not
is "wait_point: an agent that ended is not waited for to the limit" "$((SECONDS - t0 < 5))" 1
sleep 30 & AGENT_PID=$!
is "wait_point: the limit" "$(wait_point "done" before 1 || echo "not, $?")" "not, 1"
kill "$AGENT_PID"; wait "$AGENT_PID" 2>/dev/null

[ "$fails" = 0 ] && echo "all passed"
exit $((fails > 0))
