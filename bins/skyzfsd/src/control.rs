//! The daemon's channel (M2.5-2): how an agent on the node asks a serving
//! daemon for its count of cuts, asks it to publish a cut as the
//! snapshot `snap/<t>`, and looks the snapshot up (design §5.2, "A
//! snapshot is published by a cut of the pool's present history", and
//! §5.4). It is the daemon's first interface that is not a file. The rule
//! a publish follows, and what object storage holds once it is made, are
//! `skyzfs_upload::publish`'s (M2.5-1); this module carries the requests
//! to the uploader and its answers back. The protocol (version 1), its
//! limits and the client are `skyzfs_control`'s (M2.6a), so that the
//! agent does not link the daemon; they are re-exported here.
//!
//! # The socket
//!
//! A Unix stream socket, [`SOCKET`] in the volume's run directory, made
//! once the device serves, before the daemon prints `ready`, and removed
//! at the end of `serve`. A socket
//! left there by an instance that was killed is removed by the next one,
//! which holds the volume's lock, before it makes its own. Its mode is
//! 0600, and the daemon answers only a peer whose user is the daemon's
//! own or root (`SO_PEERCRED`): any other is refused (`peer`) and told
//! nothing else. One connection is answered at a time, on a thread of its
//! own, and one request per connection: the daemon answers and closes.
//! A connection whose request (the greeting, the request line and the
//! manifest) is not whole within [`REQUEST_LIMIT`] is closed, however
//! its bytes come (F3 of the review of #127), and the answer is written
//! with [`IO_LIMIT`] for each write.
//!
//! `status` says what the channel is (`control`: `uploads` where the
//! uploader answers it, `no_upload` where every request is refused
//! `no-upload`, `failed` where the socket could not be made) and what it
//! has done (`control_answered`, `control_peer`, `control_malformed`:
//! [`ChannelFigures`]), as of its last connection.
//!
//! # The protocol
//!
//! `skyzfs_control`'s documentation gives it: the requests, the answers
//! and the codes of a refusal. What each answer means of the uploader is
//! said there as well, as the daemon's.
//!
//! # What the daemon decides here, and the design does not
//!
//! The socket's place and mode, the peer's users, the limits and the
//! words of the protocol are this module's and `skyzfs_control`'s (the
//! design leaves the channel to M2.5; the lead's plan of M2.5 says "a
//! Unix socket per volume, protocol in M2.5-2's pull request"). A daemon
//! that cannot make its socket serves all the same and says so on
//! standard error: serving the volume comes before publishing it.

use std::io::{self, BufRead, BufReader, Read, Write};

use skyzfs_upload::publish::SnapRowError;
use skyzfs_upload::{Progress, Refusal, Seen};

pub use skyzfs_control::*;

/// The code of a publish's refusal.
pub fn code_of(refusal: &Refusal) -> Code {
    match refusal {
        Refusal::Name => Code::Name,
        Refusal::Attachment => Code::Attachment,
        Refusal::State => Code::State,
        Refusal::Manifest(_) => Code::Manifest,
        Refusal::Expiry(_) => Code::Expiry,
        Refusal::Asked => Code::Asked,
        Refusal::Busy => Code::Busy,
        Refusal::Taken => Code::Taken,
        Refusal::Stopped => Code::Stopped,
        Refusal::Rewound => Code::Rewound,
    }
}

/// The answer that says how a publish stands.
pub fn reply_of(progress: &Progress) -> Reply {
    match progress {
        Progress::Pending => Reply::Pending,
        Progress::Done(commit) => Reply::Done(*commit),
        Progress::Refused(why) => Reply::Refused(code_of(why), why.to_string()),
        Progress::Expired => Reply::Expired,
        Progress::Withdrawn => Reply::Withdrawn,
        Progress::Unknown => Reply::Unknown,
    }
}

