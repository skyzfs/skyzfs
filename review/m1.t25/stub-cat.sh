#!/bin/sh
# A stand-in for cat: a read of a /proc/PID/stack hangs (STUB=hang) or
# gives a fixture (STUB=<file>); every other use is the real cat.
case "${1:-}" in
  /proc/*/stack)
    case "${STUB:-}" in
      hang) exec sleep 1000 ;;
      '') exec /bin/cat "$@" ;;
      *) exec /bin/cat "$STUB" ;;
    esac ;;
esac
exec /bin/cat "$@"
