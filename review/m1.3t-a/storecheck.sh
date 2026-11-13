#!/bin/bash
# The replay rig's handling of RIG_SERVE_ARGS at b0e2879 (replay-rig.sh:123-149), alone.
set -euo pipefail
read -r -a SERVE_ARGS <<< "${1:-}"
IN_PLACE=""; STORE_NAMED=""
for i in "${!SERVE_ARGS[@]}"; do
    case ${SERVE_ARGS[$i]} in
        --store=*) STORE_NAMED=1; [ "${SERVE_ARGS[$i]}" != --store=in-place ] || IN_PLACE=1 ;;
        --store) STORE_NAMED=1; [ "${SERVE_ARGS[$((i + 1))]:-}" != in-place ] || IN_PLACE=1 ;;
    esac
done
if [ -z "$STORE_NAMED" ]; then
    for o in "${SERVE_ARGS[@]}"; do
        case ${o%%=*} in
            --log-high|--log-low|--writeback-kib|--home-writeback-kib|--readers|--step-pause-ms|--step-pause-until|--step-pause-at|--refuse-holes)
                echo "RIG_SERVE_ARGS: ${o%%=*} is an option of the log store; name --store log beside it (a build from before M1.3t)" >&2
                exit 64 ;;
        esac
    done
    SERVE_ARGS=(--store in-place "${SERVE_ARGS[@]}"); IN_PLACE=1
fi
echo "serve ${SERVE_ARGS[*]}; IN_PLACE='$IN_PLACE'"
