case $ZCR_SCALE in
  *[!0-9\ ]*) echo "::error::scale is counts separated by spaces: $ZCR_SCALE"; exit 64 ;;
esac
read -ra counts <<< "$ZCR_SCALE"
[ "${#counts[@]}" -le 3 ] || { echo "::error::scale: at most three counts"; exit 64; }
for n in "${counts[@]}"; do
  case $n in 0*) echo "::error::scale: not a count: $n"; exit 64 ;; esac
  if [ "${#n}" -gt 6 ] || [ "$n" -gt 100000 ]; then echo "::error::scale: $n is more than 100000"; exit 64; fi
done
echo ACCEPTED "${counts[*]:-}"
