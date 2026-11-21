//! Integration tests for `t3-process-ffi` (`BUG-09` env, pipes, Win32 job supervision,
//! and `native/resource-monitor` protocol v3 engine).

mod process_ffi_tests {
    mod env_tests;
    mod resource_monitor_tests;
    mod supervisor_tests;
}
