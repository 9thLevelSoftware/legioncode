//! Integration and property-based tests for `t3-client-core`.

#[path = "client_core_tests/backoff_probe_dpop.rs"]
mod backoff_probe_dpop;
#[path = "client_core_tests/diff_markdown.rs"]
mod diff_markdown;
#[path = "client_core_tests/steady_shells.rs"]
mod steady_shells;
#[path = "client_core_tests/thread_families.rs"]
mod thread_families;
#[path = "client_core_tests/wave2_linked_threads.rs"]
mod wave2_linked_threads;
#[path = "client_core_tests/wave2_recovery_caches.rs"]
mod wave2_recovery_caches;
