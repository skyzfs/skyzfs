//! The channel between a node's agent and a serving `skyzfsd` (work
//! packages M2.5-2 and M2.6a): the protocol's words and limits, and the
//! client that asks ([`ask`]). The daemon's side, which takes the
//! connections and answers them, is `skyzfsd::control`'s. This crate
//! holds nothing of the daemon's (no ublk binding, no async runtime, no
//! client of object storage), so that a program that only asks, the
//! agent of M2.6, links none of it.
//!
//! The daemon listens on a Unix stream socket, [`SOCKET`] in the
//! volume's run directory ([`socket_path`]), of mode 0600, and answers only a peer whose
//! user is its own or root; it answers one request per connection and
//! closes. A request whose client has closed its connection (both ways)
//! by the time the daemon reads it whole is dropped, not handled and not
//! answered, so that nothing is done for a client that stopped waiting;
//! a client that only shut its writing down after its request is
//! answered.
//!
//! The limits a request is held to ([`valid_name`], [`MANIFEST_MAX`],
//! [`EXPIRY_MAX`], and [`EXPIRY_DEFAULT`] where it names no expiry) are
//! the protocol's: the uploader's rule of a publish
//! (`skyzfs_upload::publish`) applies these same ones, from here.
//!
//! # The protocol, version 3
//!
//! Version 3 (work package M2.7b) is version 2 with the verb `retire`,
//! its answers `retired` and `gone`, the refusal code `other`, and the
//! store's rewind mark in the answer to `cuts` (`rewound=R`, which it
//! must carry). Every other line of version 2 reads the same in version
//! 3, and the greeting changed, as it did for version 2 (below). **A daemon and an agent
//! are deployed together**: each refuses a peer of version 2 at the
//! greeting (`version`), the agent saying which versions met, and a
//! node runs no publish and no retention until both speak version 3.
//!
//! Version 2 was version 1 with the verb `look`, its answers `snap`,
//! `absent` and `unreadable`, and the refusal codes `rewound` and
//! `store`. Each line of version 1 reads the same in version 2. The
//! greeting changed all the same (CONTRIBUTING, "Formats are
//! versioned"): a reader of version 1 takes a word it does not know for
//! an answer that does not parse, an error and not "not said", so the
//! exception for a new code does not hold. A peer of the other version
//! is refused at the greeting (`version`), before any word it cannot
//! read.
//!
//! Lines end with `\n` and are UTF-8, at most [`LINE_MAX`] bytes. The
//! client sends the greeting [`HELLO`], one request line, and for
//! `publish` exactly the manifest's bytes after it. The daemon answers
//! with the greeting and one answer line, and closes. A line is words
//! separated by one space: the verb, then `key=value` pairs, each key at
//! most once and in any order; a key the verb does not take, a missing
//! one, or a value that does not parse is `malformed`.
//!
//! | Request | Answers |
//! |---|---|
//! | `cuts` | `cuts attachment=A state=S count=C group=G opens=O rewound=R` |
//! | `publish name=T attachment=A state=S after=C group=G manifest=N [expiry-s=E]`, then N bytes | `accepted` |
//! | `progress name=T` | `pending`, `done commit=H`, `expired`, `withdrawn`, `unknown` |
//! | `withdraw name=T` | as `progress`, as the publish stands after the withdrawal |
//! | `look name=T` | `absent`, `snap commit=H state=S number=N group=G manifest-len=L manifest-hash=X`, `unreadable commit=H why=W` |
//! | `retire name=T commit=H` | `retired`, `gone` |
//!
//! Any request can be answered `refused CODE TEXT`: CODE is one word
//! ([`Code`]) and TEXT says why in words, for a person.
//!
//! - `cuts`: the attachment this daemon serves (A, 32 hex digits: the
//!   attachment record's nonce, fresh at each attach afresh and kept by
//!   a take-over), the store's cut state (S, 32 hex digits, or `none`
//!   where the store holds no cuts), the cuts counted in that state (C:
//!   the newest cut's number, 0 before the first), and the newest cut's
//!   transaction group and whether it opens as itself (G a number and O
//!   `itself` or `rewind`, or both `none` before the first cut), and
//!   the store's rewind mark (R, version 3: the number of the newest cut
//!   that may follow a rewind, or the count the daemon started with,
//!   whichever is higher; 0 where neither is). A `publish` asked after a
//!   count below R is refused `rewound`, and each later cut at or under
//!   the highest group the daemon has seen moves R. So a newest cut
//!   that opens as itself with C above R is above every group seen
//!   before it, and a snapshot taken then can be published (design
//!   §5.2: the agent syncs a pool after a rewind import until it is).
//!   The count is the store's when the request is answered: a cut
//!   counted before the request was sent is in it.
//! - `publish`: publish as `snap/T` the first cut of state S counted
//!   after C whose group is G or later and that opens as itself, with
//!   the ZFS manifest of N bytes (at most 1 MiB, opaque to the daemon),
//!   pending at most E seconds (default 600, at most 3600). The agent
//!   reads A, S and C with `cuts` before it takes `pool@T`, and G is
//!   the snapshot's `createtxg`. A request of another attachment or
//!   state is refused. `accepted` says that it pends; how it ends is
//!   read with `progress`. Asking twice is asking once.
//!
//!   The E seconds are counted from the acceptance on this daemon's
//!   uploader: asked again while it pends, a publish keeps the deadline
//!   it had. Asked of the next daemon (after a restart), it is a request
//!   of that daemon, judged by the one rule below, and has a new deadline
//!   if it is accepted. So an agent that holds `pool@T` while its publish
//!   pends counts its hold from its last request answered `accepted`,
//!   not its first (q11 of the review of #122).
//!
//!   `refused rewound`, to a `publish` or as its `progress` (work
//!   package M2.6a). One rule: a request whose C is below the count this
//!   daemon started with, or below the live rewind mark, is refused
//!   `rewound`, when it is asked or later, while it waits for its cut,
//!   once the mark passes C. The mark is the number of the newest cut
//!   whose group is not above the highest group this daemon has seen
//!   since its start, as the first groups after `zpool import -F` or `-T`
//!   are; it never goes back. A cut that carries such a request may be
//!   of a history without `pool@T`. Nothing is published. The agent
//!   checks `pool@T` and asks again with a count read anew. A publish
//!   already given its cut when the mark passes its C is not ended so
//!   (`skyzfs_upload::publish`).
//! - `progress`: how the publish of T stands. `done` names the commit
//!   (H, 64 hex digits) that `snap/T` names. `unknown`: not asked of
//!   this daemon's uploader, or its end has left the 1024 kept.
//! - `withdraw`: end the publish of T, unless it is made already. One
//!   whose transaction may have landed is looked up first, and so may
//!   still read `pending` and then `done`. While it still pends so, the
//!   same `publish` asked again undoes the withdrawal, and its `accepted`
//!   holds: the publish goes on (D1 of the review of #122).
//! - `look` (work package M2.6a): what `snap/T` is in object storage,
//!   read there when asked (`skyzfs_upload::look`), whatever this
//!   daemon's uploader knows of it. `absent`: no ref `snap/T`. `snap`:
//!   the ref names commit H and has the row of H, which records the cut
//!   published (its state S, number N and group G) and the ZFS
//!   manifest's length L and BLAKE3 hash X (64 hex digits); the agent
//!   compares them with its request. `unreadable`: the ref names H and
//!   has no row read as its own, W saying why: `no-row`, `version` (a
//!   row of another version), `length` (of version 1 and another
//!   length), or `other-commit` (a row of another commit, beside a ref
//!   deleted and made anew). The daemon looks with a client of object
//!   storage of the channel's own, and not through its uploader, so a
//!   look is answered while the uploader steps and after it has stopped
//!   or ended; `refused store` says that object storage answered with
//!   an error, and `no-answer` that it did not answer within
//!   [`ANSWER_LIMIT`]. Neither is `absent`. A daemon with no uploader
//!   refuses `look` `no-upload` as it does the other requests.
//! - `retire` (work package M2.7b, the agent's retention): delete
//!   `snap/T` in object storage where it names commit H (64 hex
//!   digits), with its row, in one transaction that compares the ref
//!   with H. `retired`: it named H, and the ref and its row are gone.
//!   `gone`: there is no `snap/T`, so asking again after a `retired`
//!   that went unheard reads `gone`; the agent counts both as done.
//!   `refused other`: `snap/T` names another commit, given in the text,
//!   and nothing is deleted. The commit's blocks and the snapshot's
//!   manifest stay in object storage until garbage collection (M4)
//!   finds that no ref reaches them. The request goes through the
//!   uploader and not `look`'s client, so that its lease and its fence
//!   hold for deleting a ref as for making one: it is answered after a
//!   step of its own, `no-answer` while the uploader is in a long step
//!   (the transaction may still land: ask again), and `refused stopped`
//!   once the uploader steps no more. A retire of a name whose publish
//!   pends on this uploader is refused `asked`.
//!
//! A name T that no publish can have
//! ([`valid_name`]) is refused `name` by each of
//! `publish`, `progress`, `withdraw`, `look` and `retire`, before the
//! request goes further.
//!
//! `refused stopped`, to a `publish` or as a publish's `progress`: this
//! daemon's uploader steps no more (its lease lost, a commit or a publish
//! refused by the fence, the store keeping a cut). A publish that pended
//! when it stopped ends so even where its transaction may have landed
//! unheard. `stopped` therefore says "ask the next daemon", which looks
//! `snap/T` up, and not "not published" (q10 of the review of #122).
//!
//! A refusal of the channel itself, with no publish involved: `version`
//! (the greeting is not [`HELLO`]), `malformed`, `peer`, `no-upload`
//! (this daemon has no uploader, and the text says why), `ended` (its
//! uploader has ended), and `no-answer`: the uploader did not answer
//! within [`ANSWER_LIMIT`], since it answers between two of its steps
//! and a step can wait on object storage. After `no-answer` a `publish`
//! may still have been taken: ask again, which answers as asking once,
//! or withdraw it. For `cuts`, which the store's queue answers and not
//! the uploader, `ended` says that the store is no longer served, and
//! `no-answer` that its queue took no turn within the limit. A `publish`
//! goes there first, for the store's rewind mark, which goes with it to
//! the uploader (B3 of the review of #140): it can be answered either
//! of those before the uploader sees it, and its two waits share the
//! one [`ANSWER_LIMIT`]. For `look`,
//! `no-answer` and `store` are object storage's (above). The other codes
//! are a publish's refusals (`skyzfs_upload::Refusal`).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::fmt::Write as _;
use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(target_os = "linux")]
mod client;
#[cfg(target_os = "linux")]
pub use client::ask;

