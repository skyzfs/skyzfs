//! The cut's row (`skyzfs/cut`): a meta row of the volume's refs object
//! that names the cut head's commit holds, and that commit, written in
//! the same transaction as the head swap, so that both land or neither
//! (the notes, "M2.4's shape").
//!
//! Version 2 is 66 bytes, little endian:
//!
//! | bytes | what |
//! |---|---|
//! | 0..2 | the version, 2 |
//! | 2..18 | the cut state's name (`skyzfs-local`'s `CutState::id`) |
//! | 18..26 | the cut's number in that state |
//! | 26..34 | the cut's transaction group |
//! | 34..66 | the hash of the commit the head swap installed |
//!
//! The commit is what tells head is the row's (F1 of the review of
//! #122): another writer can move head to a commit of the cut's group (a
//! rollback to a snapshot of another cut with the same group, after a
//! rewind import), which a group cannot tell from the cut's own commit.
//!
//! Version 1 is the first 34 bytes of version 2, with 1 as the version,
//! and names no commit. This uploader writes version 2 only, and still
//! reads version 1, as a row that names its cut and no commit
//! ([`Named::commit`] is `None`); `check::judge` says what such a row
//! vouches for.
//!
//! The version is read first: a row of another version is refused for
//! its version, whatever its length.

use skynbd_types::CommitHash;
use skyzfs_local::inplace::CommittedCut;

/// The row's key among the refs object's meta rows.
pub const ROW_KEY: &str = "skyzfs/cut";

/// The version this uploader writes.
pub const ROW_VERSION: u16 = 2;

/// The length of a row of version 2.
pub const ROW_LEN: usize = 2 + 16 + 8 + 8 + 32;

/// The length of a row of version 1, which names no commit.
pub const ROW_V1_LEN: usize = 2 + 16 + 8 + 8;

/// Why a row cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RowError {
    /// Of a version this uploader does not read (neither 1 nor 2): a
    /// newer daemon's, which this one does not overwrite.
    #[error("the cut's row is of version {0}; this uploader reads versions 1 and {ROW_VERSION}")]
    Version(u16),
    /// Of version 1 or 2 with another length than that version's, or too
    /// short to hold a version.
    #[error(
        "the cut's row is {0} bytes long; version 1 is {ROW_V1_LEN} and version {ROW_VERSION} is {ROW_LEN}"
    )]
    Length(usize),
}

/// What a row names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Named {
    /// The cut head's commit holds.
    pub cut: CommittedCut,
    /// The commit the swap that wrote the row installed as head; `None`
    /// for a row of version 1, which names none.
    pub commit: Option<CommitHash>,
}

/// The row as the uploader read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// There is none: no cut was ever committed to the volume.
    Absent,
    /// It names a cut.
    Named(Named),
    /// It cannot be read ([`RowError`]). The uploader uploads nothing
    /// while it is there and never overwrites it.
    Unreadable(RowError),
}

impl Row {
    /// The row read from the refs object's bytes, `None` where there is
    /// none.
    pub fn of(bytes: Option<&[u8]>) -> Self {
        match bytes {
            None => Row::Absent,
            Some(bytes) => match decode(bytes) {
                Ok(named) => Row::Named(named),
                Err(e) => Row::Unreadable(e),
            },
        }
    }

    /// What the row names, where it names the cut `cut`.
    pub fn naming(&self, cut: &CommittedCut) -> Option<Named> {
        match self {
            Row::Named(named) if named.cut == *cut => Some(*named),
            _ => None,
        }
    }
}

/// The row, of version 2, that names `cut` and `commit`, the commit that
/// holds it.
pub fn encode(cut: &CommittedCut, commit: &CommitHash) -> Vec<u8> {
    let mut row = Vec::with_capacity(ROW_LEN);
    row.extend_from_slice(&ROW_VERSION.to_le_bytes());
    row.extend_from_slice(&cut.state);
    row.extend_from_slice(&cut.number.to_le_bytes());
    row.extend_from_slice(&cut.group.to_le_bytes());
    row.extend_from_slice(commit);
    row
}

