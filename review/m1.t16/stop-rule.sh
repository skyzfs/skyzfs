#!/usr/bin/env bash
# Runs the two new steps of topology-bench.yml ("Barriers on the work disk",
# "Work disk mount at the end") as GitHub's bash would (-eo pipefail), with
# findmnt, mount and sudo replaced by stubs, and prints each case's exit code.
#
#   review/m1.t16/stop-rule.sh <checkout of the pull request's head>
#
# Nothing is mounted and nothing outside a temporary directory is written.
set -u
repo=${1:?checkout}
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
mkdir -p "$t/bin"
python3 - "$repo/.github/workflows/topology-bench.yml" "$t" <<'PY'
import sys, yaml
w = yaml.safe_load(open(sys.argv[1]))
steps = w["jobs"]["bench"]["steps"]
for name, out in (("Barriers on the work disk", "begin.sh"), ("Work disk mount at the end", "end.sh")):
    (s,) = [s for s in steps if s.get("name") == name]
    open(f"{sys.argv[2]}/{out}", "w").write(s["run"])
PY
# sudo: run the command (mkdir is made harmless below). mount: exit $MOUNT_RC
# and, when it "works", switch the options findmnt reports to $OPTS_AFTER.
cat > "$t/bin/sudo" <<'SH'
#!/bin/sh
exec "$@"
SH
cat > "$t/bin/mkdir" <<'SH'
#!/bin/sh
exit 0
SH
cat > "$t/bin/mount" <<'SH'
#!/bin/sh
echo "mount $*" >> "$STATE/calls"
[ "$MOUNT_RC" = 0 ] && touch "$STATE/remounted"
exit "$MOUNT_RC"
SH
cat > "$t/bin/findmnt" <<'SH'
#!/bin/sh
[ -n "${FINDMNT_RC:-}" ] && exit "$FINDMNT_RC"
case "$*" in
  *TARGET*) echo / ;;
  *FSTYPE*) echo "${FSTYPE:-ext4}" ;;
  *FS-OPTIONS*) if [ -e "$STATE/remounted" ]; then echo "$OPTS_AFTER"; else echo "$OPTS_BEFORE"; fi ;;
esac
SH
chmod +x "$t/bin/"*
B=rw,discard,journal_async_commit,nobarrier,errors=remount-ro,commit=30,data=writeback
A=rw,discard,journal_async_commit,errors=remount-ro,commit=30,data=writeback
run() { # name script mount_rc opts_before opts_after [remounted-already]
  local name=$1 script=$2; shift 2
  rm -rf "$t/state"; mkdir "$t/state"
  [ "${4:-}" = remounted ] && touch "$t/state/remounted"
  local rc=0
  STATE="$t/state" MOUNT_RC=$1 OPTS_BEFORE=$2 OPTS_AFTER=$3 TB_WORK=/mnt/topology-bench \
    GITHUB_STEP_SUMMARY="$t/state/summary" PATH="$t/bin:$PATH" \
    bash --noprofile --norc -eo pipefail "$t/$script" > "$t/state/out" 2>&1 || rc=$?
  printf '%-58s exit %s\n' "$name" "$rc"
}
run "begin: remount takes, nobarrier gone"            begin.sh 0 "$B" "$A"
run "begin: mount fails"                               begin.sh 32 "$B" "$A"
run "begin: mount exits 0, nobarrier still there"      begin.sh 0 "$B" "$B"
run "begin: nobarrier last in the list"                begin.sh 0 "$B" "rw,nobarrier"
run "begin: nobarrier first in the list"               begin.sh 0 "$B" "nobarrier,rw"
run "begin: nobarrier the only option"                 begin.sh 0 "$B" "nobarrier"
run "begin: an option that only contains the word"     begin.sh 0 "$B" "rw,xnobarrierx"
run "begin: barriers were on already"                  begin.sh 0 "$A" "$A"
FINDMNT_RC=1 run "begin: findmnt fails"                begin.sh 0 "$B" "$A"
run "end: still on"                                    end.sh 0 "$B" "$A" remounted
run "end: nobarrier back"                              end.sh 0 "$B" "$B" remounted
FINDMNT_RC=1 run "end: findmnt fails"                  end.sh 0 "$B" "$A" remounted
