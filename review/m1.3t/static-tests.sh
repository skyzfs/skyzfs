#!/bin/bash
# Static list of test functions in a commit's skyzfs crates and bins: "file::fn" for
# every fn that follows a #[test] attribute (other attributes and doc lines may lie between).
#   static-tests.sh COMMIT
set -eu
c=$1
git ls-tree -r --name-only "$c" -- crates/skyzfs-local crates/skyzfs-ublk crates/skyzfs-ublk-sys crates/skyzfs-blk bins/skyzfsd bins/skyzfs-blkcheck | grep '\.rs$' | while read -r f; do
    git show "$c:$f" | awk -v f="$f" '
        /^[[:space:]]*#\[test\]/ { armed = 1; next }
        armed && /^[[:space:]]*(pub )?(async )?fn [a-zA-Z0-9_]+/ { n = $0; sub(/^[[:space:]]*(pub )?(async )?fn /, "", n); sub(/[^a-zA-Z0-9_].*/, "", n); print f "::" n; armed = 0; next }
        armed && /^[[:space:]]*(#\[|\/\/)/ { next }
        armed && /^[[:space:]]*$/ { next }
        { armed = 0 }'
done | sort
