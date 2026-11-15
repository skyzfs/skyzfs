#!/usr/bin/env python3
"""#101 at 7e4b7b8: two tests for plants of `plants.py` that pass every test.

    gaps.py CHECKOUT TARGET_DIR

R7. The pull request's test of the `applied` line's hashes writes at sectors 8 and 9, inside the
first two labels, where the record writes a `label` line too. The plant keeps a write's hashes
only where the record writes that line. The test here has one write of a block in the middle of
the volume go through the record.

C7. No test turns the commands with no signal while the command's file is there and no write is
under way. The plant reads the command at every such turn. The test here lets a cut go and
turns once more.

Run as the head has it and under each plant. The files are restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
REC = 'bins/skyzfsd/src/record.rs'
CUT = 'bins/skyzfsd/src/cut_commands.rs'
TESTS = 'bins/skyzfsd/src/record/tests.rs'
CUT_TESTS = 'bins/skyzfsd/src/cut_commands/tests.rs'
CUT_TEST = r'''
#[test]
fn review_a_turn_with_no_signal_reads_no_command() {
    let mut rig = Rig::new();
    let mut stand = Stand::holding(&[(3, CutPhase::Uploading, &[1]), (4, CutPhase::Waiting, &[2])]);
    rig.send("s release 3", &mut stand);
    assert_eq!(rig.reply(), "s ok\n");
    // The command's file is still there, and the queue turns with no signal.
    assert!(!rig.commands.turn(false, &mut stand));
    println!("review: after a turn with no signal the reply is {:?}", rig.reply());
    assert_eq!(rig.reply(), "s ok\n", "the command is carried out once");
    assert_eq!(stand.released, [3]);
}
'''
TEST = r'''
#[test]
fn review_a_write_away_from_the_labels_has_its_hashes_and_its_bytes() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("requests.tsv");
    let data_path = dir.path().join("requests.tsv.data");
    let mut inner = Scripted::default();
    // One block in the middle of the volume.
    inner
        .waits
        .push_back(Ok(vec![fetched(0, op::WRITE, SIZE / 2 / 512, 8)]));
    let file = open(&path).expect("open");
    let contents = Contents::new(Some(open(&data_path).expect("the data")));
    let mut watcher = watcher(Lines::another(&file), contents.clone());
    let mut queue = Recorded::new(inner, Some(file), 0, SIZE, "off", contents);
    let _ = queue.wait(&mut Vec::new(), &mut Vec::new());
    let buf = queue.take_buf(0, 1).expect("a buffer");
    watcher(Seen::Applied(RequestId(0)));
    let p = pid();
    let got: Vec<String> = lines(&path)
        .into_iter()
        .filter(|line| line.contains("\tapplied\t") || line.contains("\tlabel\t"))
        .collect();
    println!("review: {got:?}");
    assert_eq!(
        got,
        [format!(
            "{p}\tapplied\t0\twrite\t-\t-\t-\t{}\tdata=0",
            hex(&Sha256::digest(&buf[..4096])[..8])
        )]
    );
    assert_eq!(std::fs::read(&data_path).expect("the data"), &buf[..]);
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfsd', '--features', 'test-hooks', '--lib', '--', 'review_', '--nocapture']
PLANTS = [
    ('as the head has it', None, None, None),
    ('C7 a command is read again at each later turn', CUT,
     "        if asked {", "        if asked || self.job.is_none() {"),
    ("R7 a write's hashes are kept only where the log writes its label line", REC,
     "            if self.lines.file.is_some() {\n                self.contents.took(tag, offset, &buf[..written]);",
     "            if self.lines.file.is_some() && self.ends.touched(offset, len) {\n"
     "                self.contents.took(tag, offset, &buf[..written]);"),
]


def run(env):
    p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    print('    exit', p.returncode)
    for ln in out.splitlines():
        s = ln.strip()
        if ('review: ' in s or s.startswith(('error', 'left:', 'right:')) or (s.startswith('test ') and 'FAILED' in s)
                or (s.startswith('test result') and ' 0 passed; 0 failed' not in s)):
            print('   ', s[:330])


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    paths = {name: os.path.join(ROOT, name) for name in (REC, CUT, TESTS, CUT_TESTS)}
    orig = {name: open(path).read() for name, path in paths.items()}

    def put(name, text):
        with open(paths[name], 'w') as f:
            f.write(text)

    try:
        put(TESTS, orig[TESTS] + TEST)
        put(CUT_TESTS, orig[CUT_TESTS] + CUT_TEST)
        for name, where, old, new in PLANTS:
            print(name + ':', flush=True)
            if where:
                assert orig[where].count(old) == 1, name
                put(where, orig[where].replace(old, new))
            run(env)
            if where:
                put(where, orig[where])
    finally:
        for name in paths:
            put(name, orig[name])


if __name__ == '__main__':
    main()