/// The largest ZFS manifest a request carries: 1 MiB. Its format is the
/// agent's (M2.6); the daemon stores it as it came.
pub const MANIFEST_MAX: usize = 1 << 20;

/// How long a publish may pend where the agent names no expiry: ten
/// minutes. The daemon's channel fills it in.
pub const EXPIRY_DEFAULT: Duration = Duration::from_secs(10 * 60);

/// The longest a publish may pend: an hour.
pub const EXPIRY_MAX: Duration = Duration::from_secs(60 * 60);

/// Whether `name` may name a snapshot: 1 to 128 of `[A-Za-z0-9._-]`, as
/// SkyNBD's engine requires of a snapshot's name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

/// The socket's name in the volume's run directory ([`socket_path`]).
pub const SOCKET: &str = "control";

/// The directory, under `run_dir`, of the volume whose directory is
/// `volume_dir`: `v-<dev>-<ino>`, the device and inode numbers of the
/// volume's directory in hex. The run directory (`skyzfsd serve
/// --run-dir`) is shared by every daemon on the node, so a daemon keeps
/// what is its volume's own in this directory, its channel's
/// [`SOCKET`] among it. The daemon's records name it with this function
/// (`skyzfs_ublk::records::volume_run_dir` is this one), so that an agent
/// given the same run directory and volume finds the same socket. The
/// volume's directory is read (`stat`, through a symbolic link): any
/// path to it names the same directory. Unix only: it names the
/// directory by its device and inode numbers.
#[cfg(unix)]
pub fn volume_run_dir(run_dir: &Path, volume_dir: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::metadata(volume_dir)?;
    Ok(run_dir.join(format!("v-{:x}-{:x}", meta.dev(), meta.ino())))
}

