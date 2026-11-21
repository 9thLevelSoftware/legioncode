//! CLI text and JSON formatters for T3 Connect waiting requests and trusted devices.

use serde::Serialize;

use crate::auth_records::{AuthRecordsError, ClientMetadataRow, format_client_metadata};

use super::{TrustedConnectDevice, WaitingConnectDevice};

/// Formats waiting and trusted T3 Connect devices matching `cliAuthFormat.ts:200-240` byte-for-byte.
pub fn format_connect_device_list(
    waiting: &[WaitingConnectDevice],
    trusted: &[TrustedConnectDevice],
    json: bool,
) -> Result<String, AuthRecordsError> {
    if json {
        let payload = ConnectDeviceListJson { waiting, trusted };
        let mut out =
            serde_json::to_string_pretty(&payload).map_err(|source| AuthRecordsError::Json {
                operation: "format_connect_device_list",
                source,
            })?;
        out.push('\n');
        return Ok(out);
    }

    let describe = |client: Option<&ClientMetadataRow>| match client {
        Some(meta) => format_client_metadata(meta),
        None => "unlabeled client".to_owned(),
    };

    let mut lines = Vec::new();
    if waiting.is_empty() {
        lines.push("No devices are waiting for approval.".to_owned());
    } else {
        lines.push("Waiting for approval:".to_owned());
        for req in waiting {
            lines.push(format!(
                "  {}  {}",
                req.verification_code,
                describe(req.client.as_ref()),
            ));
        }
    }
    lines.push(String::new());
    if trusted.is_empty() {
        lines.push("No trusted T3 Connect devices.".to_owned());
    } else {
        lines.push("Trusted:".to_owned());
        for dev in trusted {
            lines.push(format!(
                "  {}  {}  since {}",
                dev.verification_code,
                describe(dev.client.as_ref()),
                dev.approved_at,
            ));
        }
    }

    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

#[derive(Serialize)]
struct ConnectDeviceListJson<'a> {
    waiting: &'a [WaitingConnectDevice],
    trusted: &'a [TrustedConnectDevice],
}
