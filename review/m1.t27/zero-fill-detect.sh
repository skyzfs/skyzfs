#!/bin/bash
# Review probe for pull request #63: hole-rig.sh decides whether the
# daemon has `create --zero-fill` with
#     if ! "$SKYZFSD" help 2>&1 | grep -q -- '--zero-fill'; then skip
# under `set -o pipefail`. grep -q exits at its first match; a daemon
# still printing then gets a broken pipe, the pipeline fails, and the
# course is skipped although the option is there. How often?
#   zero-fill-detect.sh SKYZFSD TRIES [busy]
set -Eeuo pipefail
SKYZFSD=$1; TRIES=$2
wrong=0
for _ in $(seq 1 "$TRIES"); do
    if ! "$SKYZFSD" help 2>&1 | grep -q -- '--zero-fill'; then wrong=$((wrong + 1)); fi
done
echo "$TRIES tries${3:+ ($3)}: the option was taken as missing $wrong times"