/// The channel of the volume whose directory is `volume_dir`, served by a
/// daemon whose run directory is `run_dir`: [`SOCKET`] in
/// [`volume_run_dir`].
#[cfg(unix)]
pub fn socket_path(run_dir: &Path, volume_dir: &Path) -> io::Result<PathBuf> {
    Ok(volume_run_dir(run_dir, volume_dir)?.join(SOCKET))
}

/// The greeting of version 3, the first line each way.
pub const HELLO: &str = "skyzfs-control 3";

/// The longest line, its `\n` included.
pub const LINE_MAX: usize = 4096;

/// How long either side waits for the other's next bytes.
pub const IO_LIMIT: Duration = Duration::from_secs(10);

/// How long the daemon waits for a whole request, its manifest
/// included: one deadline for the connection's reads, so that a peer that
/// sends a byte now and then holds the channel's thread no longer than
/// this (F3 of the review of #127). A request of the command line's
/// client is written at once, a manifest of at most 1 MiB with it.
pub const REQUEST_LIMIT: Duration = IO_LIMIT;

/// How long the daemon waits for its uploader's answer: it answers
/// between two of its steps, and a step can wait on object storage.
pub const ANSWER_LIMIT: Duration = Duration::from_secs(30);

/// How long a client waits for the whole of an ask (one deadline,
/// [`ask`]; S4 of the review of #131): the daemon may wait
/// [`ANSWER_LIMIT`] for its uploader, and [`IO_LIMIT`] more for the
/// connection. A daemon that answers one connection at a time and is
/// busy with another past this is no answer, as a daemon away is.
pub const ASK_LIMIT: Duration = Duration::from_secs(ANSWER_LIMIT.as_secs() + IO_LIMIT.as_secs());

