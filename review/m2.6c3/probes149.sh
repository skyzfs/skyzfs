#!/bin/bash
# Command-line probes of the review of #149 at af50814, on the release
# binary built without the test hooks (gates149.sh builds it). Each line
# prints the arguments, the exit code and the first line of each stream.
#   BIN=<target>/release/skyzfs-agent bash probes149.sh
set -u
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/vol" "$T/run" "$T/dir" "$T/dir-with-a-dir/record" "$T/dir-corrupt"
printf 'skyzfs-remote 1\nendpoint http://e\nbucket b\nvolume v\nid 01JDZQ0000000000000000000A\n' > "$T/vol/remote"
printf 'not a record' > "$T/dir-corrupt/record"
COMMON=(--volume "$T/vol" --run "$T/run" --device /dev/ublkb0 --pool tank --dir "$T/dir")
probe() {
  "$BIN" "$@" > "$T/out" 2> "$T/err"
  rc=$?
  printf '%s\n  exit %s | out: %s | err: %s\n' "$*" "$rc" "$(head -1 "$T/out")" "$(head -1 "$T/err")" \
    | sed -e "s#$T#<tmp>#g"
}
echo "== usage"
probe
probe --help
probe publish --help
probe publish "${COMMON[@]}" --help
probe frobnicate
probe publish "${COMMON[@]}" --nope 1
probe publish "${COMMON[@]}" --pool tank2
probe publish "${COMMON[@]}" --limit-s
probe publish "${COMMON[@]}" word
probe publish "${COMMON[@]}" --limit-s 0
probe publish "${COMMON[@]}" --limit-s 1.5
probe publish "${COMMON[@]}" --expiry-s 100000000
probe settle "${COMMON[@]}" --expiry-s 60
probe recover "${COMMON[@]}" --name t
probe recover "${COMMON[@]}" --name t --give-up --retry
probe recover "${COMMON[@]}" --name t --retry=yes
probe record
probe record --dir "$T/dir" --pool tank
echo "== a value option takes the next option as its value (S3)"
probe recover "${COMMON[@]}" --name --give-up --retry
probe publish --volume --run "$T/run" --device /dev/ublkb0 --pool tank --dir "$T/dir"
echo "== the test hooks, refused without the feature"
for h in '--kill-at take:before' '--kill-at=take:before' '--trace-points f' '--trace-points=f' \
         '--assist-after-s 5' '--assist-after-s=5' '--age-max-s 5' '--age-max-s=5'; do
  # shellcheck disable=SC2086
  probe publish "${COMMON[@]}" $h
done
probe record --dir "$T/dir" --kill-at take:before
echo "== an argument not UTF-8"
probe publish "${COMMON[@]}" --name "$(printf 'a\377')"
echo "== record"
probe record --dir "$T/dir"
probe record --dir "$T/no-such-directory"
probe record --dir "$T/dir-with-a-dir"
probe record --dir "$T/dir-corrupt"
echo "== record into a closed pipe (S4; a record this small may fit the pipe)"
"$BIN" record --dir "$T/dir" | true; echo "  exit ${PIPESTATUS[0]}"
echo "== volume's remote missing"
probe settle --volume "$T/no-vol" --run "$T/run" --device /dev/ublkb0 --pool tank --dir "$T/dir"
