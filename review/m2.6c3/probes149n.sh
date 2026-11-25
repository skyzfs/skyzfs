#!/usr/bin/env bash
# Command-line probes of the next look at #149. Usage: probes149n.sh BINARY
# (a build of skyzfs-agent at the head). Prints each probe's exit code
# and its output's first lines.
set -u
A=$1; T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
probe() { echo "== $*"; "$@" >"$T/o" 2>"$T/e"; echo "exit $?"; head -c 400 "$T/o" | head -3; head -c 400 "$T/e" | head -3; }
mkdir "$T/d"; : > "$T/file"
probe "$A" record --dir "$T/missing"
probe "$A" record --dir "$T/file"
probe "$A" record --dir "$T/d"
probe "$A" record --dir "$T/d" --help
probe "$A" record --dir --help
probe "$A" record --dir -h
probe "$A" record --dir --d
probe "$A" record --dir=--d
COMMON=(--volume 0f0f0f0f-0f0f-0f0f-0f0f-0f0f0f0f0f0f --run "$T/run" --device /dev/null --pool tank --dir "$T/d")
probe "$A" recover "${COMMON[@]}" --name --give-up --retry
probe "$A" recover "${COMMON[@]}" --name -h --give-up
probe "$A" publish --volume --run "$T/run" --device /dev/null --pool tank --dir "$T/d"
probe "$A" publish -h
echo "== record into a closed pipe"; ( "$A" record --dir "$T/d" 2>"$T/e"; echo "exit $?" >"$T/x" ) | true; cat "$T/x"; grep -c panicked "$T/e"
echo "== --help into head -1"; ( "$A" --help 2>"$T/e"; echo "exit $?" >"$T/x" ) | head -1; cat "$T/x"; grep -c panicked "$T/e"
