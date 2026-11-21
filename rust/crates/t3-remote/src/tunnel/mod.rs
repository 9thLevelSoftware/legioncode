//! SSH and Tailscale remote connectivity primitives.
//!
//! - [`ssh`]: OpenSSH argv builders, `ssh -G` / `config` / `known_hosts` discovery, control-master
//!   socket helpers, and local port-forward tunnel state machine (`packages/ssh/src/`).
//! - [`tailscale`]: Tailscale CLI status/serve JSON parsers, `MagicDNS` HTTPS URL builder,
//!   Serve root-holder inspector, and exact `cli/pair.ts` diagnostics (`packages/tailscale/src/`).

pub mod ssh;
pub mod tailscale;
