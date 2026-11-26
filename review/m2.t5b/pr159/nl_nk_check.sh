#!/bin/bash
# The workflow's N:K check and scale_all, taken out of a checkout and run
# on a table of inputs. Usage (from the checkout's root):
#   bash nk_check.sh
# Prints, for each input, the workflow step's exit and scale_all's runs
# and about.txt lines (scale stubbed to print RUN N[:K]).
set -uo pipefail
wf=.github/workflows/skyzfs-zfs-cli-record.yml
rec=tests/kernel/zfs-cli-record.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# The step's validation: from `case $ZCR_SCALE in` to the end of its loop.
awk '/^          case \$ZCR_SCALE in/ { p = 1 } p { sub(/^          /, ""); print } p && /^done$/ { exit }' "$wf" > "$tmp/wf.sh"
# scale_all, the function alone.
awk '/^scale_all\(\) \{/ { p = 1 } p { print } p && /^\}$/ { exit }' "$rec" > "$tmp/sa.sh"
for v in '' 10 10:1000 10:0 100000:1 1:100000 10:10000 4:40000 '10:1000 10:1000' ' 10:1 ' \
    1:20000 1:20001 10:18181 10:18182 9:20000 99999:2 199999:1 1:020000 \
    '10 10:0' '1 2 3 4' \
    10: :5 0 0:5 10:02 010 1:2:3 10::5 100000:2 100001 1:100001 1:999999 9999999 10:x a '5 6 7 8' \
    1:99999999999999999999 "$(printf '10\t20')"; do
    rc=0
    ZCR_SCALE=$v bash "$tmp/wf.sh" > "$tmp/wf.out" 2>&1 || rc=$?
    runs=$(OUT=$tmp bash -c ': > "$OUT/about.txt"; scale() { printf "RUN %s " "$1${2:+:$2}"; }; . "$1"; scale_all $2; sed "s/^/[about: /; s/\$/]/" "$OUT/about.txt" | tr "\n" " "' _ "$tmp/sa.sh" "$v")
    printf '%-26s workflow exit %-3s scale_all: %s\n' "'$v'" "$rc" "${runs:-nothing}"
done
