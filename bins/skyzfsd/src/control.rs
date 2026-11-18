//! The daemon's channel (M2.5-2): how an agent on the node asks a serving
//! daemon for its count of cuts, and asks it to publish a cut as the
//! snapshot `snap/<t>` (design §5.2, "A snapshot is published by a cut of
//! the pool's present history", and §5.4). It is the daemon's first
//! interface that is not a file. The rule a publish follows, and what
//! object storage holds once it is made, are `skyzfs_upload::publish`'s
//! (M2.5-1); this module carries the requests to the uploader and its
//! answers back.
//!
//! # The socket
//!
//! A Unix stream socket, [`SOCKET`] in the volume's run directory, made
//! once the device serves and removed at the end of `serve`. A socket
//! left there by an instance that was killed is removed by the next one,
//! which holds the volume's lock, before it makes its own. Its mode is
//! 0600, and the daemon answers only a peer whose user is the daemon's
//! own or root (`SO_PEERCRED`): any other is refused (`peer`) and told
//! nothing else. One connection is answered at a time, on a thread of its
//! own, and one request per connection: the daemon answers and closes.
//! A connection that sends nothing for [`IO_LIMIT`] is closed.
//!
//! `status` says what the channel is (`control`: `uploads` where the
//! uploader answers it, `no_upload` where every request is refused
//! `no-upload`, `failed` where the socket could not be made) and what it
//! has done (`control_answered`, `control_peer`, `control_malformed`:
//! [`ChannelFigures`]), as of its last connection.
//!
//! # The protocol, version 1
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
//! | `cuts` | `cuts attachment=A state=S count=C group=G opens=O` |
//! | `publish name=T attachment=A state=S after=C group=G manifest=N [expiry-s=E]`, then N bytes | `accepted` |
//! | `progress name=T` | `pending`, `done commit=H`, `expired`, `withdrawn`, `unknown` |
//! | `withdraw name=T` | as `progress`, as the publish stands after the withdrawal |
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
//!   `itself` or `rewind`, or both `none` before the first cut). The
//!   count is the store's when the request is answered: a cut counted
//!   before the request was sent is in it.
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
//!   it had; asked of the next daemon (after a restart), it has a new
//!   one. So an agent that holds `pool@T` while its publish pends counts
//!   its hold from its last request answered `accepted`, not its first
//!   (q11 of the review of #122).
//! - `progress`: how the publish of T stands. `done` names the commit
//!   (H, 64 hex digits) that `snap/T` names. `unknown`: not asked of
//!   this daemon's uploader, or its end has left the 1024 kept.
//! - `withdraw`: end the publish of T, unless it is made already. One
//!   whose transaction may have landed is looked up first, and so may
//!   still read `pending` and then `done`.
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
//! or withdraw it. The other codes are a publish's refusals
//! (`skyzfs_upload::Refusal`).
//!
//! # What the daemon decides here, and the design does not
//!
//! The socket's place and mode, the peer's users, the limits and the
//! words of the protocol are this module's (the design leaves the
//! channel to M2.5; the lead's plan of M2.5 says "a Unix socket per
//! volume, protocol in M2.5-2's pull request"). A daemon that cannot
//! make its socket serves all the same and says so on standard error:
//! serving the volume comes before publishing it.

use std::fmt::Write as _;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use skyzfs_upload::publish::{EXPIRY_MAX, MANIFEST_MAX};
use skyzfs_upload::{Progress, Refusal};

/// The socket's name in the volume's run directory.
pub const SOCKET: &str = "control";

/// The greeting of version 1, the first line each way.
pub const HELLO: &str = "skyzfs-control 1";

/// The longest line, its `\n` included.
pub const LINE_MAX: usize = 4096;

/// How long either side waits for the other's next bytes.
pub const IO_LIMIT: Duration = Duration::from_secs(10);

/// How long the daemon waits for its uploader's answer: it answers
/// between two of its steps, and a step can wait on object storage.
pub const ANSWER_LIMIT: Duration = Duration::from_secs(30);

/// How long the command line's client waits for each read and write: the
/// daemon may wait [`ANSWER_LIMIT`] for its uploader, and [`IO_LIMIT`]
/// more for the connection.
pub const ASK_LIMIT: Duration = Duration::from_secs(ANSWER_LIMIT.as_secs() + IO_LIMIT.as_secs());

/// A request ([module docs](self)).
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

