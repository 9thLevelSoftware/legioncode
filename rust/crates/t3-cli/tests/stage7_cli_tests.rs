//! Stage 7 Wave Q integration tests covering all newly ported CLI commands and workers.

mod support;

#[path = "stage7/app_triage_browser.rs"]
mod app_triage_browser;
#[path = "stage7/help_routing_workers.rs"]
mod help_routing_workers;
#[path = "stage7/up_other_and_protect.rs"]
mod up_other_and_protect;
#[path = "stage7/up_service_install_update.rs"]
mod up_service_install_update;

use up_service_install_update as stage7_up_tools;
