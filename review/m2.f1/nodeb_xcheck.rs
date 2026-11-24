//! The review of #134: node B's row codec at 4595f55, lines 201-203 and
//! 331-398 of bins/skyzfs-nodeb/src/main.rs copied unchanged, against
//! skyzfs-upload's `row::encode` and `row::decode`.
#![allow(dead_code)]

use skyzfs_local::inplace::CommittedCut;
use skyzfs_upload::row;

fn hex(h: &[u8]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

const ROW_LEN_V1: usize = 34;
const ROW_LEN_V2: usize = 66;

/// A row node B can read: its cut, and its commit where it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RowCut {
    state: [u8; 16],
    number: u64,
    group: u64,
    commit: Option<[u8; 32]>,
}

/// A row's bytes -> its cut, for version 1 and 2; None for anything else.
fn decode_row(b: &[u8]) -> Option<RowCut> {
    let v2 = match (b.len(), b.get(..2)) {
        (ROW_LEN_V1, Some([1, 0])) => false,
        (ROW_LEN_V2, Some([2, 0])) => true,
        _ => return None,
    };
    let mut state = [0u8; 16];
    let mut n = [0u8; 8];
    let mut g = [0u8; 8];
    state.copy_from_slice(&b[2..18]);
    n.copy_from_slice(&b[18..26]);
    g.copy_from_slice(&b[26..34]);
    let commit = v2.then(|| {
        let mut c = [0u8; 32];
        c.copy_from_slice(&b[34..66]);
        c
    });
    Some(RowCut {
        state,
        number: u64::from_le_bytes(n),
        group: u64::from_le_bytes(g),
        commit,
    })
}

fn encode_row(c: &RowCut) -> Vec<u8> {
    let version: u16 = if c.commit.is_some() { 2 } else { 1 };
    let mut b = version.to_le_bytes().to_vec();
    b.extend_from_slice(&c.state);
    b.extend_from_slice(&c.number.to_le_bytes());
    b.extend_from_slice(&c.group.to_le_bytes());
    if let Some(h) = c.commit {
        b.extend_from_slice(&h);
    }
    b
}

/// `row`'s lines for the row: `row STATE NUMBER GROUP` for either
/// version (`row none`, `row unreadable HEX`), then `row_commit HEX` for
/// a row of version 2 and `row_commit none` for any other.
fn row_line(bytes: Option<&[u8]>) -> String {
    let Some(b) = bytes else {
        return "row\tnone\nrow_commit\tnone".to_string();
    };
    match decode_row(b) {
        Some(c) => format!(
            "row\t{}\t{}\t{}\nrow_commit\t{}",
            hex(&c.state),
            c.number,
            c.group,
            c.commit.map_or("none".to_string(), |h| hex(&h))
        ),
        None => format!("row\tunreadable\t{}\nrow_commit\tnone", hex(b)),
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut b = [0u8; N];
        for x in b.iter_mut() {
            *x = self.next() as u8;
        }
        b
    }
}

/// The daemon writes, node B reads: the same cut and commit, and node B
/// writes back the same bytes.
#[test]
fn review_f1_node_b_reads_what_the_daemon_writes() {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..10_000 {
        let cut = CommittedCut {
            state: r.bytes::<16>(),
            number: r.next(),
            group: r.next(),
        };
        let commit = r.bytes::<32>();
        let b = row::encode(&cut, &commit);
        let got = decode_row(&b).expect("node B reads the daemon's row");
        assert_eq!(
            (got.state, got.number, got.group, got.commit),
            (cut.state, cut.number, cut.group, Some(commit))
        );
        assert_eq!(encode_row(&got), b, "node B writes the same bytes");
        assert_eq!(
            row_line(Some(&b)),
            format!(
                "row\t{}\t{}\t{}\nrow_commit\t{}",
                hex(&cut.state),
                cut.number,
                cut.group,
                hex(&commit)
            )
        );
    }
}

/// Node B writes (version 1 and 2), the daemon reads: the same cut and
/// commit.
#[test]
fn review_f1_the_daemon_reads_what_node_b_writes() {
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    for k in 0..10_000 {
        let c = RowCut {
            state: r.bytes::<16>(),
            number: r.next(),
            group: r.next(),
            commit: (k % 2 == 0).then(|| r.bytes::<32>()),
        };
        let named = row::decode(&encode_row(&c)).expect("the daemon reads node B's row");
        assert_eq!(
            (named.cut.state, named.cut.number, named.cut.group, named.commit),
            (c.state, c.number, c.group, c.commit)
        );
    }
}

/// Any bytes: node B reads a row exactly where the daemon does, and the
/// same cut and commit.
#[test]
fn review_f1_node_b_and_the_daemon_accept_the_same_bytes() {
    let mut r = Rng(0xdead_beef_cafe_f00d);
    let (mut read, mut refused) = (0, 0);
    for _ in 0..200_000 {
        let len = (r.next() % 80) as usize;
        let mut b: Vec<u8> = (0..len).map(|_| r.next() as u8).collect();
        if len >= 2 && r.next() % 4 != 0 {
            let v = (r.next() % 4) as u16;
            b[..2].copy_from_slice(&v.to_le_bytes());
        }
        if r.next() % 3 == 0 {
            b.resize(if r.next() % 2 == 0 { 34 } else { 66 }, 7);
            let v: u16 = if r.next() % 2 == 0 { 1 } else { 2 };
            b[..2].copy_from_slice(&v.to_le_bytes());
        }
        let nb = decode_row(&b);
        let d = row::decode(&b).ok();
        assert_eq!(nb.is_some(), d.is_some(), "{}", hex(&b));
        if let (Some(nb), Some(d)) = (nb, d) {
            assert_eq!(
                (nb.state, nb.number, nb.group, nb.commit),
                (d.cut.state, d.cut.number, d.cut.group, d.commit)
            );
            read += 1;
        } else {
            refused += 1;
        }
    }
    println!("node B and the daemon agree on 200000 byte strings: {read} read, {refused} refused");
    assert!(read > 10_000 && refused > 10_000);
}
