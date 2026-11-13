EARLIER_FAILED=${TB_EARLIER_FAILED:-}
[[ $EARLIER_FAILED =~ ^[0-9\ ]*$ ]] || { echo "TB_EARLIER_FAILED is row numbers, not: $EARLIER_FAILED" >&2; exit 64; }
read -r -a _ef <<< "$EARLIER_FAILED"
case ${#_ef[@]} in
    0) ;;
    1) LABEL="${LABEL:+$LABEL; }after failed row ${_ef[0]}" ;;
    *) LABEL="${LABEL:+$LABEL; }after failed rows $(IFS=,; echo "${_ef[*]}")" ;;
esac
