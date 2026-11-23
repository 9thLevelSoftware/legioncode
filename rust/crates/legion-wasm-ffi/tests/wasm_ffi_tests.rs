//! Integration test suite for `legion-wasm-ffi` pure-Rust host-testable bridge and ABI surface.

mod wasm_ffi_tests {
    mod client_core;
    mod edge_worker;
    mod helpers;
    mod hook_actor;
    mod terminal_vt;
    mod wave5_helpers;
}