/// A request ([crate docs](crate)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// `cuts`.
    Cuts,
    /// `publish`; its manifest's bytes follow the line.
    Publish(Publish),
    /// `progress name=T`.
    Progress(String),
    /// `withdraw name=T`.
    Withdraw(String),
    /// `look name=T`.
    Look(String),
    /// `retire name=T commit=H`.
    Retire(Retire),
}

/// What `retire` names: the published snapshot to delete in object
/// storage, and the commit its ref must name for it to be deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retire {
    /// The snapshot's name, T.
    pub name: String,
    /// The commit `snap/T` names, H, as the publish's `done` said it.
    pub commit: [u8; 32],
}

/// What `publish` names. Its manifest travels after the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publish {
    /// The snapshot's name, T.
    pub name: String,
    /// The attachment the count was read in, A.
    pub attachment: [u8; 16],
    /// The cut state the count is of, S.
    pub state: [u8; 16],
    /// The count read before the snapshot was taken, C.
    pub after: u64,
    /// The snapshot's `createtxg`, G.
    pub group: u64,
    /// The manifest's length in bytes, N.
    pub manifest: usize,
    /// How long it may pend, E; the daemon's default where `None`.
    pub expiry: Option<Duration>,
}

/// An answer ([crate docs](crate)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// The answer to `cuts`.
    Cuts(CutsReply),
    /// A publish pends.
    Accepted,
    /// Accepted and not ended.
    Pending,
    /// Published: `snap/T` names this commit.
    Done([u8; 32]),
    /// Not made in its time.
    Expired,
    /// Withdrawn before it was made.
    Withdrawn,
    /// Not asked of this uploader, or its end has left the view.
    Unknown,
    /// `look`: no `snap/T`.
    Absent,
    /// `look`: `snap/T` and the row of its commit.
    Snap(SnapReply),
    /// `look`: `snap/T` with no row read as its own.
    Unreadable(UnreadableReply),
    /// `retire`: `snap/T` named the commit, and it and its row are
    /// deleted.
    Retired,
    /// `retire`: there is no `snap/T`.
    Gone,
    /// Refused, and why.
    Refused(Code, String),
}

