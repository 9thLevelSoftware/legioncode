//! Windows Group Policy `.admx` and `.adml` template generators (`policyTemplates.ts`).

use super::windows_data::{ADMX_KEY, Element, WINDOWS_POLICY_GROUPS, policies};

pub(super) fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn element_id(element: &Element<'_>) -> String {
    match element {
        Element::Checkbox {
            list: Some(list),
            name,
            ..
        } => format!("{list}_{name}"),
        Element::Text { name, .. }
        | Element::Enum { name, .. }
        | Element::Checkbox { name, .. } => (*name).to_owned(),
    }
}

fn admx_element(element: &Element<'_>) -> String {
    let id = element_id(element);
    match element {
        Element::Text { name, json, .. } => {
            let required = if *name == "Organization" {
                " required=\"true\""
            } else {
                ""
            };
            let max_len = if *json { " maxLength=\"32767\"" } else { "" };
            format!("<text id=\"{id}\" valueName=\"{name}\"{required}{max_len} />")
        }
        Element::Enum { name, items, .. } => {
            let mut lines = vec![format!(
                "<enum id=\"{id}\" valueName=\"{name}\" required=\"true\">"
            )];
            for (value, _) in *items {
                lines.push(format!(
                    "  <item displayName=\"$(string.{id}_{value})\"><value><string>{value}</string></value></item>"
                ));
            }
            lines.push("</enum>".to_owned());
            lines.join("\n          ")
        }
        Element::Checkbox { list, name, .. } => {
            let key_attr = match list {
                Some(list_name) => format!(" key=\"{ADMX_KEY}\\{list_name}\""),
                None => String::new(),
            };
            let escaped_name = escape_xml(name);
            format!(
                "<boolean id=\"{id}\"{key_attr} valueName=\"{escaped_name}\"><trueValue><decimal value=\"1\" /></trueValue><falseValue><decimal value=\"0\" /></falseValue></boolean>"
            )
        }
    }
}

/// Generates the language-neutral Windows Group Policy `.admx` template.
pub fn windows_admx() -> String {
    let mut body = Vec::new();
    for policy in policies() {
        let head = format!(
            "<policy name=\"{id}\" class=\"Machine\" displayName=\"$(string.{id})\" explainText=\"$(string.{id}_Help)\" key=\"{ADMX_KEY}\"",
            id = policy.id
        );
        let common = "<parentCategory ref=\"LegionCode\" />\n      <supportedOn ref=\"SUPPORTED_LegionCode\" />";
        if let Some(toggle) = policy.toggle {
            body.push(format!(
                "    {head} valueName=\"{toggle}\">\n      {common}\n      <enabledValue><decimal value=\"1\" /></enabledValue>\n      <disabledValue><decimal value=\"0\" /></disabledValue>\n    </policy>"
            ));
            continue;
        }
        let grouped = WINDOWS_POLICY_GROUPS.contains(&policy.id);
        let value_name = if grouped {
            format!(" valueName=\"{}\"", policy.id)
        } else {
            String::new()
        };
        let group_values = if grouped {
            "\n      <enabledValue><decimal value=\"1\" /></enabledValue>\n      <disabledValue><decimal value=\"0\" /></disabledValue>"
        } else {
            ""
        };
        let rendered_elements: Vec<String> = policy
            .elements
            .unwrap_or_default()
            .iter()
            .map(admx_element)
            .collect();
        body.push(format!(
            "    {head}{value_name} presentation=\"$(presentation.{id})\">\n      {common}{group_values}\n      <elements>\n        {elements}\n      </elements>\n    </policy>",
            id = policy.id,
            elements = rendered_elements.join("\n        "),
        ));
    }

    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>
<policyDefinitions xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" revision=\"1.0\" schemaVersion=\"1.0\" xmlns=\"http://schemas.microsoft.com/GroupPolicy/2006/07/PolicyDefinitions\">
  <policyNamespaces>
    <target prefix=\"legioncode\" namespace=\"LegionCode.Policies.LegionCode\" />
  </policyNamespaces>
  <resources minRequiredRevision=\"1.0\" />
  <supportedOn>
    <definitions>
      <definition name=\"SUPPORTED_LegionCode\" displayName=\"$(string.SUPPORTED_LegionCode)\" />
    </definitions>
  </supportedOn>
  <categories>
    <category name=\"LegionCode\" displayName=\"$(string.LegionCode)\" />
  </categories>
  <policies>
{}
  </policies>
</policyDefinitions>
",
        body.join("\n")
    )
}

fn presentation_element(element: &Element<'_>) -> String {
    let id = element_id(element);
    match element {
        Element::Text { label, .. } => {
            format!(
                "<textBox refId=\"{id}\"><label>{}</label></textBox>",
                escape_xml(label)
            )
        }
        Element::Enum {
            label,
            default_item,
            ..
        } => {
            format!(
                "<dropdownList refId=\"{id}\" noSort=\"true\" defaultItem=\"{default_item}\">{}</dropdownList>",
                escape_xml(label)
            )
        }
        Element::Checkbox {
            label,
            default_checked,
            ..
        } => {
            format!(
                "<checkBox refId=\"{id}\" defaultChecked=\"{default_checked}\">{}</checkBox>",
                escape_xml(label)
            )
        }
    }
}

/// Generates the `en-US` `.adml` strings and dialog layouts paired with [`windows_admx`].
pub fn windows_adml() -> String {
    let mut strings: Vec<(String, String)> = vec![
        ("LegionCode".to_owned(), "LegionCode".to_owned()),
        (
            "SUPPORTED_LegionCode".to_owned(),
            "LegionCode with managed policy support".to_owned(),
        ),
    ];
    let mut presentations = Vec::new();
    for policy in policies() {
        strings.push((policy.id.to_owned(), policy.title.to_owned()));
        strings.push((format!("{}_Help", policy.id), policy.help.to_owned()));
        if let Some(elements) = policy.elements {
            for element in &elements {
                if let Element::Enum { items, .. } = element {
                    let elem_id = element_id(element);
                    for (value, label) in *items {
                        strings.push((format!("{elem_id}_{value}"), (*label).to_owned()));
                    }
                }
            }
            let rendered: Vec<String> = elements.iter().map(presentation_element).collect();
            presentations.push(format!(
                "      <presentation id=\"{}\">\n        {}\n      </presentation>",
                policy.id,
                rendered.join("\n        ")
            ));
        }
    }
    let string_table: Vec<String> = strings
        .iter()
        .map(|(id, text)| format!("      <string id=\"{id}\">{}</string>", escape_xml(text)))
        .collect();

    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>
<policyDefinitionResources xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" revision=\"1.0\" schemaVersion=\"1.0\" xmlns=\"http://schemas.microsoft.com/GroupPolicy/2006/07/PolicyDefinitions\">
  <displayName>LegionCode</displayName>
  <description>Policies for LegionCode</description>
  <resources>
    <stringTable>
{}
    </stringTable>
    <presentationTable>
{}
    </presentationTable>
  </resources>
</policyDefinitionResources>
",
        string_table.join("\n"),
        presentations.join("\n")
    )
}
