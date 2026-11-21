//! macOS `.mobileconfig` device configuration profile generator (`policyTemplates.ts:308-386`).

use std::fmt::Write as _;

use serde_json::{Map, Value};
use thiserror::Error;

use super::ManagedPolicyDocument;
use super::windows_templates::escape_xml;

/// Error returned when rendering a macOS `.mobileconfig` profile fails.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PolicyTemplateError {
    /// A configuration profile cannot represent JSON `null` values (`policyTemplates.ts:352-355`).
    #[error("{reason}")]
    NullInProfile {
        /// Human-readable reason naming the offending setting path.
        reason: String,
    },
    /// Failed to convert a policy document into a JSON value tree.
    #[error("failed to prepare policy template payload: {0}")]
    Serialization(String),
}

fn find_null_path(value: &Value, path: &mut Vec<String>) -> Option<Vec<String>> {
    match value {
        Value::Null => Some(path.clone()),
        Value::Array(items) => {
            for (idx, item) in items.iter().enumerate() {
                path.push(idx.to_string());
                if let Some(found) = find_null_path(item, path) {
                    return Some(found);
                }
                path.pop();
            }
            None
        }
        Value::Object(map) => {
            for (key, item) in map {
                path.push(key.clone());
                if let Some(found) = find_null_path(item, path) {
                    return Some(found);
                }
                path.pop();
            }
            None
        }
        Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

fn plist_value(value: &Value, indent: &str, out: &mut String) {
    let inner = format!("{indent}  ");
    match value {
        Value::String(text) => {
            let _ = write!(out, "{indent}<string>{}</string>", escape_xml(text));
        }
        Value::Bool(flag) => {
            let _ = write!(out, "{indent}<{flag} />");
        }
        Value::Number(num) => {
            if let Some(int) = num.as_i64() {
                let _ = write!(out, "{indent}<integer>{int}</integer>");
            } else if let Some(uint) = num.as_u64() {
                let _ = write!(out, "{indent}<integer>{uint}</integer>");
            } else {
                let _ = write!(out, "{indent}<real>{num}</real>");
            }
        }
        Value::Array(items) => {
            let _ = writeln!(out, "{indent}<array>");
            for item in items {
                plist_value(item, &inner, out);
                out.push('\n');
            }
            let _ = write!(out, "{indent}</array>");
        }
        Value::Object(entries) => {
            let _ = writeln!(out, "{indent}<dict>");
            for (key, item) in entries {
                let _ = writeln!(out, "{inner}<key>{}</key>", escape_xml(key));
                plist_value(item, &inner, out);
                out.push('\n');
            }
            let _ = write!(out, "{indent}</dict>");
        }
        Value::Null => {
            let _ = write!(out, "{indent}<string />");
        }
    }
}

/// Generates a macOS `.mobileconfig` configuration profile for `com.t3tools.t3code`.
///
/// Rejects documents containing `null` anywhere in their JSON tree because XML property
/// lists cannot encode `null` (`policyTemplates.ts:341-386`).
pub fn macos_profile(
    document: &ManagedPolicyDocument,
    profile_uuid: &str,
    payload_uuid: &str,
) -> Result<String, PolicyTemplateError> {
    let doc_value = serde_json::to_value(document)
        .map_err(|err| PolicyTemplateError::Serialization(err.to_string()))?;
    let mut path_buf = Vec::new();
    if let Some(empty) = find_null_path(&doc_value, &mut path_buf) {
        let trimmed = if empty.first().map(String::as_str) == Some("settings")
            && empty.last().map(String::as_str) == Some("value")
        {
            &empty[..empty.len() - 1]
        } else {
            &empty[..]
        };
        return Err(PolicyTemplateError::NullInProfile {
            reason: format!(
                "{} is null, which a configuration profile cannot hold; deploy this policy as a file.",
                trimmed.join(".")
            ),
        });
    }

    let mut payload_dict = Map::new();
    payload_dict.insert(
        "PayloadDisplayName".to_owned(),
        Value::String("T3 Code policy".to_owned()),
    );
    payload_dict.insert(
        "PayloadIdentifier".to_owned(),
        Value::String(format!("com.t3tools.t3code.policy.{payload_uuid}")),
    );
    payload_dict.insert(
        "PayloadType".to_owned(),
        Value::String("com.t3tools.t3code".to_owned()),
    );
    payload_dict.insert(
        "PayloadUUID".to_owned(),
        Value::String(payload_uuid.to_owned()),
    );
    payload_dict.insert("PayloadVersion".to_owned(), Value::Number(1.into()));
    if let Value::Object(doc_entries) = doc_value {
        for (key, value) in doc_entries {
            payload_dict.insert(key, value);
        }
    }

    let mut profile = Map::new();
    profile.insert(
        "PayloadContent".to_owned(),
        Value::Array(vec![Value::Object(payload_dict)]),
    );
    profile.insert(
        "PayloadDescription".to_owned(),
        Value::String("Configures T3 Code for every user on this Mac.".to_owned()),
    );
    profile.insert(
        "PayloadDisplayName".to_owned(),
        Value::String(format!("T3 Code policy for {}", document.organization)),
    );
    profile.insert(
        "PayloadIdentifier".to_owned(),
        Value::String("com.t3tools.t3code.policy".to_owned()),
    );
    profile.insert(
        "PayloadOrganization".to_owned(),
        Value::String(document.organization.clone()),
    );
    profile.insert(
        "PayloadScope".to_owned(),
        Value::String("System".to_owned()),
    );
    profile.insert(
        "PayloadType".to_owned(),
        Value::String("Configuration".to_owned()),
    );
    profile.insert(
        "PayloadUUID".to_owned(),
        Value::String(profile_uuid.to_owned()),
    );
    profile.insert("PayloadVersion".to_owned(), Value::Number(1.into()));

    let mut plist_body = String::new();
    plist_value(&Value::Object(profile), "", &mut plist_body);
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
{plist_body}
</plist>
"
    ))
}
