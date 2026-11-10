// Review probe, not part of the pull request.
use skyzfs_blk::model::Model;
use skyzfs_blk::*;

const B: usize = BLOCK_USIZE;
fn m() -> Model { Model::new(Geometry::new(1, 16, false).unwrap()) }
fn w(m: &mut Model, id: u64, block: u64, n: usize, fill: u8) {
    m.submit(&Request { id: RequestId(id), op: Op::Write { offset: block * BLOCK_SIZE, data: vec![fill; n * B] } }).unwrap();
}
fn r(m: &mut Model, id: u64, block: u64, n: usize) {
    m.submit(&Request { id: RequestId(id), op: Op::Read { offset: block * BLOCK_SIZE, buf: vec![0; n * B] } }).unwrap();
}
fn f(m: &mut Model, id: u64) { m.submit(&Request { id: RequestId(id), op: Op::Flush }).unwrap(); }
fn ack(m: &mut Model, id: u64) -> Result<(), model::Violation> {
    m.complete(&Completion { id: RequestId(id), result: Ok(()), buf: None })
}
fn got(m: &mut Model, id: u64, fills: &[u8]) -> Result<(), model::Violation> {
    let bytes = fills.iter().flat_map(|x| vec![*x; B]).collect();
    m.complete(&Completion { id: RequestId(id), result: Ok(()), buf: Some(bytes) })
}

// A (Codex): read in flight across a completed flush.
#[test]
fn a_read_across_flush() {
    let mut m = m();
    r(&mut m, 1, 0, 1);
    w(&mut m, 2, 0, 1, 7); ack(&mut m, 2).unwrap();
    f(&mut m, 3); ack(&mut m, 3).unwrap();
    println!("A: {:?}", got(&mut m, 1, &[0]));
}

// B: X and Y overlap, both in flight. Daemon order X, Y. Y acked, flush, X acked. Final content Y.
#[test]
fn b_mutation_across_flush() {
    let mut m = m();
    w(&mut m, 1, 0, 1, 1); // X
    w(&mut m, 2, 0, 1, 2); // Y
    ack(&mut m, 2).unwrap();
    f(&mut m, 3); ack(&mut m, 3).unwrap();
    ack(&mut m, 1).unwrap();
    r(&mut m, 4, 0, 1);
    println!("B: read Y after order X,Y: {:?}", got(&mut m, 4, &[2]));
    // control: without the flush
    let mut m = self::m();
    w(&mut m, 1, 0, 1, 1); w(&mut m, 2, 0, 1, 2);
    ack(&mut m, 2).unwrap(); ack(&mut m, 1).unwrap();
    r(&mut m, 4, 0, 1);
    println!("B control (no flush): {:?}", got(&mut m, 4, &[2]));
}

// C: as B but X never acked; power cut; read back Y (X logged before Y, both durable).
#[test]
fn c_power_cut() {
    let mut m = m();
    w(&mut m, 1, 0, 1, 1);
    w(&mut m, 2, 0, 1, 2);
    ack(&mut m, 2).unwrap();
    f(&mut m, 3); ack(&mut m, 3).unwrap();
    m.power_cut();
    println!("C: readback {:?}", m.readback());
    r(&mut m, 4, 0, 1);
    println!("C: Y after cut: {:?}", got(&mut m, 4, &[2]));
}

// D: X in flight across a kill, with Y acked after kill... (X applied by old daemon, then Y, then X reissued)
#[test]
fn d_kill() {
    let mut m = m();
    w(&mut m, 1, 0, 1, 1); // X in flight
    m.kill();
    ack(&mut m, 1).unwrap(); // reissued X acked
    r(&mut m, 4, 0, 1);
    println!("D: {:?}", got(&mut m, 4, &[1]));
}
