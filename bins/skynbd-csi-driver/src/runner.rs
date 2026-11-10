//! Host-command execution seam — shared with the workstation CLI via
//! the agent crate ([`skynbd_agent::hostcmd`]), so the nbd-client /
//! mkfs / mount choreography and the pool-device preparation run the
//! same code everywhere and stay unit-testable without root.

#[allow(unused_imports)]
pub use skynbd_agent::hostcmd::{HostRunner, Output, Runner};

#[cfg(test)]
pub use skynbd_agent::hostcmd::mock;
