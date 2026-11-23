//! Provider instance configuration loading from `settings.json` for driver resolution.

use std::collections::BTreeMap;
use std::path::Path;

use legion_providers::drivers::{normalize_driver_slug, tokenize_cli_args};
use serde_json::Value;

pub(super) struct InstanceConfig {
    pub(super) driver: String,
    pub(super) binary_path: Option<String>,
    pub(super) home_path: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) extra_args: Vec<String>,
    pub(super) env: BTreeMap<String, String>,
}

pub(super) fn read_instance_config(
    settings_path: Option<&Path>,
    instance_id: &str,
) -> InstanceConfig {
    let mut cfg = InstanceConfig {
        driver: normalize_driver_slug(instance_id).to_owned(),
        binary_path: None,
        home_path: None,
        base_url: None,
        api_key: None,
        extra_args: Vec::new(),
        env: BTreeMap::new(),
    };
    let Some(path) = settings_path else {
        return cfg;
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return cfg;
    };
    let Ok(root) = serde_json::from_str::<Value>(&raw) else {
        return cfg;
    };
    if let Some(inst) = root
        .get("providerInstances")
        .and_then(|m| m.get(instance_id))
    {
        if let Some(drv) = inst.get("driver").and_then(Value::as_str) {
            normalize_driver_slug(drv).clone_into(&mut cfg.driver);
        }
        collect_instance_env(inst.get("environment"), &mut cfg.env);
        if let Some(inner) = inst.get("config") {
            apply_inner_config(inner, &mut cfg);
        }
        apply_inner_config(inst, &mut cfg);
    } else if let Some(legacy) = root.get("providers").and_then(|m| m.get(&cfg.driver)) {
        apply_inner_config(legacy, &mut cfg);
    }
    cfg
}

fn collect_instance_env(env_val: Option<&Value>, out: &mut BTreeMap<String, String>) {
    match env_val {
        Some(Value::Array(items)) => {
            for item in items {
                let Some(name) = item
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                if let Some(val) = item.get("value").and_then(Value::as_str) {
                    out.insert(name.to_owned(), val.to_owned());
                }
            }
        }
        Some(Value::Object(map)) => {
            for (k, v) in map {
                if let Some(s) = v.as_str() {
                    out.insert(k.clone(), s.to_owned());
                }
            }
        }
        _ => {}
    }
}

fn push_arg_list(val: Option<&Value>, out: &mut Vec<String>) {
    match val {
        Some(Value::String(s)) if !s.trim().is_empty() => {
            out.extend(tokenize_cli_args(s));
        }
        Some(Value::Array(arr)) => {
            for item in arr {
                if let Some(s) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    out.push(s.to_owned());
                }
            }
        }
        _ => {}
    }
}

fn apply_inner_config(val: &Value, cfg: &mut InstanceConfig) {
    let pick_str = |key: &str| {
        val.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
    };
    if cfg.binary_path.is_none() {
        cfg.binary_path = pick_str("binaryPath")
            .or_else(|| pick_str("commandPath"))
            .or_else(|| pick_str("executable"));
    }
    if cfg.home_path.is_none() {
        cfg.home_path = pick_str("homePath");
    }
    if cfg.base_url.is_none() {
        cfg.base_url = pick_str("baseUrl");
    }
    if cfg.api_key.is_none() {
        cfg.api_key = pick_str("apiKey");
    }
    push_arg_list(
        val.get("launchArgs").or_else(|| val.get("commandArgs")),
        &mut cfg.extra_args,
    );
}