/// The answer to `look` where `snap/T` has its row: what the publish
/// recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapReply {
    /// The commit `snap/T` names, H.
    pub commit: [u8; 32],
    /// The cut state of the cut published, S.
    pub state: [u8; 16],
    /// The cut's number in that state, N.
    pub number: u64,
    /// The cut's transaction group, G.
    pub group: u64,
    /// The ZFS manifest's length, L.
    pub manifest_len: u32,
    /// Its BLAKE3 hash, X.
    pub manifest_hash: [u8; 32],
}

/// The answer to `look` where `snap/T` has no row read as its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnreadableReply {
    /// The commit `snap/T` names, H.
    pub commit: [u8; 32],
    /// Why its row is not read, W.
    pub why: Unread,
}

/// Why `snap/T`'s row is not read as its own: one word on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unread {
    /// `no-row`: there is none.
    NoRow,
    /// `version`: a row of another version than 1.
    Version,
    /// `length`: a row of version 1 of another length.
    Length,
    /// `other-commit`: a row that names another commit than the ref's.
    OtherCommit,
}

const UNREADS: [(Unread, &str); 4] = [
    (Unread::NoRow, "no-row"),
    (Unread::Version, "version"),
    (Unread::Length, "length"),
    (Unread::OtherCommit, "other-commit"),
];

impl Unread {
    /// The word of why.
    pub fn word(self) -> &'static str {
        UNREADS
            .iter()
            .find(|(why, _)| *why == self)
            .map_or("no-row", |(_, word)| word)
    }

    fn from_word(word: &str) -> Option<Self> {
        UNREADS
            .iter()
            .find(|(_, w)| *w == word)
            .map(|(why, _)| *why)
    }
}

/// The answer to `cuts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutsReply {
    /// The attachment this daemon serves.
    pub attachment: [u8; 16],
    /// The store's cut state; `None` where it holds no cuts.
    pub state: Option<[u8; 16]>,
    /// The cuts counted in it.
    pub count: u64,
    /// The newest cut's group, and whether it opens as itself.
    pub newest: Option<(u64, bool)>,
    /// The store's rewind mark (version 3): the number of the newest cut
    /// that may follow a rewind, or the count the daemon started with if
    /// that is higher. A publish asked after a lower count is refused
    /// `rewound`.
    pub rewound: u64,
}

/// Why a request is refused: one word on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// The name is not 1 to 128 of `[A-Za-z0-9._-]`.
    Name,
    /// Another attachment than this daemon's.
    Attachment,
    /// Another cut state than the store's.
    State,
    /// A manifest over 1 MiB.
    Manifest,
    /// An expiry of zero or over an hour.
    Expiry,
    /// The name pends with other parameters; to `retire`, a publish of
    /// the name pends on this daemon.
    Asked,
    /// Too many publishes pend.
    Busy,
    /// `snap/T` exists and does not record this request: it names
    /// another commit, or its row records another cut or manifest.
    Taken,
    /// The uploader publishes nothing more, and retires nothing more.
    Stopped,
    /// A rewind may have been counted after the request's count.
    Rewound,
    /// The greeting is not this version's.
    Version,
    /// The request does not parse.
    Malformed,
    /// The peer's user is neither the daemon's nor root.
    Peer,
    /// The daemon has no uploader.
    NoUpload,
    /// What answers the request has ended: the uploader, for a publish,
    /// its progress, its withdrawal or a retire (whose transaction may
    /// have landed: ask again); for `cuts`, the store's queue, whose desk
    /// is closed once the store is no longer served.
    Ended,
    /// What answers the request did not answer within the limit: the
    /// uploader, for a publish, its progress, its withdrawal or a retire
    /// (taken at its next turn, so it may still be made: ask again); for
    /// `cuts`, the store's queue, which gives the count between two of
    /// its turns; for `look`, object storage.
    NoAnswer,
    /// Object storage answered `look` or `retire` with an error (a
    /// retire's transaction may have landed: ask again), or a retire's
    /// ref moved at each of its tries.
    Store,
    /// `retire`: `snap/T` names another commit than the request's.
    Other,
}

