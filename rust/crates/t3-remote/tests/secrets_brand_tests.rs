//! Tests for secret path-traversal confinement (`#39483`) and brand routing across
//! secrets, MCP bridge, theme override, and trace logs.

use std::path::Path;

use t3_contracts::brand::{ACTIVE_BRAND, LEGION_CODE};
use t3_remote::acp_mcp_bridge::resolve_mcp_bridge_url_for_brand;
use t3_remote::secrets::{
    FileSecretStore, SecretStoreError, branded_secret_name_for_brand, resolve_secret_home_for_brand,
};
use t3_remote::theme::{resolve_theme_override_for_brand, theme_env_var_candidates_for_brand};
use t3_remote::trace::{resolve_trace_file_for_brand, trace_file_env_var_candidates_for_brand};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn secrets_path_traversal_and_brand_routing_enforced() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = FileSecretStore::from_state_dir(dir.path())?;
    for bad in [
        "",
        ".",
        "..",
        "marker-x/../session-signing-key",
        "marker-x\\..\\session-signing-key",
        "drive:secret",
        "bad\0name",
    ] {
        assert!(matches!(
            store.set(bad, b"x"),
            Err(SecretStoreError::InvalidName { .. })
        ));
        assert!(matches!(
            store.create(bad, b"x"),
            Err(SecretStoreError::InvalidName { .. })
        ));
        assert!(matches!(
            store.get(bad),
            Err(SecretStoreError::InvalidName { .. })
        ));
        assert!(matches!(
            store.remove(bad),
            Err(SecretStoreError::InvalidName { .. })
        ));
    }

    assert_eq!(
        branded_secret_name_for_brand(&ACTIVE_BRAND, "token"),
        "t3code-token"
    );
    assert_eq!(
        branded_secret_name_for_brand(&LEGION_CODE, "token"),
        "legioncode-token"
    );
    assert_eq!(
        resolve_secret_home_for_brand(&LEGION_CODE, |_| None, Some(Path::new("/home/u"))),
        Some(Path::new("/home/u/.legion").to_path_buf())
    );
    assert_eq!(
        resolve_mcp_bridge_url_for_brand(&LEGION_CODE, |k| {
            (k == "LEGION_ACP_MCP_URL").then(|| "http://127.0.0.1:4321/mcp".to_owned())
        })
        .as_deref(),
        Some("http://127.0.0.1:4321/mcp")
    );
    assert_eq!(
        theme_env_var_candidates_for_brand(&LEGION_CODE),
        ["LEGION_THEME", "T3CODE_THEME"]
    );
    assert_eq!(
        resolve_theme_override_for_brand(&ACTIVE_BRAND, |k| {
            (k == "T3CODE_THEME").then(|| "ember".to_owned())
        })
        .as_deref(),
        Some("ember")
    );
    assert_eq!(
        trace_file_env_var_candidates_for_brand(&LEGION_CODE),
        ["LEGION_TRACE_FILE", "T3CODE_TRACE_FILE"]
    );
    assert_eq!(
        resolve_trace_file_for_brand(&LEGION_CODE, |_| None, Some(Path::new("/home/u"))),
        Some(Path::new("/home/u/.legion/userdata/logs/trace.ndjson").to_path_buf())
    );
    Ok(())
}
