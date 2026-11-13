#!/bin/bash
# Tests of tests/lib/net-try.sh. Run: bash tests/kernel/test_net_try.sh
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=tests/lib/net-try.sh
. "$HERE/../lib/net-try.sh"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
is() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi; }
NET_TRY_PAUSE=0
flaky() { echo x >> "$T/calls"; [ "$(wc -l < "$T/calls")" -ge "$1" ]; }       # succeeds at its Nth call

: > "$T/calls"; net_try flaky 1; is "a command that works is run once" "$?/$(wc -l < "$T/calls" | tr -d ' ')/$NET_TRIES" 0/1/1
: > "$T/calls"; net_try flaky 3; is "one that works at the third try" "$?/$(wc -l < "$T/calls" | tr -d ' ')/$NET_TRIES" 0/3/3
: > "$T/calls"; net_try flaky 4; rc=$?
is "one that never works is tried three times and fails" "$([ $rc != 0 ] && echo failed)/$(wc -l < "$T/calls" | tr -d ' ')" failed/3
net_try sh -c 'exit 7'; is "the last status is passed on" "$?" 7

cat > "$T/apt.log" <<'LOGEOF'
Ign:4 http://azure.archive.ubuntu.com/ubuntu noble-updates/main amd64 libudev-dev amd64 255.4
Err:2 https://security.ubuntu.com/ubuntu noble-updates/universe amd64 libclang-18-dev amd64 1:18.1.3
  Temporary failure resolving 'azure.archive.ubuntu.com'
E: Unable to fetch some archives, maybe run apt-get update or try with --fix-missing?
LOGEOF
case $(net_why "$T/apt.log") in "could not resolve the package mirrors"*"a network outage"*) v=named ;; *) v=$(net_why "$T/apt.log") ;; esac
is "a name that does not resolve is said so" "$v" named
printf 'fatal: unable to access https://github.com/x/: Could not resolve host: github.com\n' > "$T/git.log"
case $(net_why "$T/git.log") in "could not resolve"*) v=named ;; *) v=other ;; esac
is "for a clone as well" "$v" named
printf 'Reading package lists...\nE: Unable to locate package nosuch\n' > "$T/other.log"
is "another failure is given as it is" "$(net_why "$T/other.log")" "E: Unable to locate package nosuch"
net_down "$T/apt.log" "$T/other.log"; is "net_down: a log with an unresolved name" "$?" 0
net_down "$T/other.log" "$T/nosuch.log"; is "net_down: not for another failure or a missing log" "$?" 1
: > "$T/empty.log"
is "a log with no reason says so" "$(net_why "$T/empty.log")" "no reason found in empty.log"

[ "$fails" = 0 ] || { echo "$fails failed"; exit 1; }
echo "all passed"
