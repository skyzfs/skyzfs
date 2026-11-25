#!/usr/bin/env bash
# Review scratch (Q10, PR #148): each plant applied alone to a copy of
# tests/kernel/zfs-cli-record.sh at the head, then the PR's unit test
# (tests/kernel/test_zfs_cli_record.sh) run on it. "caught" = the unit
# test exits non-zero.
#
#   bash review/q10/plants.sh HEAD WORKDIR
set -uo pipefail
HEAD=${1:?head}
W=${2:?workdir}
REPO=$(git rev-parse --show-toplevel)
fresh() {
    rm -rf "$W/t" && mkdir -p "$W/t/tests/kernel" "$W/t/tests/lib"
    for f in tests/kernel/zfs-cli-record.sh tests/kernel/test_zfs_cli_record.sh tests/lib/lim.sh tests/lib/gnu-dd.sh; do
        git -C "$REPO" show "$HEAD:$f" > "$W/t/$f"
    done
}
# plant ID DESCRIPTION SED-EXPRESSION
plant() {
    local id=$1 what=$2 expr=$3 f
    fresh
    f=$W/t/tests/kernel/zfs-cli-record.sh
    cp "$f" "$f.orig"
    sed -i -e "$expr" "$f"
    if cmp -s "$f" "$f.orig"; then
        printf '%s\tNOT APPLIED\t%s\n' "$id" "$what"
        return
    fi
    if bash "$W/t/tests/kernel/test_zfs_cli_record.sh" > "$W/$id.log" 2>&1; then
        printf '%s\tnot caught\t%s\n' "$id" "$what"
    else
        printf '%s\tcaught\t%s\t(%s)\n' "$id" "$what" "$(grep -m1 '^FAIL' "$W/$id.log" | cut -c1-80)"
    fi
}
fresh
bash "$W/t/tests/kernel/test_zfs_cli_record.sh" > "$W/base.log" 2>&1 && echo "base: unit test passes" || echo "base: unit test FAILS"

plant U1 "program: -j dropped" 's/"\${ro\[@\]}" -j /"${ro[@]}" /'
plant U2 "program: memory limit 100 MB, not MiB" 's/-m 104857600/-m 100000000/'
plant U3 "rec: stdin not given" 's/lim "\$LIMIT" "\$@" < "\$in" >/lim "$LIMIT" "$@" >/'
plant U4 "rec: no limit" 's/    lim "\$LIMIT" "\$@" < "\$in"/    "$@" < "$in"/'
plant U5 "catch: no limit" 's/        lim "\$LIMIT" "\$@" < \/dev\/null/        "$@" < \/dev\/null/'
plant U6 "catch: pattern read on stderr" 's/grep -Eq "\$pattern" "\$tmp\/stdout"/grep -Eq "$pattern" "$tmp\/stderr"/'
plant U7 "catch: rc of the first try kept, not the last" 's/^        rc=0$/        [ "$i" = 1 ] \&\& rc=0/'
plant U8 "keep: a name used twice not refused" '/^keep() {/,/^}/{s/if \[ -e "\$d" \]; then/if false; then/}'
plant U9 "agent_commands: zpool status without -P" 's/rec "\$x-status" zpool status -P "\$p"/rec "$x-status" zpool status "$p"/'
plant U10 "agent_commands: zpool get without -p" 's/zpool get -H -p -o property,value guid,load_guid,multihost,readonly,checkpoint "\$p"$/zpool get -H -o property,value guid,load_guid,multihost,readonly,checkpoint "$p"/'
plant U11 "record_pool: holds -r without -H" 's/zfs holds -H -r "\$p@t"/zfs holds -r "$p@t"/'
plant U12 "record_pool: hold tag changed (skyzfs -> other)" 's/zfs hold -r skyzfs "\$p@t"/zfs hold -r other "$p@t"/g'
plant U13 "record_pool: members.lua run without -n" 's/members.lua -n "\$p" t$/members.lua "$p" t/'
plant U14 "catch: the not-caught note says caught" 's/echo "not caught: no output of \$tries matched \/\$pattern\/; this is the last"/echo "caught at try $i"/'
plant U15 "catch: returns 0 when not caught" 's/^    \[ "\$i" -le "\$tries" \]$/    true/'
# The head's second commit (73dce38): program --locale.
plant U16 "program: --locale read and dropped" 's/env=(env "LC_ALL=\$2")/env=()/'
plant U17 "program: --locale sets LANG, not LC_ALL" 's/env=(env "LC_ALL=\$2")/env=(env "LANG=$2")/'
