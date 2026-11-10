//! Thin-mode stack seam over the agent crate
//! ([`skynbd_agent::thinstack`]), so the publish-only build/teardown
//! choreography (origin-advance §5) has exactly one implementation.

#[allow(unused_imports)]
pub use skynbd_agent::provision::{StackNames, ThinStackSpec};
pub use skynbd_agent::thinstack::{DmThin, ThinStack};
