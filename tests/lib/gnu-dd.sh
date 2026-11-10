# shellcheck shell=bash
# Sourced by the e2e and bench scripts. Sets DD to a GNU dd; the scripts
# run "$DD", never bare `dd`.
#
# Why: ubuntu-26.04 makes `dd` the Rust coreutils (uutils), whose dd
# fails O_DIRECT reads and writes with "IO error: Invalid input" (its
# buffer is not aligned the way O_DIRECT requires). It still installs GNU
# coreutils, with a `gnu` prefix: `gnudd`, from gnu-coreutils, which
# coreutils-from-uutils pre-depends on. ubuntu-24.04 and earlier have GNU
# dd as plain `dd` and no `gnudd`, so there this is just `dd`. Either
# way the scripts get the GNU semantics they were written against
# (iflag/oflag=direct, fullblock, notrunc).
# shellcheck disable=SC2034 # DD is read by the scripts that source this
if command -v gnudd >/dev/null 2>&1; then
  DD="gnudd"
else
  DD="dd"
  # Not fatal: buffered dd works with any implementation. Only the
  # O_DIRECT calls need GNU, and they would fail loudly or silently
  # depending on the caller, so say why up front.
  # GNU's --version names the FSF; uutils' does not. Not "GNU
  # coreutils": Ubuntu 24.04's GNU dd prints "dd (coreutils) 9.4".
  case "$(dd --version 2>/dev/null)" in
    *"Free Software Foundation"*) ;;
    *) echo "warning: $(command -v dd) is not GNU dd and no gnudd is installed;" \
            "O_DIRECT dd calls may fail" >&2 ;;
  esac
fi
