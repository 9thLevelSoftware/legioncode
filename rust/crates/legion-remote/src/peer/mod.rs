//! Linked-environment peer management, transitive pairing, read-only thread relay,
//! and remote task delegation tracking.
//!
//! - [`client_pairing`]: Batch peer linking (`#39426`) and transitive client/link pairing (`#40036`).
//! - [`delegation`]: [`delegation::RemoteDelegationTracker`] with `15s`/`600s` wait slicing,
//!   `stop_count` fencing, and `BUG-07` fix ensuring `ParentDeleted` always emits `StopThere`
//!   (`apps/server/src/peer/RemoteDelegation.ts`).
//! - [`links`]: [`links::PeerLinksService`], loopback/Tailnet HTTP vs HTTPS URL guards,
//!   and byte-exact CLI list/JSON formatters (`apps/server/src/peer/PeerLinks.ts`,
//!   `apps/server/src/cli/environment.ts`).
//! - [`thread_relay`]: Read-only linked-environment thread HTTP/SSE relay (`#39406`),
//!   newest-page default (`#39981`, `#40003`), and headless settings hint (`#39964`).

pub mod batch_linking;
pub mod client_pairing;
pub mod client_pairing_issuer;
pub mod delegation;
pub mod linked_history;
pub mod links;
pub mod thread_relay;
pub mod thread_relay_helpers;
