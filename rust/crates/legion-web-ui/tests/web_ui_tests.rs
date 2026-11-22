//! Integration and property test suite for `legion-web-ui`.

#[path = "web_ui_tests/client_and_stream.rs"]
mod client_and_stream;
#[path = "web_ui_tests/surfaces_and_panels.rs"]
mod surfaces_and_panels;
#[path = "web_ui_tests/wave5_features.rs"]
mod wave5_features;
#[path = "web_ui_tests/wave5_workspace.rs"]
mod wave5_workspace;
#[path = "web_ui_tests/workflow_e2e.rs"]
mod workflow_e2e;
#[path = "web_ui_tests/xss_and_props.rs"]
mod xss_and_props;