const CODES: [(Code, &str); 18] = [
    (Code::Name, "name"),
    (Code::Attachment, "attachment"),
    (Code::State, "state"),
    (Code::Manifest, "manifest"),
    (Code::Expiry, "expiry"),
    (Code::Asked, "asked"),
    (Code::Busy, "busy"),
    (Code::Taken, "taken"),
    (Code::Stopped, "stopped"),
    (Code::Rewound, "rewound"),
    (Code::Version, "version"),
    (Code::Malformed, "malformed"),
    (Code::Peer, "peer"),
    (Code::NoUpload, "no-upload"),
    (Code::Ended, "ended"),
    (Code::NoAnswer, "no-answer"),
    (Code::Store, "store"),
    (Code::Other, "other"),
];

impl Code {
    /// The code's word.
    pub fn word(self) -> &'static str {
        CODES
            .iter()
            .find(|(code, _)| *code == self)
            .map_or("malformed", |(_, word)| word)
    }

    fn from_word(word: &str) -> Option<Self> {
        CODES
            .iter()
            .find(|(_, w)| *w == word)
            .map(|(code, _)| *code)
    }
}

impl Reply {
    /// Whether it is a refusal.
    pub fn refused(&self) -> bool {
        matches!(self, Reply::Refused(..))
    }
}

/// `bytes` in lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// `N` bytes from exactly `2 * N` hex digits.
pub fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != 2 * N || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

/// The `key=value` pairs after a verb, each key once and only `keys`.
fn pairs<'a>(words: &[&'a str], keys: &[&str]) -> Result<Vec<(&'a str, &'a str)>, String> {
    let mut out: Vec<(&str, &str)> = Vec::new();
    for word in words {
        let (key, value) = word
            .split_once('=')
            .ok_or_else(|| format!("`{word}` is not key=value"))?;
        if !keys.contains(&key) {
            return Err(format!("`{key}` is not a key here"));
        }
        if out.iter().any(|(k, _)| *k == key) {
            return Err(format!("`{key}` is given twice"));
        }
        out.push((key, value));
    }
    Ok(out)
}

fn take<'a>(pairs: &[(&str, &'a str)], key: &str) -> Result<&'a str, String> {
    pairs
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .ok_or_else(|| format!("`{key}` is missing"))
}

fn number(pairs: &[(&str, &str)], key: &str) -> Result<u64, String> {
    let text = take(pairs, key)?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("`{key}` is not a number: `{text}`"));
    }
    text.parse()
        .map_err(|_| format!("`{key}` is out of range: `{text}`"))
}

fn id(pairs: &[(&str, &str)], key: &str) -> Result<[u8; 16], String> {
    let text = take(pairs, key)?;
    unhex(text).ok_or_else(|| format!("`{key}` is not 32 hex digits: `{text}`"))
}

fn hash(pairs: &[(&str, &str)], key: &str) -> Result<[u8; 32], String> {
    let text = take(pairs, key)?;
    unhex(text).ok_or_else(|| format!("`{key}` is not 64 hex digits: `{text}`"))
}

/// Split a line into its words: one space between, none at either end.
fn words(line: &str) -> Result<Vec<&str>, String> {
    let words: Vec<&str> = line.split(' ').collect();
    if words.iter().any(|w| w.is_empty()) {
        return Err("words are separated by one space".into());
    }
    Ok(words)
}

