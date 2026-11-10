#!/bin/sh
# Exits 0 when every path that differs between two commits is
# documentation, and 1 when anything else differs or the commits cannot
# be compared. publish-badges.sh uses it to keep publishing figures for a
# commit that main has moved past only by such changes.
#
#   scripts/ci/docs-only.sh BASE HEAD
#
# Documentation is the same set the `changes` jobs' paths-filter treats
# as non-code (ci.yml, e2e.yml, coverage.yml): anything under docs/, any
# Markdown file, and LICENSE. Keep the two in step. Both commits must be
# present locally.
set -eu

base=${1:?base commit}
head=${2:?head commit}

# --no-renames lists both sides of a rename, so moving code into a .md
# file does not count as a documentation change.
paths=$(git diff --name-only --no-renames "$base" "$head") || exit 1
printf '%s\n' "$paths" | while IFS= read -r path; do
    case $path in
        '' | docs/* | *.md | LICENSE) ;;
        *)
            echo "docs-only: $path is not documentation"
            exit 1
            ;;
    esac
done
