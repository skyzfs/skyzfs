//! The SkyZFS node agent (work package M2.6; design §5.2, "A snapshot
//! is published by a cut of the pool's present history", and §5.4): it
//! takes a snapshot of the pool on a served volume, holds it, and asks
//! the volume's daemon to publish it as `snap/<t>` with the cut that
//! holds it, through the daemon's channel ([`skyzfs_control`]).
//!
//! This crate is the agent's core, built in steps (the plan of M2.6,
//! §8): the names it gives ([`names`]), its record of each volume
//! ([`record`]), the ZFS manifest ([`manifest`]), the planner, which
//! says what to do next from what is seen ([`plan`]), and, next, the
//! engine and the adapters to `zfs` and the daemon. It has no async
//! runtime and links nothing of the daemon.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod manifest;
pub mod names;
pub mod plan;
pub mod record;
