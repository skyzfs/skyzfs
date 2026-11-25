#!/usr/bin/env bash
# Review scratch (Q10, PR #148, next look): each plant applied alone to a
# copy of tests/kernel/zfs-cli-record.sh at HEAD, then the PR's unit
# test (tests/kernel/test_zfs_cli_record.sh at HEAD) run on it.
# "caught" = the unit test exits non-zero. U1-U17 are the plants of the
# review at 73dce38, their patterns brought to the current code; V1-V30
# are new (the pins of the adapter's words, the locale, secs, scale,
# catch's and keep's fixes).
#
#   bash review/q10/plants_next.sh HEAD WORKDIR
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
    if timeout 300 bash "$W/t/tests/kernel/test_zfs_cli_record.sh" > "$W/$id.log" 2>&1; then
        printf '%s\tnot caught\t%s\n' "$id" "$what"
    else
        printf '%s\tcaught\t%s\t(%s)\n' "$id" "$what" "$(grep -m1 '^FAIL' "$W/$id.log" | cut -c1-90)"
    fi
}
fresh
if bash "$W/t/tests/kernel/test_zfs_cli_record.sh" > "$W/base.log" 2>&1; then
    echo "base: unit test passes ($(grep -c '^ok' "$W/base.log") ok)"
else
    echo "base: unit test FAILS"
fi

plant U1 "program: -j dropped" 's/"\${ro\[@\]}" -j /"${ro[@]}" /'
plant U2 "program: memory limit 100 MB, not MiB" 's/-m 104857600/-m 100000000/'
plant U3 "rec: stdin not given" 's/lim "\$LIMIT" "\$@" < "\$in" >/lim "$LIMIT" "$@" >/'
plant U4 "rec: no limit" 's/    lim "\$LIMIT" "\$@" < "\$in"/    "$@" < "$in"/'
plant U5 "catch: no limit" 's/        lim "\$LIMIT" "\$@" < \/dev\/null/        "$@" < \/dev\/null/'
plant U6 "catch: pattern read on stderr" 's/grep -Eq -- "\$pattern" "\$tmp\/stdout"/grep -Eq -- "$pattern" "$tmp\/stderr"/'
plant U7 "catch: rc of the first try kept, not the last" 's/^        rc=0$/        [ "$i" = 1 ] \&\& rc=0/'
plant U8 "keep: a name used twice not refused" '/^keep() {/,/^}/{s/if \[ -e "\$d" \]; then/if false; then/}'
plant U9 "status: zpool status without -P" 's/rec "\$1" zpool status -P "\$2"/rec "$1" zpool status "$2"/'
plant U10 "agent_commands: zpool get without -p" 's/rec "\$x-get" zpool get -H -p -o/rec "$x-get" zpool get -H -o/'
plant U11 "record_pool: holds -r without -H" 's/rec "\$x-holds-r" zfs holds -H -r/rec "$x-holds-r" zfs holds -r/'
plant U12 "hold tag changed (TAG=skyzfs)" 's/^TAG=skyzfs-publish$/TAG=skyzfs/'
plant U13 "record_pool: members.lua run without -n" 's/program "\$x-members" "\$p" members.lua -n "\$p" t$/program "$x-members" "$p" members.lua "$p" t/'
plant U14 "catch: the not-caught note says caught" 's/note="not caught: no output of \$tries matched \/\$pattern\/; this is the last"/note="caught at try $i"/'
plant U15 "catch: returns 0 when not caught" 's/^    \[ -n "\$caught" \]$/    true/'
plant U16 "program: --locale read and dropped" 's/env=(env "LC_ALL=\$2")/env=()/'
plant U17 "program: --locale sets LANG, not LC_ALL" 's/env=(env "LC_ALL=\$2")/env=(env "LANG=$2")/'

