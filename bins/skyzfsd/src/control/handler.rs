//! What the daemon answers on its channel: its uploader's answers, or
//! a refusal where it has none ([module docs](super)).
//!
//! The count of cuts is not asked of the uploader: it is the store's,
//! asked at the desk where the queue's thread answers the uploader's
//! calls ([`Desk::cuts_now`]), and given at the queue's next turn,
//! whatever step the uploader is in. A stalled PUT or a slow lookup
//! holds up the publishes' answers, which wait for the step under way,
//! but not `cuts` (F1 and F4 of the review of #127).
//!
//! Nor is `look` (work package M2.6a): the channel reads `snap/T` and its
//! row in the refs object itself, through a client of object storage of
//! its own ([`Lookup`]), made on the channel's runtime when the handler
//! is, and kept. The uploader's client is on the uploader's runtime, and
//! a look made through it would wait for the uploader's thread; and the
//! uploader's own lookups are steps of its publishes, not answers to the
//! agent. A look is answered while the uploader steps, after it has
//! stopped, and after it has ended.

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use skynbd_refs::RefStore;
use skynbd_types::VolumeId;
use skyzfs_upload::publish::{Request as Ask, EXPIRY_DEFAULT};
use skyzfs_upload::queued::Desk;
use skyzfs_upload::{Answer, Control};
use tokio::sync::{mpsc, oneshot};

use super::{
    code_of, reply_of, retired_reply, seen_reply, Code, CutsReply, Handler, Reply, Request,
};
use crate::upload::Runtime;

/// How many requests may wait in the uploader's inbox. The channel asks
/// one at a time, so more wait only after `no-answer`s, whose requests
/// the uploader then skips.
pub const INBOX: usize = 16;

/// A handler that refuses every request `no-upload`, saying `why`.
pub fn no_upload(why: String) -> Handler {
    Box::new(move |_: &Request, _: Vec<u8>| Reply::Refused(Code::NoUpload, why.clone()))
}

/// What `look` reads: the volume's refs object in object storage,
/// through the channel's own client.
pub struct Lookup<R> {
    /// The volume's id in object storage.
    pub vol: VolumeId,
    /// Its refs.
    pub refs: R,
}

/// A handler that asks the uploader through `inbox` (read by
/// `Uploader::run_with`), the store's count of cuts at `desk`, and
/// `look`s of what `lookup` makes, for the daemon that serves
/// `attachment`. Each answer is waited for at most `limit`
/// ([`ANSWER_LIMIT`](super::ANSWER_LIMIT) in the daemon), and a
/// `publish`, which asks the desk and then the uploader, waits for both
/// within it. `lookup` is
/// made here, on the channel's runtime, and asks nothing of object
/// storage.
pub fn uploader<R, F>(
    attachment: [u8; 16],
    inbox: mpsc::Sender<Control>,
    desk: Arc<Desk>,
    lookup: impl FnOnce() -> F,
    limit: Duration,
) -> io::Result<Handler>
where
    R: RefStore + 'static,
    F: Future<Output = Lookup<R>>,
{
    // The channel's thread is not the runtime's: it waits on its own,
    // with time to limit the wait. The runtime's I/O is the lookup's
    // client, and its drop does not wait for a name lookup that hangs
    // (`crate::upload::Runtime`).
    let runtime = crate::upload::runtime()?;
    let lookup = runtime.block_on(lookup())?;
    Ok(Box::new(move |request: &Request, manifest: Vec<u8>| {
        let waiter = Waiter {
            runtime: &runtime,
            limit,
            began: std::time::Instant::now(),
        };
        let asked = Asked {
            inbox: &inbox,
            desk: &desk,
            lookup: &lookup,
            attachment,
        };
        match answer(&waiter, &asked, request, manifest) {
            Ok(reply) | Err(reply) => reply,
        }
    }))
}

struct Waiter<'a> {
    runtime: &'a Runtime,
    /// How long a request's answer is waited for, all its waits counted.
    limit: Duration,
    /// When the request began.
    began: std::time::Instant,
}

impl Waiter<'_> {
    /// Run `future` on the runtime, for at most what is left of the
    /// limit: `None` where it did not end in time.
    fn within<F: Future>(&self, future: F) -> Result<Option<F::Output>, Reply> {
        let limit = self.limit.saturating_sub(self.began.elapsed());
        // The timeout is made inside the runtime: its timer needs the
        // runtime's context.
        self.runtime
            .block_on(async move { tokio::time::timeout(limit, future).await.ok() })
            .map_err(|e| Reply::Refused(Code::Ended, format!("the channel's runtime: {e}")))
    }
}

/// What the handler asks.
struct Asked<'a, R> {
    inbox: &'a mpsc::Sender<Control>,
    desk: &'a Desk,
    lookup: &'a Lookup<R>,
    attachment: [u8; 16],
}

