// The review of #139 (M2.T7): node B's snap-row decode against the
// uploader's SnapRow codec, both directions, golden and random, and the
// refusal cases. Appended to bins/skyzfs-nodeb/src/main.rs for a local
// run only (xcheck_snaprow.sh); not part of the pull request.
#[cfg(test)]
mod review_xcheck_snaprow {
    use super::*;
    use skyzfs_upload::publish::{
        manifest_key, snap_row_key, SnapRow, SNAP_ROW_LEN as UP_LEN, SNAP_ROW_PREFIX as UP_PREFIX,
    };

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            // xorshift64*
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn bytes<const N: usize>(&mut self) -> [u8; N] {
            let mut b = [0u8; N];
            for x in b.iter_mut() {
                *x = self.next() as u8;
            }
            b
        }
        fn vec(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| self.next() as u8).collect()
        }
        // A u64 that is often at an edge.
        fn edgy(&mut self) -> u64 {
            match self.next() % 6 {
                0 => 0,
                1 => u64::MAX,
                2 => 1 << (self.next() % 64),
                3 => self.next() % 1000,
                _ => self.next(),
            }
        }
    }

    /// Node B's line split into its fields.
    struct Line {
        state: String,
        number: u64,
        group: u64,
        commit: String,
        snap: String,
        len: u32,
        hash: String,
    }

    fn parse_line(line: &str) -> Line {
        let f: Vec<&str> = line.split('\t').collect();
        assert_eq!(f.len(), 8, "{line}");
        assert_eq!(f[0], "snaprow", "{line}");
        Line {
            state: f[1].to_string(),
            number: f[2].parse().expect("number"),
            group: f[3].parse().expect("group"),
            commit: f[4].to_string(),
            snap: f[5].to_string(),
            len: f[6].parse().expect("len"),
            hash: f[7].to_string(),
        }
    }

    fn same(line: &Line, row: &SnapRow) {
        assert_eq!(line.state, hex(&row.cut.state));
        assert_eq!(line.number, row.cut.number);
        assert_eq!(line.group, row.cut.group);
        assert_eq!(line.commit, hex(&row.commit));
        assert_eq!(line.snap, row.snap.to_string());
        assert_eq!(line.len, row.manifest_len);
        assert_eq!(line.hash, hex(&row.manifest_hash));
    }

    fn random_row(r: &mut Rng) -> SnapRow {
        SnapRow {
            cut: skyzfs_local::inplace::CommittedCut {
                state: r.bytes(),
                number: r.edgy(),
                group: r.edgy(),
            },
            commit: r.bytes(),
            snap: skynbd_types::SnapId(ulid::Ulid::from_bytes(r.bytes())),
            manifest_len: r.edgy() as u32,
            manifest_hash: r.bytes(),
        }
    }

    #[test]
    fn x_constants_and_keys_agree() {
        assert_eq!(UP_LEN, SNAP_ROW_LEN);
        assert_eq!(UP_PREFIX, SNAP_ROW_PREFIX);
        for name in ["t1", "a-b_c.d", "x"] {
            assert_eq!(snap_row_key(name), format!("{SNAP_ROW_PREFIX}{name}"));
        }
        let mut r = Rng(7);
        for _ in 0..200 {
            let vol = skynbd_types::VolumeId(ulid::Ulid::from_bytes(r.bytes()));
            let snap = skynbd_types::SnapId(ulid::Ulid::from_bytes(r.bytes()));
            assert_eq!(
                format!("{}skyzfs/manifest/{}", skynbd_engine::keys::volume_prefix(vol), snap.0),
                manifest_key(vol, snap)
            );
        }
    }

    /// Bytes laid out by hand from the documented table, each field
    /// distinct, read by both codecs to the values the table gives.
    #[test]
    fn x_golden_bytes_by_hand() {
        let mut b = vec![1u8, 0];
        b.extend(0x10u8..0x20); // state
        b.extend([0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]); // number, LE
        b.extend([0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18]); // group, LE
        b.extend(0x20u8..0x40); // commit
        b.extend(0x40u8..0x50); // snap id, big endian
        b.extend([0x50, 0x51, 0x52, 0x53]); // manifest len, LE
        b.extend(0x60u8..0x80); // hash
        assert_eq!(b.len(), 118);
        let ulid_be: [u8; 16] = std::array::from_fn(|i| 0x40 + i as u8);
        let want_snap = ulid::Ulid(u128::from_be_bytes(ulid_be)).to_string();
        let state: Vec<u8> = (0x10u8..0x20).collect();
        let commit: Vec<u8> = (0x20u8..0x40).collect();
        let hash: Vec<u8> = (0x60u8..0x80).collect();
        let (line, read) = snap_row_line(Some(&b));
        let l = parse_line(&line);
        assert_eq!(l.state, hex(&state));
        assert_eq!(l.number, 0x0807_0605_0403_0201);
        assert_eq!(l.group, 0x1817_1615_1413_1211);
        assert_eq!(l.commit, hex(&commit));
        assert_eq!(l.snap, want_snap);
        assert_eq!(l.len, 0x5352_5150);
        assert_eq!(l.hash, hex(&hash));
        let read = read.expect("reads");
        assert_eq!(read.snap.to_string(), want_snap);
        assert_eq!(read.manifest_len, 0x5352_5150);
        assert_eq!(read.manifest_hash.to_vec(), hash);
        let up = SnapRow::decode(&b).expect("the uploader reads it");
        same(&l, &up);
        assert_eq!(up.encode(), b);
    }

    /// Uploader encodes, node B decodes.
    #[test]
    fn x_random_rows_encoded_by_the_uploader() {
        let mut r = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..20_000 {
            let row = random_row(&mut r);
            let b = row.encode();
            let (line, read) = snap_row_line(Some(&b));
            same(&parse_line(&line), &row);
            let read = read.expect("reads");
            assert_eq!(read.snap, row.snap.0);
            assert_eq!(read.manifest_len, row.manifest_len);
            assert_eq!(read.manifest_hash, row.manifest_hash);
        }
    }

    /// Random bytes of version 1: node B's fields are the uploader's
    /// decode, and the uploader's encode gives the bytes back.
    #[test]
    fn x_random_bytes_read_by_both() {
        let mut r = Rng(12345);
        for _ in 0..20_000 {
            let mut b = r.vec(118);
            b[0] = 1;
            b[1] = 0;
            let up = SnapRow::decode(&b).expect("version 1, 118 bytes");
            assert_eq!(up.encode(), b);
            let (line, read) = snap_row_line(Some(&b));
            same(&parse_line(&line), &up);
            assert!(read.is_some());
        }
    }

    /// What node B refuses (unreadable) is what the uploader refuses, at
    /// any length and version; the unreadable line carries the bytes.
    #[test]
    fn x_refusals_agree() {
        let mut r = Rng(99);
        let versions = [0u16, 1, 2, 3, 0x100, 0x101, 0x0201, 0xffff];
        let mut n = 0;
        for len in 0..=300usize {
            for k in 0..(versions.len() + 4) {
                let mut b = r.vec(len);
                let v = versions.get(k).copied().unwrap_or(r.next() as u16);
                if len >= 2 {
                    b[..2].copy_from_slice(&v.to_le_bytes());
                }
                let up = SnapRow::decode(&b);
                let (line, read) = snap_row_line(Some(&b));
                assert_eq!(up.is_ok(), read.is_some(), "len {len} version {v:#x}: {line}");
                if up.is_err() {
                    assert_eq!(line, format!("snaprow\tunreadable\t{}", hex(&b)));
                }
                n += 1;
            }
        }
        assert!(n > 3000);
        assert_eq!(snap_row_line(None), ("snaprow\tnone".to_string(), None));
    }

    /// The manifest is judged against the row as the uploader made it
    /// (its length and BLAKE3, `Publishing::snapshot`).
    #[test]
    fn x_manifest_against_the_uploaders_row() {
        let mut r = Rng(4242);
        for _ in 0..2000 {
            let n = (r.next() % 3000) as usize;
            let m = r.vec(n);
            let mut row = random_row(&mut r);
            row.manifest_len = m.len() as u32;
            row.manifest_hash = *blake3::hash(&m).as_bytes();
            let (_, read) = snap_row_line(Some(&row.encode()));
            let read = read.expect("reads");
            assert!(manifest_line(&m, &read).ends_with("\tmatches"));
            let mut other = m.clone();
            if other.is_empty() {
                other.push(0);
            } else {
                let i = (r.next() as usize) % other.len();
                other[i] ^= 1 << (r.next() % 8);
            }
            assert!(manifest_line(&other, &read).ends_with("\tdiffers"));
            let mut longer = m.clone();
            longer.push(0);
            assert!(manifest_line(&longer, &read).ends_with("\tdiffers"));
        }
    }
}
