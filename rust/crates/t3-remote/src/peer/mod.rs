//! Linked-environment peer management and remote task delegation tracking.
//!
//! - [`links`]: [`links::PeerLinksService`], loopback/Tailnet HTTP vs HTTPS URL guards,
//!   and byte-exact CLI list/JSON formatters (`apps/server/src/peer/PeerLinks.ts`,
//!   `apps/server/src/cli/environment.ts`).
//! - [`delegation`]: [`delegation::RemoteDelegationTracker`] with `15s`/`600s` wait slicing,
//!   `stop_count` fencing, and `BUG-07` fix ensuring `ParentDeleted` always emits `StopThere`
//!   (`apps/server/src/peer/RemoteDelegation.ts`).

pub mod delegation;
pub mod links;
