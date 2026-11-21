//! Integration tests and `proptest` property suites for `t3-edge`.

mod edge_tests {
    mod boot_service_units;
    mod cf_bindings;
    mod desktop_updates;
    mod i18n;
    mod relay_hooks;
    mod task_cache;
}
