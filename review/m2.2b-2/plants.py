#!/usr/bin/env python3
"""#101 at 7e4b7b8: plant one change at a time in the package's own code and run the tests of
skyzfsd (with its test hooks), of the status file and of the signals.

    plants.py CHECKOUT TARGET_DIR [PREFIX ...]

With prefixes, only the plants whose name starts with one of them. The file is restored after
each plant. A test program that a signal ends counts as a kill: it is reported as "did not
run", with the signal's line.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
CUT = 'bins/skyzfsd/src/cut_commands.rs'
REC = 'bins/skyzfsd/src/record.rs'
SIG = 'crates/skyzfs-ublk-sys/src/signals.rs'
STA = 'crates/skyzfs-ublk/src/status.rs'
SRV = 'bins/skyzfsd/src/serve.rs'
RUNS = [
    ['cargo', 'test', '-p', 'skyzfsd', '--features', 'test-hooks', '--no-fail-fast'],
    ['cargo', 'test', '-p', 'skyzfs-ublk', '--no-fail-fast', '--lib', '--', 'status'],
    ['cargo', 'test', '-p', 'skyzfs-ublk-sys', '--no-fail-fast', '--lib', '--', 'signals'],
]

PLANTS = [
    # the commands
    ('C1 a command while a write is under way is carried out', CUT,
     "            Ok(_) if self.job.is_some() => {", "            Ok(_) if self.job.is_some() && false => {"),
    ('C2 a map that grew under a write is not noticed', CUT,
     "    if now != job.map.len() {", "    if false && now != job.map.len() {"),
    ('C3 a cut whose flush is under way can be written', CUT,
     "    if cut.phase == CutPhase::Flushing {", "    if false && cut.phase == CutPhase::Flushing {"),
    ('C4 the map is written in the order the store gives it', CUT,
     "    map.sort_unstable();\n", ""),
    ('C5 the whole map is written in one turn', CUT,
     "    let end = (job.next + SLICE).min(job.map.len());", "    let end = job.map.len();"),
    ('C6 a block is written at the place of its turn in the map', CUT,
     "            Ok(true) => job.file.write_all_at(&job.buf, block * BLOCK_SIZE),",
     "            Ok(true) => job.file.write_all_at(&job.buf, at as u64 * BLOCK_SIZE),"),
    ('C7 a command is read again at each later turn', CUT,
     "        if asked {", "        if asked || self.job.is_none() {"),
    ('C8 with held cuts off the commands are carried out', CUT,
     "            Ok(_) if !cuts.holds_cuts() => refused(", "            Ok(_) if !cuts.holds_cuts() && false => refused("),
    ('C9 a path that is not absolute is taken', CUT,
     "                if path.is_absolute() {", "                if true {"),
    ('C10 the list of blocks is not written', CUT,
     "                writeln!(job.blocks, \"{block}\")", "                write!(job.blocks, \"\")"),
    ('C11 the file is made if it is not there', CUT,
     "        .write(true)\n        .open(&path)", "        .write(true)\n        .create(true)\n        .open(&path)"),
    ('C12 a block not in the cut\'s map is written all the same', CUT,
     "            Ok(false) => Ok(()),", "            Ok(false) => job.file.write_all_at(&job.buf, block * BLOCK_SIZE),"),
    ('C13 the write is of the cut before the one named', CUT,
     "        let written = match cuts.read(job.number, block, &mut job.buf) {",
     "        let written = match cuts.read(job.number - 1, block, &mut job.buf) {"),
    # the request log
    ('R1 a block\'s hash is its first 4 bytes', REC,
     "            hex(&digest[..8])", "            hex(&digest[..4])"),
    ('R2 the block after an aligned write is named too', REC,
     "    let last = (end - 1) / BLOCK;", "    let last = end / BLOCK;"),
    ('R3 the cut line has no number', REC,
     "            format!(\"cut\\t{tag}\\t{op}\\t{opens}\\t{}\\t{}\", cut.number, cut.group)",
     "            format!(\"cut\\t{tag}\\t{op}\\t{opens}\\t-\\t{}\", cut.group)"),
    ('R4 there is no cuts line', REC,
     "        recorded.line(&columns(\"cuts\", \"-\", None, cuts));", "        let _ = cuts;"),
    ('R5 the applied line does not say where the bytes lie', REC,
     "                Ok(at) => columns.push_str(&format!(\"\\tdata={at}\")),", "                Ok(_) => {}"),
    ('R6 the bytes are not written to the data file', REC,
     "file.write_all(bytes).map(|()| at)", "Ok::<u64, io::Error>(at)"),
    ('R7 a write\'s hashes are kept only where the log writes its label line', REC,
     "            if self.lines.file.is_some() {\n                self.contents.took(tag, offset, &buf[..written]);",
     "            if self.lines.file.is_some() && self.ends.touched(offset, len) {\n"
     "                self.contents.took(tag, offset, &buf[..written]);"),
    # the signals
    ('G1 SIGUSR2 is not in the set', SIG,
     "[libc::SIGTERM, libc::SIGINT, libc::SIGUSR1, libc::SIGUSR2]", "[libc::SIGTERM, libc::SIGINT, libc::SIGUSR1]"),
    ('G2 SIGUSR2 is taken as SIGUSR1', SIG,
     "        libc::SIGUSR2 => Some(Signal::Command),", "        libc::SIGUSR2 => Some(Signal::User),"),
    # the status
    ('V1 cuts_held is read where the copied bytes are', STA,
     "            held: field(320),", "            held: field(328),"),
    ('V2 a file of the twelfth version is not read', STA,
     "    } else if is(TWELFTH_LEN, TWELFTH_MAGIC) {", "    } else if false {"),
    ('V3 begun is said as taken_up', STA,
     "            2 => Self::Begun,", "            2 => Self::TakenUp,"),
    ('V4 status does not print cut_state', STA,
     "            lines.push((\"cut_state\", held.state.said().to_string()));\n", ""),
    # serve
    ('W1 a state taken up is shown as begun', SRV,
     "        Some(CutsTakenUp::TakenUp) => CutStateShown::TakenUp,", "        Some(CutsTakenUp::TakenUp) => CutStateShown::Begun,"),
    ('W2 cuts_held shows the copied bytes', SRV,
     "        held: figures.held,", "        held: figures.copied,"),
    ('W3 a signal is read as a command at every turn after it', SRV,
     "commands.turn(flags.command.swap(false, Ordering::SeqCst), pump.target())",
     "commands.turn(flags.command.load(Ordering::SeqCst), pump.target())"),
    ('W4 SIGUSR2 sets no flag', SRV,
     "                    flags.command.store(true, Ordering::SeqCst);", "                    let _ = &flags.command;"),
]


def tests(env):
    failed, broken = set(), []
    for cmd in RUNS:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env)
        out = p.stdout + p.stderr
        names = {ln.split()[1].split('::')[-1] for ln in out.splitlines()
                 if ln.startswith('test ') and ln.rstrip().endswith('FAILED')}
        failed |= names
        if p.returncode and not names:
            broken += [ln for ln in out.splitlines() if ln.startswith('error')] or ['exit %d' % p.returncode]
    return sorted(failed), broken


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    for name, path, *pairs in PLANTS:
        if ONLY and not name.startswith(ONLY):
            continue
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        counts = [orig.count(old) for old in pairs[0::2]]
        if counts != [1] * len(counts):
            print(f'{name}: NOT APPLIED (found {counts} times)', flush=True)
            continue
        changed = orig
        for old, new in zip(pairs[0::2], pairs[1::2]):
            changed = changed.replace(old, new)
        with open(full, 'w') as f:
            f.write(changed)
        try:
            failed, broken = tests(env)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
        if failed:
            print(f'{name}: KILLED by {len(failed)}: ' + ', '.join(failed[:5]) + (' ...' if len(failed) > 5 else ''),
                  flush=True)
        elif broken:
            print(f'{name}: DID NOT BUILD OR RUN: ' + ' | '.join(broken[:3])[:300], flush=True)
        else:
            print(f'{name}: SURVIVED', flush=True)


if __name__ == '__main__':
    if ONLY == ('none',):
        env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
        print('as the head has it:', tests(env), flush=True)
    else:
        main()
