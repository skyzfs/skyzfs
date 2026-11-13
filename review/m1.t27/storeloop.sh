#!/bin/bash
# The hole job's replacement of --store in serve_args, as in the workflow at 7f1f9b8, with the summary on stdout.
set -e -o pipefail
SERVE_ARGS=$1
read -r -a given <<< "$SERVE_ARGS"; kept=()
for ((i = 0; i < ${#given[@]}; i++)); do
  case ${given[i]} in
    --store) i=$((i + 1)); [ "${given[i]:-}" = in-place ] || echo "  summary: not --store ${given[i]:-}" ;;
    --store=*) [ "${given[i]}" = --store=in-place ] || echo "  summary: not ${given[i]}" ;;
    *) kept+=("${given[i]}") ;;
  esac
done
SERVE_ARGS="--store in-place${kept[*]:+ ${kept[*]}}"
echo "[$1] -> [$SERVE_ARGS]"
