//! The cut's row (`skyzfs/cut`): a meta row of the volume's refs object
//! that names the cut head's commit holds, written in the same
//! transaction as the head swap, so that both land or neither (the notes,
//! "M2.4's shape").
//!
//! Version 1 is 34 bytes, little endian:
//!
//! | bytes | what |
//! |---|---|
//! | 0..2 | the version, 1 |
//! | 2..18 | the cut state's name (`skyzfs-local`'s `CutState::id`) |
//! | 18..26 | the cut's number in that state |
//! | 26..34 | the cut's transaction group |
//!
//! The version is read first: a row of another version is refused for
//! its version, whatever its length.

use skyzfs_local::inplace::CommittedCut;

/// The row's key among the refs object's meta rows.
pub const ROW_KEY: &str = "skyzfs/cut";

/// The version this uploader writes and reads.
pub const ROW_VERSION: u16 = 1;

/// The length of a row of version 1.
pub const ROW_LEN: usize = 2 + 16 + 8 + 8;

/// Why a row cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RowError {
    /// Of another version than [`ROW_VERSION`]: a newer daemon's, which
    /// this one does not overwrite.
    #[error("the cut's row is of version {0}; this uploader reads version {ROW_VERSION}")]
    Version(u16),
    /// Of version 1 with another length, or too short to hold a version.
    #[error("the cut's row is {0} bytes long; version {ROW_VERSION} is {ROW_LEN}")]
    Length(usize),
}

/// The row as the uploader read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// There is none: no cut was ever committed to the volume.
    Absent,
    /// It names a cut.
    Named(CommittedCut),
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
                Ok(cut) => Row::Named(cut),
                Err(e) => Row::Unreadable(e),
            },
        }
    }
}

/// The row that names `cut`.
pub fn encode(cut: &CommittedCut) -> Vec<u8> {
    let mut row = Vec::with_capacity(ROW_LEN);
    row.extend_from_slice(&ROW_VERSION.to_le_bytes());
    row.extend_from_slice(&cut.state);
    row.extend_from_slice(&cut.number.to_le_bytes());
    row.extend_from_slice(&cut.group.to_le_bytes());
    row
}

/// The cut a row names: its version first, then its length.
pub fn decode(bytes: &[u8]) -> Result<CommittedCut, RowError> {
    let version = match bytes {
        [a, b, ..] => u16::from_le_bytes([*a, *b]),
        _ => return Err(RowError::Length(bytes.len())),
    };
    if version != ROW_VERSION {
        return Err(RowError::Version(version));
    }
    let (Some(state), Some(number), Some(group)) = (
        bytes.get(2..18).and_then(|b| <[u8; 16]>::try_from(b).ok()),
        bytes.get(18..26).and_then(|b| <[u8; 8]>::try_from(b).ok()),
        bytes.get(26..34).and_then(|b| <[u8; 8]>::try_from(b).ok()),
    ) else {
        return Err(RowError::Length(bytes.len()));
    };
    if bytes.len() != ROW_LEN {
        return Err(RowError::Length(bytes.len()));
    }
    Ok(CommittedCut {
        state,
        number: u64::from_le_bytes(number),
        group: u64::from_le_bytes(group),
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

    #[test]
    fn a_row_reads_back_as_the_cut_it_names() {
        let row = encode(&CUT);
        assert_eq!(row.len(), ROW_LEN);
        assert_eq!(decode(&row), Ok(CUT));
        assert_eq!(Row::of(Some(&row)), Row::Named(CUT));
        assert_eq!(Row::of(None), Row::Absent);
    }

    /// The layout of version 1, byte by byte: a row another build wrote
    /// reads the same.
    #[test]
    fn a_row_of_version_1_is_laid_out_as_documented() {
        let row = encode(&CUT);
        assert_eq!(&row[0..2], &[1, 0]);
        assert_eq!(&row[2..18], b"a state's name..");
        assert_eq!(&row[18..26], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(
            &row[26..34],
            &[0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11]
        );
    }

    /// A row of another version is refused for its version, whatever its
    /// length; one of version 1 with another length for its length.
    #[test]
    fn a_row_of_another_version_or_length_is_unreadable() {
        let mut row = encode(&CUT);
        row[0] = 2;
        assert_eq!(decode(&row), Err(RowError::Version(2)));
        assert_eq!(decode(&[2, 0]), Err(RowError::Version(2)));
        assert_eq!(decode(&[0, 1, 9]), Err(RowError::Version(256)));
        let mut long = encode(&CUT);
        long.push(0);
        assert_eq!(decode(&long), Err(RowError::Length(ROW_LEN + 1)));
        let short = &encode(&CUT)[..ROW_LEN - 1];
        assert_eq!(decode(short), Err(RowError::Length(ROW_LEN - 1)));
        assert_eq!(decode(&[1]), Err(RowError::Length(1)));
        assert_eq!(decode(&[]), Err(RowError::Length(0)));
        assert_eq!(
            Row::of(Some(&[2, 0])),
            Row::Unreadable(RowError::Version(2))
        );
    }
}
