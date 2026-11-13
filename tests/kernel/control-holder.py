#!/usr/bin/env python3
"""A process that holds a block device open, for the control rig.

    control-holder.py DEVICE excl|plain < COMMANDS > ANSWERS

Opens DEVICE for reading with direct I/O, with O_EXCL (as an imported
pool holds its device) or without, and prints `open` or `open failed:
errno N (text)`. Then one answer line per command line on stdin:

    read OFFSET   read 4096 bytes there: `read OFFSET ok SHA256` or
                  `read OFFSET failed: errno N (text)`
    close         close the device, print `closed` and exit 0

The rig writes the commands into a fifo and reads the answers from a
file, so a read that the server holds blocks this process and nothing
else. End of input without `close` also closes and exits.
"""
import hashlib
import mmap
import os
import sys


def main():
    dev, mode = sys.argv[1], sys.argv[2]
    flags = os.O_RDONLY | os.O_DIRECT | (os.O_EXCL if mode == 'excl' else 0)
    try:
        fd = os.open(dev, flags)
    except OSError as e:
        say(f'open failed: errno {e.errno} ({e.strerror})')
        return 1
    say('open')
    buf = mmap.mmap(-1, 4096)
    for line in sys.stdin:
        words = line.split()
        if not words:
            continue
        if words[0] == 'close':
            break
        if words[0] == 'read' and len(words) == 2 and words[1].isdigit():
            try:
                n = os.preadv(fd, [buf], int(words[1]))
                if n == 4096:
                    say(f'read {words[1]} ok {hashlib.sha256(buf[:]).hexdigest()}')
                else:
                    say(f'read {words[1]} failed: short read of {n} bytes')
            except OSError as e:
                say(f'read {words[1]} failed: errno {e.errno} ({e.strerror})')
        else:
            say(f'not a command: {line.strip()}')
    os.close(fd)
    say('closed')
    return 0


def say(text):
    print(text, flush=True)


if __name__ == '__main__':
    if len(sys.argv) != 3 or sys.argv[2] not in ('excl', 'plain'):
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(main())
