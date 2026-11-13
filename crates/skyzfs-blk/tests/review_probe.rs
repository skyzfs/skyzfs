//! Hand-written histories against the model, from the review of pull
//! request #4 (branch `skyzfs-review`, `review/m1.1/review_probe.rs` at
//! e82406d), adopted here. On adoption they assert instead of printing
//! and use the contract's `Buf`. Each is a history a correct target can
//! produce, so the model must accept it.
//!
//! Rule numbers refer to the crate documentation of `skyzfs-blk`.
use skyzfs_blk::model::Model;
use skyzfs_blk::*;

const B: usize = BLOCK_USIZE;
fn m() -> Model {
    Model::new(Geometry::new(1, 16, false).unwrap())
}
fn w(m: &mut Model, id: u64, block: u64, n: usize, fill: u8) {
    m.submit(&Request {
        id: RequestId(id),
        op: Op::Write {
            offset: block * BLOCK_SIZE,
            data: Buf::filled(n, fill),
        },
    })
    .unwrap();
}
fn r(m: &mut Model, id: u64, block: u64, n: usize) {
    m.submit(&Request {
        id: RequestId(id),
        op: Op::Read {
            offset: block * BLOCK_SIZE,
            buf: Buf::zeroed(n),
        },
    })
    .unwrap();
}
fn f(m: &mut Model, id: u64) {
    m.submit(&Request {
        id: RequestId(id),
        op: Op::Flush,
    })
    .unwrap();
}
fn ack(m: &mut Model, id: u64) -> Result<(), model::Violation> {
    m.complete(&Completion {
        id: RequestId(id),
        result: Ok(()),
        buf: None,
    })
}
fn got(m: &mut Model, id: u64, fills: &[u8]) -> Result<(), model::Violation> {
    let bytes: Vec<u8> = fills.iter().flat_map(|x| vec![*x; B]).collect();
    m.complete(&Completion {
        id: RequestId(id),
        result: Ok(()),
        buf: Buf::from_blocks(&bytes),
    })
}

/// R3, R7: a read in flight across a write and a completed flush may
/// return the state before the write (Codex's finding).
#[test]
fn a_read_across_flush() {
    let mut m = m();
    r(&mut m, 1, 0, 1);
    w(&mut m, 2, 0, 1, 7);
    ack(&mut m, 2).unwrap();
    f(&mut m, 3);
    ack(&mut m, 3).unwrap();
    got(&mut m, 1, &[0]).unwrap();
}

/// R7, R8: X and Y overlap, both in flight; the daemon's order is X, Y.
/// Y acknowledged, flush completed, X acknowledged: reads return Y
/// (finding B1). The same without the flush is the control.
#[test]
fn b_mutation_across_flush() {
    let mut m = m();
    w(&mut m, 1, 0, 1, 1); // X
    w(&mut m, 2, 0, 1, 2); // Y
    ack(&mut m, 2).unwrap();
    f(&mut m, 3);
    ack(&mut m, 3).unwrap();
    ack(&mut m, 1).unwrap();
    r(&mut m, 4, 0, 1);
    got(&mut m, 4, &[2]).unwrap();
    // control: without the flush
    let mut m = self::m();
    w(&mut m, 1, 0, 1, 1);
    w(&mut m, 2, 0, 1, 2);
    ack(&mut m, 2).unwrap();
    ack(&mut m, 1).unwrap();
    r(&mut m, 4, 0, 1);
    got(&mut m, 4, &[2]).unwrap();
}

/// R10: as above, but X is never acknowledged and power is cut: Y, which
/// was flushed, is read back.
#[test]
fn c_power_cut() {
    let mut m = m();
    w(&mut m, 1, 0, 1, 1);
    w(&mut m, 2, 0, 1, 2);
    ack(&mut m, 2).unwrap();
    f(&mut m, 3);
    ack(&mut m, 3).unwrap();
    m.power_cut();
    assert_eq!(m.readback(), vec![0]);
    r(&mut m, 4, 0, 1);
    got(&mut m, 4, &[2]).unwrap();
}

/// R9: X in flight across a daemon loss, reissued and acknowledged, is
/// read back.
#[test]
fn d_kill() {
    let mut m = m();
    w(&mut m, 1, 0, 1, 1); // X in flight
    m.kill();
    ack(&mut m, 1).unwrap(); // reissued X acked
    r(&mut m, 4, 0, 1);
    got(&mut m, 4, &[1]).unwrap();
}
