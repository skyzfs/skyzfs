//! Looking a snapshot up in object storage for the agent (work package
//! M2.6a: the channel's `look`, the plan's §3 and §8): what `snap/<t>`
//! is in the volume's refs object, read with no publish involved.
//!
//! The agent looks when how a publish ended does not say whether
//! `snap/<t>` was made, or says it and is to be checked (`done`,
//! `expired`, `withdrawn`, `taken`, a refusal of another attachment or
//! state, a daemon that went away), and compares what it [`Seen`] with
//! its own request:
//!
//! - [`Seen::Absent`]: no ref `snap/<t>`. A row left with no ref
//!   (SkyNBD deletes a snapshot's ref and not its row) is not read.
//! - [`Seen::Snap`]: the ref, and a row of version 1 that names the
//!   ref's commit: the cut published and the manifest's length and hash,
//!   as the publish recorded them.
//! - [`Seen::Unreadable`]: the ref, with no row, a row this crate does
//!   not read, or a row that names another commit (a ref deleted and
//!   made anew beside the row left behind). Whose it is cannot be told.
//!
//! The ref and the row are two reads of the refs object, not one. A
//! publish lands both in one transaction, so the reads see either both
//! or neither of it; a snapshot deleted and made anew between the two
//! reads is seen [`Seen::Unreadable`], never as the other's (this
//! module's reading: the design does not say how the agent looks). The
//! uploader looks a name up the same way before it publishes
//! ([`crate::EngineRemote`]).

use skynbd_refs::{RefError, RefStore};
use skynbd_types::{CommitHash, VolumeId};

use crate::publish::{snap_ref, snap_row_key, Found, SnapRow, SnapRowError};

/// What `snap/<t>` is ([module docs](self)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen {
    /// No ref `snap/<t>`.
    Absent,
    /// The ref, on `commit`, and the row that names it.
    Snap {
        /// The commit the ref names.
        commit: CommitHash,
        /// Its row.
        row: SnapRow,
    },
    /// The ref, on `commit`, with no row that can be read as its own.
    Unreadable {
        /// The commit the ref names.
        commit: CommitHash,
        /// Why the row is not read.
        why: Unread,
    },
}

/// Why a snapshot's row is not read as the ref's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unread {
    /// The ref has no row.
    NoRow,
    /// The row's bytes are not a row this crate reads.
    Row(SnapRowError),
    /// The row names this other commit.
    OtherCommit(CommitHash),
}

impl Seen {
    /// What a lookup's `found` shows.
    pub fn of(found: Option<Found>) -> Self {
        let Some(found) = found else {
            return Seen::Absent;
        };
        let commit = found.commit;
        let unreadable = |why| Seen::Unreadable { commit, why };
        let Some(bytes) = found.row else {
            return unreadable(Unread::NoRow);
        };
        match SnapRow::decode(&bytes) {
            Err(e) => unreadable(Unread::Row(e)),
            Ok(row) if row.commit != commit => unreadable(Unread::OtherCommit(row.commit)),
            Ok(row) => Seen::Snap { commit, row },
        }
    }
}

/// `snap/<name>` of `vol` in `refs`: the commit its ref names and its
/// row's bytes, or `None` with no ref.
pub async fn find<R: RefStore + ?Sized>(
    refs: &R,
    vol: VolumeId,
    name: &str,
) -> Result<Option<Found>, RefError> {
    let Some(value) = refs.get(vol, &snap_ref(name)).await? else {
        return Ok(None);
    };
    let row = refs.meta_get(vol, &snap_row_key(name)).await?;
    Ok(Some(Found {
        commit: value.commit,
        row,
    }))
}

/// What `snap/<name>` of `vol` is in `refs`. An error of the refs
/// object is not [`Seen::Absent`].
pub async fn look<R: RefStore + ?Sized>(
    refs: &R,
    vol: VolumeId,
    name: &str,
) -> Result<Seen, RefError> {
    Ok(Seen::of(find(refs, vol, name).await?))
}

#[cfg(test)]
mod tests;