fn answer<R: RefStore>(
    waiter: &Waiter<'_>,
    asked: &Asked<'_, R>,
    request: &Request,
    manifest: Vec<u8>,
) -> Result<Reply, Reply> {
    let Asked {
        inbox,
        desk,
        lookup,
        attachment,
    } = *asked;
    Ok(match request {
        Request::Cuts => {
            let now = count(waiter, desk)?;
            Reply::Cuts(CutsReply {
                attachment,
                state: now.state,
                count: now.count,
                newest: now.newest.map(|cut| (cut.group, cut.itself)),
                rewound: now.rewound,
            })
        }
        Request::Publish(p) => {
            // The store's rewind mark now, read at the desk as for
            // `cuts`, goes with the request: the uploader's own is given
            // at its checks of cuts, and can be a rewind behind while a
            // cut uploads (B3 of the review of #140). A rewind counted
            // after this read was counted after the agent's snapshot
            // (`skyzfs_upload::publish`'s module docs).
            let now = count(waiter, desk)?;
            let ask = Ask {
                name: p.name.clone(),
                attachment: p.attachment,
                state: p.state,
                after: p.after,
                group: p.group,
                manifest,
                expiry: p.expiry.unwrap_or(EXPIRY_DEFAULT),
            };
            let (tx, rx) = oneshot::channel();
            send(
                inbox,
                Control::Ask {
                    request: ask,
                    rewound: now.rewound,
                    reply: tx,
                },
            )?;
            match wait(waiter, rx, PUBLISH_AGAIN)? {
                Answer::Accepted => Reply::Accepted,
                Answer::Refused(why) => Reply::Refused(code_of(&why), why.to_string()),
            }
        }
        Request::Progress(name) => {
            let (tx, rx) = oneshot::channel();
            send(inbox, Control::Progress(name.clone(), tx))?;
            reply_of(&wait(waiter, rx, PUBLISH_AGAIN)?)
        }
        Request::Withdraw(name) => {
            let (tx, rx) = oneshot::channel();
            send(inbox, Control::Withdraw(name.clone(), tx))?;
            reply_of(&wait(waiter, rx, PUBLISH_AGAIN)?)
        }
        Request::Look(name) => look(waiter, lookup, name)?,
        Request::Retire(r) => {
            // Through the uploader, under its lease and fence (the lead's
            // ?6 of M2.7b): its answer waits for the step under way.
            let (tx, rx) = oneshot::channel();
            send(
                inbox,
                Control::Retire {
                    name: r.name.clone(),
                    commit: r.commit,
                    reply: tx,
                },
            )?;
            retired_reply(&wait(waiter, rx, RETIRE_AGAIN)?)
        }
    })
}

/// What `snap/<name>` is in object storage, read within the waiter's
/// limit. An error of object storage is `store`, and not `absent`.
fn look<R: RefStore>(waiter: &Waiter<'_>, lookup: &Lookup<R>, name: &str) -> Result<Reply, Reply> {
    match waiter.within(skyzfs_upload::look::look(&lookup.refs, lookup.vol, name))? {
        Some(Ok(seen)) => Ok(seen_reply(&seen)),
        Some(Err(e)) => Err(Reply::Refused(Code::Store, format!("object storage: {e}"))),
        None => Err(Reply::Refused(
            Code::NoAnswer,
            format!(
                "object storage did not answer within what was left of the \
                 request's {} s",
                waiter.limit.as_secs_f64()
            ),
        )),
    }
}

/// Put `control` in the uploader's inbox, without waiting for room.
fn send(inbox: &mpsc::Sender<Control>, control: Control) -> Result<(), Reply> {
    inbox.try_send(control).map_err(|e| match e {
        mpsc::error::TrySendError::Full(_) => Reply::Refused(
            Code::NoAnswer,
            format!("{INBOX} requests wait for the uploader"),
        ),
        mpsc::error::TrySendError::Closed(_) => ended(),
    })
}

/// What a `no-answer` of a publish, its progress or its withdrawal adds.
const PUBLISH_AGAIN: &str = "a publish may have been taken: ask again, or withdraw it";

/// What a `no-answer` of a retire adds: the uploader takes it at its
/// next turn, so it may still be made.
const RETIRE_AGAIN: &str = "the retire may still be made: ask again, which answers gone once it is";

/// The uploader's answer, waited for at most the waiter's limit. A
/// `no-answer`, and an `ended` where the uploader took the request and
/// dropped it, end with `again`.
fn wait<T>(waiter: &Waiter<'_>, rx: oneshot::Receiver<T>, again: &str) -> Result<T, Reply> {
    match waiter.within(rx)? {
        Some(Ok(answer)) => Ok(answer),
        // The uploader dropped the request unanswered: it has ended,
        // perhaps in the request's own step, which may have landed.
        Some(Err(_)) => Err(Reply::Refused(
            Code::Ended,
            format!("the uploader has ended; {again}"),
        )),
        None => Err(Reply::Refused(
            Code::NoAnswer,
            format!(
                "the uploader did not answer within what was left of the \
                 request's {} s; {again}",
                waiter.limit.as_secs_f64()
            ),
        )),
    }
}

/// The store's count of cuts, asked at the desk and waited for at most
/// the waiter's limit. A closed desk is a store no longer served:
/// `ended`, and not a count of nothing (F5 of the review of #127).
fn count(waiter: &Waiter<'_>, desk: &Desk) -> Result<skyzfs_upload::CutsNow, Reply> {
    match waiter.within(desk.cuts_now())? {
        Some(Some(now)) => Ok(now),
        Some(None) => Err(Reply::Refused(
            Code::Ended,
            "the store is no longer served".into(),
        )),
        None => Err(Reply::Refused(
            Code::NoAnswer,
            format!(
                "the store did not answer within what was left of the \
                 request's {} s: its queue takes no turn",
                waiter.limit.as_secs_f64()
            ),
        )),
    }
}

fn ended() -> Reply {
    Reply::Refused(Code::Ended, "the uploader has ended".into())
}