/// An answer ([module docs](self)).
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
    /// Refused, and why.
    Refused(Code, String),
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
    /// The name pends with other parameters.
    Asked,
    /// Too many publishes pend.
    Busy,
    /// `snap/T` names another commit.
    Taken,
    /// The uploader publishes nothing more.
    Stopped,
    /// The greeting is not this version's.
    Version,
    /// The request does not parse.
    Malformed,
    /// The peer's user is neither the daemon's nor root.
    Peer,
    /// The daemon has no uploader.
    NoUpload,
    /// The uploader has ended.
    Ended,
    /// The uploader did not answer in time.
    NoAnswer,
}

const CODES: [(Code, &str); 15] = [
    (Code::Name, "name"),
    (Code::Attachment, "attachment"),
    (Code::State, "state"),
    (Code::Manifest, "manifest"),
    (Code::Expiry, "expiry"),
    (Code::Asked, "asked"),
    (Code::Busy, "busy"),
    (Code::Taken, "taken"),
    (Code::Stopped, "stopped"),
    (Code::Version, "version"),
    (Code::Malformed, "malformed"),
    (Code::Peer, "peer"),
    (Code::NoUpload, "no-upload"),
    (Code::Ended, "ended"),
    (Code::NoAnswer, "no-answer"),
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

    /// The code of a publish's refusal.
    pub fn of(refusal: &Refusal) -> Self {
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
        }
    }
}

impl Reply {
    /// The answer that says how a publish stands.
    pub fn of(progress: &Progress) -> Self {
        match progress {
            Progress::Pending => Reply::Pending,
            Progress::Done(commit) => Reply::Done(*commit),
            Progress::Refused(why) => Reply::Refused(Code::of(why), why.to_string()),
            Progress::Expired => Reply::Expired,
            Progress::Withdrawn => Reply::Withdrawn,
            Progress::Unknown => Reply::Unknown,
        }
    }

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
                let p = pairs(rest, &["attachment", "state", "count", "group", "opens"])?;
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
                }))
            }
            "accepted" => bare(Reply::Accepted),
            "pending" => bare(Reply::Pending),
            "expired" => bare(Reply::Expired),
            "withdrawn" => bare(Reply::Withdrawn),
            "unknown" => bare(Reply::Unknown),
            "done" => {
                let p = pairs(rest, &["commit"])?;
                let text = take(&p, "commit")?;
                let commit = unhex(text)
                    .ok_or_else(|| format!("`commit` is not 64 hex digits: `{text}`"))?;
                Ok(Reply::Done(commit))
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
                    "cuts attachment={} state={state} count={} group={group} opens={opens}",
                    hex(&c.attachment),
                    c.count
                )
            }
            Reply::Accepted => "accepted".into(),
            Reply::Pending => "pending".into(),
            Reply::Done(commit) => format!("done commit={}", hex(commit)),
            Reply::Expired => "expired".into(),
            Reply::Withdrawn => "withdrawn".into(),
            Reply::Unknown => "unknown".into(),
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
fn read_line(reader: &mut impl BufRead) -> io::Result<String> {
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

/// Ask the daemon behind the socket at `path`: `request`, with
/// `manifest` after it for `publish`, waiting at most `limit` for each
/// read and write. The answer, or why none came.
pub fn ask(path: &Path, request: &Request, manifest: &[u8], limit: Duration) -> io::Result<Reply> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(limit))?;
    stream.set_write_timeout(Some(limit))?;
    let mut sent = format!("{HELLO}\n{}\n", request.line()).into_bytes();
    sent.extend_from_slice(manifest);
    // A daemon that refuses a request before reading all of it closes
    // the connection, and the rest of the write fails: its answer is read
    // all the same, and the write's error given only when none came.
    let written = stream.write_all(&sent).and_then(|()| stream.flush());
    let mut reader = BufReader::new(stream);
    let hello = match (read_line(&mut reader), written) {
        (Ok(hello), _) => hello,
        (Err(_), Err(e)) => return Err(e),
        (Err(e), Ok(())) => return Err(e),
    };
    if hello != HELLO {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the daemon greets with `{hello}`, not `{HELLO}`"),
        ));
    }
    let line = read_line(&mut reader)?;
    Reply::parse(&line).map_err(|why| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the answer `{line}`: {why}"),
        )
    })
}

pub mod handler;
#[cfg(target_os = "linux")]
pub mod server;
#[cfg(target_os = "linux")]
pub use server::{Channel, ChannelFigures, Post};

#[cfg(test)]
mod tests;
