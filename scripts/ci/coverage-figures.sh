#!/bin/sh
# Prints the two README badge figures, space-separated: the line
# coverage percentage (one decimal) and the number of tests passed.
#
#   scripts/ci/coverage-figures.sh coverage.json coverage.log
#
# coverage.json is a `cargo llvm-cov report --json --summary-only` export
# (its line totals); coverage.log is the captured test output (its
# "test result: ok." lines are summed). Shared by publish-badges.sh and
# the coverage job's summary step, so the badges and the summary agree.
set -eu

summary_json=${1:?coverage.json}
coverage_log=${2:?coverage.log}

percent=$(jq '.data[0].totals.lines.percent' "$summary_json" | awk '{printf "%.1f", $1}')
passed=$(grep -E '^test result: ok\.' "$coverage_log" \
    | sed -E 's/^test result: ok\. ([0-9]+) passed.*/\1/' \
    | awk '{s += $1} END {print s + 0}')
echo "$percent $passed"