impl Request {
    /// Parse a request line (without its `\n`).
    pub fn parse(line: &str) -> Result<Self, String> {
        let words = words(line)?;
        let (verb, rest) = words.split_first().ok_or("an empty line")?;
        match *verb {
            "cuts" => {
                pairs(rest, &[])?;
                Ok(Request::Cuts)
            }
            "publish" => {
                let p = pairs(
                    rest,
                    &[
                        "name",
                        "attachment",
                        "state",
                        "after",
                        "group",
                        "manifest",
                        "expiry-s",
                    ],
                )?;
                let manifest = number(&p, "manifest")?;
                let expiry = match p.iter().any(|(k, _)| *k == "expiry-s") {
                    true => Some(Duration::from_secs(number(&p, "expiry-s")?)),
                    false => None,
                };
                Ok(Request::Publish(Publish {
                    name: take(&p, "name")?.to_string(),
                    attachment: id(&p, "attachment")?,
                    state: id(&p, "state")?,
                    after: number(&p, "after")?,
                    group: number(&p, "group")?,
                    manifest: usize::try_from(manifest)
                        .map_err(|_| format!("`manifest` is out of range: {manifest}"))?,
                    expiry,
                }))
            }
            "progress" => Ok(Request::Progress(
                take(&pairs(rest, &["name"])?, "name")?.to_string(),
            )),
            "withdraw" => Ok(Request::Withdraw(
                take(&pairs(rest, &["name"])?, "name")?.to_string(),
            )),
            "look" => Ok(Request::Look(
                take(&pairs(rest, &["name"])?, "name")?.to_string(),
            )),
            "retire" => {
                let p = pairs(rest, &["name", "commit"])?;
                Ok(Request::Retire(Retire {
                    name: take(&p, "name")?.to_string(),
                    commit: hash(&p, "commit")?,
                }))
            }
            other => Err(format!("`{other}` is not a request")),
        }
    }

    /// The request's line, without its `\n`.
    pub fn line(&self) -> String {
        match self {
            Request::Cuts => "cuts".into(),
            Request::Publish(p) => {
                let mut line = format!(
                    "publish name={} attachment={} state={} after={} group={} manifest={}",
                    p.name,
                    hex(&p.attachment),
                    hex(&p.state),
                    p.after,
                    p.group,
                    p.manifest
                );
                if let Some(expiry) = p.expiry {
                    let _ = write!(line, " expiry-s={}", expiry.as_secs());
                }
                line
            }
            Request::Progress(name) => format!("progress name={name}"),
            Request::Withdraw(name) => format!("withdraw name={name}"),
            Request::Look(name) => format!("look name={name}"),
            Request::Retire(r) => format!("retire name={} commit={}", r.name, hex(&r.commit)),
        }
    }
}

