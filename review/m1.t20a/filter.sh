#!/usr/bin/env bash
# The workflow's filter for serve_args, and the rig's parsing, on their own.
# The two snippets are copied from .github/workflows/skyzfs-replay-rig.yml and tests/kernel/replay-rig.sh at d4d5b10.
filter() { SERVE_ARGS=$1; [[ $SERVE_ARGS =~ ^[A-Za-z0-9=.,:/\ -]*$ ]]; }
parse() { # prints the options the rig would run with, and whether in place
    set -u
    RIG_SERVE_ARGS=$1
    read -r -a SERVE_ARGS <<< "${RIG_SERVE_ARGS:-}"
    IN_PLACE=""; STORE_NAMED=""
    for i in "${!SERVE_ARGS[@]}"; do
        case ${SERVE_ARGS[$i]} in
            --store=*) STORE_NAMED=1; [ "${SERVE_ARGS[$i]}" != --store=in-place ] || IN_PLACE=1 ;;
            --store) STORE_NAMED=1; [ "${SERVE_ARGS[$((i + 1))]:-}" != in-place ] || IN_PLACE=1 ;;
        esac
    done
    [ -n "$STORE_NAMED" ] || SERVE_ARGS=(--store log "${SERVE_ARGS[@]}")
    printf '[%s] ' "${SERVE_ARGS[@]}"; echo "in_place=${IN_PLACE:-no} n=${#SERVE_ARGS[@]}"
}
echo "== filter"
while IFS= read -r s; do
    if filter "$s"; then r=admitted; else r=refused; fi
    printf '%-9s %q\n' "$r" "$s"
done <<'IN'

--store in-place
--store=in-place --depth 16
a;b
$(id)
`id`
a\b
a"b
a'b
a*b
a?b
a[b]
~root
a{b,c}
a|b
a&b
a>b
a<b
a#b
a!b
a%b
a@b
a+b
a_b
a	b
é
IN
printf 'newline: '; if filter $'a\nb'; then echo admitted; else echo refused; fi
printf 'cr: '; if filter $'a\rb'; then echo admitted; else echo refused; fi
echo "== round trip through printf %q and a sourced file"
for s in "" "--store in-place" "--store=in-place --depth 16" "  --depth   16  " "--store"; do
    printf 'export RIG_SERVE_ARGS=%q\n' "$s" > env.sh
    ( . ./env.sh; [ "$RIG_SERVE_ARGS" = "$s" ] && echo "same: $(printf %q "$s")" || echo "DIFFERENT: $(printf %q "$s")" )
done
echo "== parse"
for s in "" "--store in-place" "--store=in-place" "--store=log" "--store log" "--depth 16" "--depth 16 --store in-place" "--store" "--store in-place --store log" "--store log --store in-place" "--store=in-place --store=log" "  --depth   16  " "--store in-placex" "--storex in-place" "/tmp/* --depth 1"; do
    printf '%-34q -> ' "$s"; ( parse "$s" )
done