/// The answer to `look` that says what was seen of `snap/T`.
pub fn seen_reply(seen: &Seen) -> Reply {
    match seen {
        Seen::Absent => Reply::Absent,
        Seen::Snap { commit, row } => Reply::Snap(SnapReply {
            commit: *commit,
            state: row.cut.state,
            number: row.cut.number,
            group: row.cut.group,
            manifest_len: row.manifest_len,
            manifest_hash: row.manifest_hash,
        }),
        Seen::Unreadable { commit, why } => Reply::Unreadable(UnreadableReply {
            commit: *commit,
            why: match why {
                skyzfs_upload::Unread::NoRow => Unread::NoRow,
                skyzfs_upload::Unread::Row(SnapRowError::Version(_)) => Unread::Version,
                skyzfs_upload::Unread::Row(SnapRowError::Length(_)) => Unread::Length,
                skyzfs_upload::Unread::OtherCommit(_) => Unread::OtherCommit,
            },
        }),
    }
}

/// What the daemon does with a request: given the request and, for
/// `publish`, its manifest, the answer.
pub type Handler = Box<dyn FnMut(&Request, Vec<u8>) -> Reply + Send>;

/// Answer one connection whose peer passed the check: read the greeting,
/// the request and its manifest, ask `handler`, and write the answer.
pub fn answer(stream: &mut (impl Read + Write), handler: &mut Handler) -> io::Result<Reply> {
    let mut reader = BufReader::new(&mut *stream);
    let reply = match read_request(&mut reader)? {
        Err(reply) => reply,
        Ok((request, manifest)) => handler(&request, manifest),
    };
    drop(reader);
    stream.write_all(format!("{HELLO}\n{}\n", reply.line()).as_bytes())?;
    stream.flush()?;
    Ok(reply)
}

/// The request and its manifest, or the refusal of a request that cannot
/// be taken.
fn read_request(reader: &mut impl BufRead) -> io::Result<Result<(Request, Vec<u8>), Reply>> {
    let hello = match read_line(reader) {
        Ok(line) => line,
        Err(e) if e.kind() == io::ErrorKind::InvalidData => String::new(),
        Err(e) => return Err(e),
    };
    if hello != HELLO {
        return Ok(Err(Reply::Refused(
            Code::Version,
            format!("this daemon speaks `{HELLO}`"),
        )));
    }
    let line = match read_line(reader) {
        Ok(line) => line,
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            return Ok(Err(Reply::Refused(Code::Malformed, e.to_string())))
        }
        Err(e) => return Err(e),
    };
    let request = match Request::parse(&line) {
        Ok(request) => request,
        Err(why) => return Ok(Err(Reply::Refused(Code::Malformed, why))),
    };
    // A name the uploader would refuse is refused here, before anything
    // reads or records it: the request log writes names into lines of
    // tab-separated columns (a minor point of the review of #127).
    let name = match &request {
        Request::Cuts => None,
        Request::Publish(p) => Some(&p.name),
        Request::Progress(name) | Request::Withdraw(name) | Request::Look(name) => Some(name),
    };
    if name.is_some_and(|name| !valid_name(name)) {
        return Ok(Err(Reply::Refused(Code::Name, Refusal::Name.to_string())));
    }
    let mut manifest = Vec::new();
    if let Request::Publish(p) = &request {
        if p.manifest > MANIFEST_MAX {
            return Ok(Err(Reply::Refused(
                Code::Manifest,
                Refusal::Manifest(p.manifest).to_string(),
            )));
        }
        if p.expiry.is_some_and(|e| e.is_zero() || e > EXPIRY_MAX) {
            let expiry = p.expiry.unwrap_or_default();
            return Ok(Err(Reply::Refused(
                Code::Expiry,
                Refusal::Expiry(expiry).to_string(),
            )));
        }
        manifest.resize(p.manifest, 0);
        reader.read_exact(&mut manifest)?;
    }
    Ok(Ok((request, manifest)))
}

pub mod handler;
#[cfg(target_os = "linux")]
pub mod server;
#[cfg(target_os = "linux")]
pub use server::{Channel, ChannelFigures, Post};

#[cfg(test)]
mod tests;