/// What a row names: its version first, then its length.
pub fn decode(bytes: &[u8]) -> Result<Named, RowError> {
    let version = match bytes {
        [a, b, ..] => u16::from_le_bytes([*a, *b]),
        _ => return Err(RowError::Length(bytes.len())),
    };
    let len = match version {
        1 => ROW_V1_LEN,
        ROW_VERSION => ROW_LEN,
        _ => return Err(RowError::Version(version)),
    };
    if bytes.len() != len {
        return Err(RowError::Length(bytes.len()));
    }
    let (Some(state), Some(number), Some(group)) = (
        bytes.get(2..18).and_then(|b| <[u8; 16]>::try_from(b).ok()),
        bytes.get(18..26).and_then(|b| <[u8; 8]>::try_from(b).ok()),
        bytes.get(26..34).and_then(|b| <[u8; 8]>::try_from(b).ok()),
    ) else {
        return Err(RowError::Length(bytes.len()));
    };
    let commit = match version {
        1 => None,
        _ => match bytes.get(34..66).and_then(|b| CommitHash::try_from(b).ok()) {
            Some(commit) => Some(commit),
            None => return Err(RowError::Length(bytes.len())),
        },
    };
    Ok(Named {
        cut: CommittedCut {
            state,
            number: u64::from_le_bytes(number),
            group: u64::from_le_bytes(group),
        },
        commit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUT: CommittedCut = CommittedCut {
        state: *b"a state's name..",
        number: 0x0102_0304_0506_0708,
        group: 0x1112_1314_1516_1718,
    };

    const COMMIT: CommitHash = [
        0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e,
        0x2f, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d,
        0x3e, 0x3f,
    ];

    const NAMED: Named = Named {
        cut: CUT,
        commit: Some(COMMIT),
    };

    /// A row of version 1, as the uploaders before F1 wrote it.
    fn v1() -> Vec<u8> {
        let mut row = vec![1, 0];
        row.extend_from_slice(&CUT.state);
        row.extend_from_slice(&CUT.number.to_le_bytes());
        row.extend_from_slice(&CUT.group.to_le_bytes());
        row
    }

    #[test]
    fn a_row_reads_back_as_the_cut_and_the_commit_it_names() {
        let row = encode(&CUT, &COMMIT);
        assert_eq!(row.len(), ROW_LEN);
        assert_eq!(decode(&row), Ok(NAMED));
        assert_eq!(Row::of(Some(&row)), Row::Named(NAMED));
        assert_eq!(Row::of(None), Row::Absent);
    }

    /// The layout of version 2, byte by byte: a row another build wrote
    /// reads the same.
    #[test]
    fn a_row_of_version_2_is_laid_out_as_documented() {
        let row = encode(&CUT, &COMMIT);
        assert_eq!(&row[0..2], &[2, 0]);
        assert_eq!(&row[2..18], b"a state's name..");
        assert_eq!(&row[18..26], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(
            &row[26..34],
            &[0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11]
        );
        assert_eq!(&row[34..66], &COMMIT);
    }

    /// A row of version 1 still reads: its cut, and no commit.
    #[test]
    fn a_row_of_version_1_names_its_cut_and_no_commit() {
        assert_eq!(v1().len(), ROW_V1_LEN);
        let named = Named {
            cut: CUT,
            commit: None,
        };
        assert_eq!(decode(&v1()), Ok(named));
        assert_eq!(Row::of(Some(&v1())), Row::Named(named));
        assert_eq!(&encode(&CUT, &COMMIT)[2..34], &v1()[2..34]);
    }

    /// A row of another version is refused for its version, whatever its
    /// length; one of version 1 or 2 with another length for its length.
    #[test]
    fn a_row_of_another_version_or_length_is_unreadable() {
        let mut row = encode(&CUT, &COMMIT);
        row[0] = 3;
        assert_eq!(decode(&row), Err(RowError::Version(3)));
        assert_eq!(decode(&[3, 0]), Err(RowError::Version(3)));
        assert_eq!(decode(&[0, 1, 9]), Err(RowError::Version(256)));
        assert_eq!(decode(&[0, 0]), Err(RowError::Version(0)));
        let mut long = encode(&CUT, &COMMIT);
        long.push(0);
        assert_eq!(decode(&long), Err(RowError::Length(ROW_LEN + 1)));
        let short = &encode(&CUT, &COMMIT)[..ROW_LEN - 1];
        assert_eq!(decode(short), Err(RowError::Length(ROW_LEN - 1)));
        // A row of version 2 of version 1's length, and the other way.
        let mut cut_short = encode(&CUT, &COMMIT);
        cut_short.truncate(ROW_V1_LEN);
        assert_eq!(decode(&cut_short), Err(RowError::Length(ROW_V1_LEN)));
        let mut v1_long = encode(&CUT, &COMMIT);
        v1_long[0] = 1;
        assert_eq!(decode(&v1_long), Err(RowError::Length(ROW_LEN)));
        assert_eq!(decode(&[1]), Err(RowError::Length(1)));
        assert_eq!(decode(&[]), Err(RowError::Length(0)));
        assert_eq!(
            Row::of(Some(&[3, 0])),
            Row::Unreadable(RowError::Version(3))
        );
    }

    /// `naming` finds the row's cut by the cut alone, whatever commit the
    /// row names, or none.
    #[test]
    fn a_row_naming_the_cut_is_found_by_the_cut() {
        let row = Row::Named(NAMED);
        assert_eq!(row.naming(&CUT), Some(NAMED));
        let other = CommittedCut {
            number: CUT.number + 1,
            ..CUT
        };
        assert_eq!(row.naming(&other), None);
        assert_eq!(
            Row::of(Some(&v1())).naming(&CUT),
            Some(Named {
                cut: CUT,
                commit: None
            })
        );
        assert_eq!(Row::Absent.naming(&CUT), None);
        assert_eq!(Row::Unreadable(RowError::Version(3)).naming(&CUT), None);
    }
}