plant V1 "main: export LC_ALL=C.UTF-8 in place of C" 's/^    export LC_ALL=C$/    export LC_ALL=C.UTF-8/'
plant V2 "main: LC_ALL=C.UTF-8 exported again after the pinned line" '/^    export LC_ALL=C$/a\    export LC_ALL=C.UTF-8'
plant V3 "rec: the command run with LC_ALL=C.UTF-8" 's/^    lim "\$LIMIT" "\$@" < "\$in" >/    LC_ALL=C.UTF-8 lim "$LIMIT" "$@" < "$in" >/'
plant V4 "record_pool: tagged-held without -n" 's/program "\$x-tagged-held" "\$p" tagged.lua -n /program "$x-tagged-held" "$p" tagged.lua /'
plant V5 "main: none-members without -n" 's/program none-members skznone members.lua -n /program none-members skznone members.lua /'
plant V6 "record_pool: hold-2 without -r" 's/\(rec "\$x-hold-2" .*\) zfs hold -r /\1 zfs hold /'
plant V7 "record_pool: release-2 without -r" 's/\(rec "\$x-release-2" .*\) zfs release -r /\1 zfs release /'
plant V8 "main: none-hold with the old tag" 's/rec none-hold zfs hold -r "\$TAG"/rec none-hold zfs hold -r skyzfs/'
plant V9 "main: rig-resilvered without its -g partner" 's/^    status rig-resilvered skzq$/    rec rig-resilvered zpool status -P skzq/'
plant V10 "program: instruction limit 10^7" 's/PROGRAM_LIMITS=(-t 100000000/PROGRAM_LIMITS=(-t 10000000/'
plant V11 "agent_commands: snapshot program run with -n" 's/program "\$x-snapshot-1" "\$p" snapshot.lua "\$p"/program "$x-snapshot-1" "$p" snapshot.lua -n "$p"/'
plant V12 "status: -g dropped from the partner" 's/rec "\$1-g" zpool status -P -g "\$2"/rec "$1-g" zpool status -P "$2"/'
plant V13 "assist property changed" 's/^ASSIST=skyzfs:assist$/ASSIST=skyzfs:assisted/'
plant V14 "agent_commands: zpool sync -f" 's/rec "\$x-assist-sync" zpool sync "\$p"/rec "$x-assist-sync" zpool sync -f "$p"/'
plant V15 "record_pool: holds without -H" 's/rec "\$x-holds" zfs holds -H "\$p@t"/rec "$x-holds" zfs holds "$p@t"/'
plant V16 "scale: hold without -r" 's/rec "scale-\$n-hold" zfs hold -r/rec "scale-$n-hold" zfs hold/'
plant V17 "scale: members without -n" 's/program "scale-\$n-members" "\$p" members.lua -n /program "scale-$n-members" "$p" members.lua /'
plant V18 "scale: its limit not local (leaks into the rest)" 's/^    local LIMIT=\${ZCR_SCALE_LIMIT:-1800}$/    LIMIT=${ZCR_SCALE_LIMIT:-1800}/'
plant V19 "scale: its limit dropped (the usual 120 s)" 's/^    local LIMIT=\${ZCR_SCALE_LIMIT:-1800}$/    :/'
plant V20 "rec: secs not written" 's/> "\$d\/secs"/> \/dev\/null/'
plant V21 "rec: secs in whole seconds" 's/printf "%.3f\\n", b - a/printf "%.0f\\n", b - a/'
plant V22 "main: the bare wait dropped" 's/^    wait$/    :/'
plant V23 "scale: its image not removed" 's/^    rm -f "\$WORK\/scale.img"$/    :/'
plant V24 "catch: a try at the limit does not end the tries (S4)" 's/^        \[ "\$rc" = 124 \] \&\& break$/        :/'
plant V25 "catch: scratch left when keep refuses (S6)" 's/|| { rm -rf "\${tmp:?}"; return 70; }/|| return 70/'
plant V26 "keep: argv written with spaces, not NUL" '/^keep() {/,/^}/{s/printf .%s\\0. "\$@" > "\$d\/argv"/printf "%s " "$@" > "$d\/argv"/}'
plant V27 "rec: --expect always seen (D4)" 's/if cat "\$d\/stdout" "\$d\/stderr" | grep -Eq -- "\$expect"; then/if true; then/'
plant V28 "rec: --expect reads stdout only" 's/if cat "\$d\/stdout" "\$d\/stderr" | grep/if cat "$d\/stdout" | grep/'
plant V29 "record_pool: hold-2 without --expect" 's/rec "\$x-hold-2" --expect .tag already exists. /rec "$x-hold-2" /'
plant V30 "main: rebuild pattern back to plain 'in progress'" "s/catch_status mirror-rebuild-status 'resilver .*' skzqm/catch_status mirror-rebuild-status 'in progress' skzqm/"
