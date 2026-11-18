//! What the daemon answers on its channel: its uploader's answers, or
//! a refusal where it has none ([module docs](super)).

use std::io;
use std::time::Duration;

use skyzfs_upload::publish::{Request as Ask, EXPIRY_DEFAULT};
use skyzfs_upload::{Answer, Control};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};

use super::{Code, CutsReply, Handler, Reply, Request};

/// How many requests may wait in the uploader's inbox. The channel asks
/// one at a time, so more wait only after `no-answer`s, whose requests
/// the uploader then skips.
pub const INBOX: usize = 16;

/// A handler that refuses every request `no-upload`, saying `why`.
pub fn no_upload(why: String) -> Handler {
    Box::new(move |_: &Request, _: Vec<u8>| Reply::Refused(Code::NoUpload, why.clone()))
}

/// A handler that asks the uploader through `inbox` (read by
/// `Uploader::run_with`), for the daemon that serves `attachment`.
/// Each answer is waited for at most `limit` ([`ANSWER_LIMIT`](super::ANSWER_LIMIT) in the
/// daemon).
pub fn uploader(
    attachment: [u8; 16],
    inbox: mpsc::Sender<Control>,
    limit: Duration,
) -> io::Result<Handler> {
    // The channel's thread is not the runtime's: it waits on its own,
    // with time to limit the wait.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;
    Ok(Box::new(move |request: &Request, manifest: Vec<u8>| {
        let waiter = Waiter {
            runtime: &runtime,
            limit,
        };
        match answer(&waiter, &inbox, attachment, request, manifest) {
            Ok(reply) | Err(reply) => reply,
        }
    }))
}

struct Waiter<'a> {
    runtime: &'a Runtime,
    limit: Duration,
}

fn answer(
    waiter: &Waiter<'_>,
    inbox: &mpsc::Sender<Control>,
    attachment: [u8; 16],
    request: &Request,
    manifest: Vec<u8>,
) -> Result<Reply, Reply> {
    Ok(match request {
        Request::Cuts => {
            let (tx, rx) = oneshot::channel();
            send(inbox, Control::Cuts(tx))?;
            let now = wait(waiter, rx)?;
            Reply::Cuts(CutsReply {
                attachment,
                state: now.state,
                count: now.count,
                newest: now.newest.map(|cut| (cut.group, cut.itself)),
            })
        }
        Request::Publish(p) => {
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
            send(inbox, Control::Ask(ask, tx))?;
            match wait(waiter, rx)? {
                Answer::Accepted => Reply::Accepted,
                Answer::Refused(why) => Reply::Refused(Code::of(&why), why.to_string()),
            }
        }
        Request::Progress(name) => {
            let (tx, rx) = oneshot::channel();
            send(inbox, Control::Progress(name.clone(), tx))?;
            Reply::of(&wait(waiter, rx)?)
        }
        Request::Withdraw(name) => {
            let (tx, rx) = oneshot::channel();
            send(inbox, Control::Withdraw(name.clone(), tx))?;
            Reply::of(&wait(waiter, rx)?)
        }
    })
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

/// The uploader's answer, waited for at most the waiter's limit.
fn wait<T>(waiter: &Waiter<'_>, rx: oneshot::Receiver<T>) -> Result<T, Reply> {
    // The timeout is made inside the runtime: its timer needs the
    // runtime's context.
    let limit = waiter.limit;
    match waiter
        .runtime
        .block_on(async move { tokio::time::timeout(limit, rx).await })
    {
        Ok(Ok(answer)) => Ok(answer),
        // The uploader dropped the request unanswered: it has ended.
        Ok(Err(_)) => Err(ended()),
        Err(_) => Err(Reply::Refused(
            Code::NoAnswer,
            format!(
                "the uploader did not answer within {} s; a publish may have been \
                 taken: ask again, or withdraw it",
                waiter.limit.as_secs_f64()
            ),
        )),
    }
}

fn ended() -> Reply {
    Reply::Refused(Code::Ended, "the uploader has ended".into())
}