impl Reply {
    /// Parse an answer line (without its `\n`).
    pub fn parse(line: &str) -> Result<Self, String> {
        if let Some(rest) = line.strip_prefix("refused ") {
            let (word, text) = rest.split_once(' ').unwrap_or((rest, ""));
            let code = Code::from_word(word).ok_or_else(|| format!("`{word}` is not a code"))?;
            return Ok(Reply::Refused(code, text.to_string()));
        }
        let words = words(line)?;
        let (verb, rest) = words.split_first().ok_or("an empty line")?;
        let bare = |reply: Reply| pairs(rest, &[]).map(|_| reply);
        match *verb {
            "cuts" => {
                let p = pairs(
                    rest,
                    &["attachment", "state", "count", "group", "opens", "rewound"],
                )?;
                let state = match take(&p, "state")? {
                    "none" => None,
                    _ => Some(id(&p, "state")?),
                };
                let newest = match (take(&p, "group")?, take(&p, "opens")?) {
                    ("none", "none") => None,
                    (_, "itself") => Some((number(&p, "group")?, true)),
                    (_, "rewind") => Some((number(&p, "group")?, false)),
                    (group, opens) => {
                        return Err(format!("`group={group} opens={opens}` do not go together"))
                    }
                };
                Ok(Reply::Cuts(CutsReply {
                    attachment: id(&p, "attachment")?,
                    state,
                    count: number(&p, "count")?,
                    newest,
                    rewound: number(&p, "rewound")?,
                }))
            }
            "accepted" => bare(Reply::Accepted),
            "pending" => bare(Reply::Pending),
            "expired" => bare(Reply::Expired),
            "withdrawn" => bare(Reply::Withdrawn),
            "unknown" => bare(Reply::Unknown),
            "absent" => bare(Reply::Absent),
            "retired" => bare(Reply::Retired),
            "gone" => bare(Reply::Gone),
            "done" => {
                let p = pairs(rest, &["commit"])?;
                Ok(Reply::Done(hash(&p, "commit")?))
            }
            "snap" => {
                let p = pairs(
                    rest,
                    &[
                        "commit",
                        "state",
                        "number",
                        "group",
                        "manifest-len",
                        "manifest-hash",
                    ],
                )?;
                let len = number(&p, "manifest-len")?;
                Ok(Reply::Snap(SnapReply {
                    commit: hash(&p, "commit")?,
                    state: id(&p, "state")?,
                    number: number(&p, "number")?,
                    group: number(&p, "group")?,
                    manifest_len: u32::try_from(len)
                        .map_err(|_| format!("`manifest-len` is out of range: {len}"))?,
                    manifest_hash: hash(&p, "manifest-hash")?,
                }))
            }
            "unreadable" => {
                let p = pairs(rest, &["commit", "why"])?;
                let word = take(&p, "why")?;
                let why = Unread::from_word(word)
                    .ok_or_else(|| format!("`why` is not a reason: `{word}`"))?;
                Ok(Reply::Unreadable(UnreadableReply {
                    commit: hash(&p, "commit")?,
                    why,
                }))
            }
            other => Err(format!("`{other}` is not an answer")),
        }
    }

    /// The answer's line, without its `\n`. A refusal's text is kept to
    /// one line.
    pub fn line(&self) -> String {
        match self {
            Reply::Cuts(c) => {
                let state = c.state.map_or_else(|| "none".into(), |s| hex(&s));
                let (group, opens) = match c.newest {
                    None => ("none".to_string(), "none"),
                    Some((group, itself)) => {
                        (group.to_string(), if itself { "itself" } else { "rewind" })
                    }
                };
                format!(
                    "cuts attachment={} state={state} count={} group={group} opens={opens} \
                     rewound={}",
                    hex(&c.attachment),
                    c.count,
                    c.rewound
                )
            }
            Reply::Accepted => "accepted".into(),
            Reply::Pending => "pending".into(),
            Reply::Done(commit) => format!("done commit={}", hex(commit)),
            Reply::Expired => "expired".into(),
            Reply::Withdrawn => "withdrawn".into(),
            Reply::Unknown => "unknown".into(),
            Reply::Absent => "absent".into(),
            Reply::Retired => "retired".into(),
            Reply::Gone => "gone".into(),
            Reply::Snap(r) => format!(
                "snap commit={} state={} number={} group={} manifest-len={} manifest-hash={}",
                hex(&r.commit),
                hex(&r.state),
                r.number,
                r.group,
                r.manifest_len,
                hex(&r.manifest_hash)
            ),
            Reply::Unreadable(r) => {
                format!("unreadable commit={} why={}", hex(&r.commit), r.why.word())
            }
            Reply::Refused(code, text) => {
                let text: String = text
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect();
                let mut line = format!("refused {} {text}", code.word());
                if line.len() >= LINE_MAX {
                    let mut end = LINE_MAX - 1;
                    while !line.is_char_boundary(end) {
                        end -= 1;
                    }
                    line.truncate(end);
                }
                line
            }
        }
    }
}

/// Read one line of at most [`LINE_MAX`] bytes, its `\n` included, and
/// give it without the `\n`.
pub fn read_line(reader: &mut impl BufRead) -> io::Result<String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(LINE_MAX as u64)
        .read_until(b'\n', &mut bytes)?;
    if bytes.pop() != Some(b'\n') {
        if bytes.len() + 1 < LINE_MAX {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the connection ended inside a line",
            ));
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("no whole line within {LINE_MAX} bytes"),
        ));
    }
    String::from_utf8(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"))
}

#[cfg(test)]
mod tests;
