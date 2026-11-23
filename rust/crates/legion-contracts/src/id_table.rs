//! Non-mutating compact ID-table interning and expansion for orchestration payloads.
//!
//! Mirrors `packages/contracts/src/orchestrationV2IdTable.ts` while fixing BUG-20:
//! - Both [`intern_record_ids`] and [`expand_record_ids`] are strictly non-mutating
//!   (read-only `&Value` inputs, fresh `Value` outputs).
//! - Negative, non-integer, or out-of-range table indices return
//!   `Err(IdTableError::OutOfBounds)` instead of producing `undefined` holes.
//!
//! # Examples
//!
//! ```
//! use serde_json::json;
//! use legion_contracts::id_table::{IdTableError, expand_record_ids, intern_record_ids};
//!
//! let container = json!({
//!     "rows": [
//!         { "id": "row:a", "parentIds": ["row:b"], "item": { "ownerId": "row:b" } },
//!         { "id": "row:b", "parentIds": [], "item": { "ownerId": null } }
//!     ]
//! });
//! let mut ids = Vec::new();
//! let compacted = intern_record_ids(&container, &mut ids)?;
//! assert_eq!(ids, vec![json!("row:a"), json!("row:b")]);
//!
//! let restored = expand_record_ids(&compacted, &ids)?;
//! assert_eq!(restored, container);
//!
//! let bad = json!({ "rows": [{ "id": 99 }] });
//! assert_eq!(expand_record_ids(&bad, &ids), Err(IdTableError::OutOfBounds));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::HashMap;

use serde_json::{Map, Number, Value};
use thiserror::Error;

/// Error returned during non-mutating ID-table compaction or expansion.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdTableError {
    /// An ID slot held a negative, fractional, or out-of-range table index.
    #[error("ID table index out of bounds")]
    OutOfBounds,
    /// A container or target passed to ID-table processing was not a JSON object.
    #[error("expected JSON object for ID table container")]
    ExpectedObject,
}

/// Target location of the internable records within a response envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdTableTarget<'a> {
    /// Records live directly on the top-level object (e.g., history pages).
    Root,
    /// Records live under a named object field (e.g., `"projection"`).
    Field(&'a str),
    /// This message kind carries no internable records.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum IdKey {
    Str(String),
    Int(i128),
    FloatBits(u64),
}

impl IdKey {
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::String(s) => Some(Self::Str(s.clone())),
            Value::Number(n) => Some(Self::from_number(n)),
            _ => None,
        }
    }

    fn from_number(n: &Number) -> Self {
        if let Some(i) = n.as_i64() {
            Self::Int(i128::from(i))
        } else if let Some(u) = n.as_u64() {
            Self::Int(i128::from(u))
        } else if let Some(f) = n.as_f64() {
            let normalized = if f == 0.0 { 0.0_f64 } else { f };
            Self::FloatBits(normalized.to_bits())
        } else {
            Self::Str(n.to_string())
        }
    }
}

fn is_id_key(key: &str) -> bool {
    key == "id" || key.ends_with("Id")
}

fn is_id_list_key(key: &str) -> bool {
    key.ends_with("Ids")
}

fn intern_ref(value: &Value, ids: &mut Vec<Value>, index: &mut HashMap<IdKey, usize>) -> Value {
    let Some(key) = IdKey::from_value(value) else {
        return value.clone();
    };
    if let Some(&pos) = index.get(&key) {
        return Value::Number(Number::from(pos));
    }
    let pos = ids.len();
    ids.push(value.clone());
    index.insert(key, pos);
    Value::Number(Number::from(pos))
}

fn intern_record(
    record: &Map<String, Value>,
    ids: &mut Vec<Value>,
    index: &mut HashMap<IdKey, usize>,
) -> Value {
    let mut out = Map::with_capacity(record.len());
    for (key, value) in record {
        let mapped = if is_id_key(key) {
            intern_ref(value, ids, index)
        } else if is_id_list_key(key) {
            match value {
                Value::Array(items) => Value::Array(
                    items
                        .iter()
                        .map(|item| intern_ref(item, ids, index))
                        .collect(),
                ),
                other => other.clone(),
            }
        } else if key == "item" {
            match value {
                Value::Object(nested) => intern_record(nested, ids, index),
                other => other.clone(),
            }
        } else {
            value.clone()
        };
        out.insert(key.clone(), mapped);
    }
    Value::Object(out)
}

/// Non-mutatingly interns `id`, `*Id`, and `*Ids` values from `container` into `ids`.
pub fn intern_record_ids(container: &Value, ids: &mut Vec<Value>) -> Result<Value, IdTableError> {
    let Some(obj) = container.as_object() else {
        return Err(IdTableError::ExpectedObject);
    };
    let mut index = HashMap::with_capacity(ids.len());
    for (pos, entry) in ids.iter().enumerate() {
        if let Some(key) = IdKey::from_value(entry) {
            index.entry(key).or_insert(pos);
        }
    }

    let mut out = Map::with_capacity(obj.len());
    for (key, value) in obj {
        let mapped = match value {
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| match item {
                        Value::Object(record) => intern_record(record, ids, &mut index),
                        other => other.clone(),
                    })
                    .collect(),
            ),
            Value::Object(record) => intern_record(record, ids, &mut index),
            other => other.clone(),
        };
        out.insert(key.clone(), mapped);
    }
    Ok(Value::Object(out))
}

fn resolve_id_entry(value: &Value, ids: &[Value]) -> Result<Value, IdTableError> {
    let Value::Number(num) = value else {
        return Ok(value.clone());
    };
    let idx = num
        .as_u64()
        .and_then(|u| usize::try_from(u).ok())
        .ok_or(IdTableError::OutOfBounds)?;
    ids.get(idx).cloned().ok_or(IdTableError::OutOfBounds)
}

fn expand_record(record: &Map<String, Value>, ids: &[Value]) -> Result<Value, IdTableError> {
    let mut out = Map::with_capacity(record.len());
    for (key, value) in record {
        let expanded = if is_id_key(key) && value.is_number() {
            resolve_id_entry(value, ids)?
        } else if is_id_list_key(key) {
            match value {
                Value::Array(entries) => {
                    let mut list = Vec::with_capacity(entries.len());
                    for entry in entries {
                        list.push(resolve_id_entry(entry, ids)?);
                    }
                    Value::Array(list)
                }
                other => other.clone(),
            }
        } else if key == "item" {
            match value {
                Value::Object(nested) => expand_record(nested, ids)?,
                other => other.clone(),
            }
        } else {
            value.clone()
        };
        out.insert(key.clone(), expanded);
    }
    Ok(Value::Object(out))
}

/// Non-mutatingly expands table indices in `container` using `ids`.
///
/// Any negative, non-integer, or out-of-range index returns
/// `Err(IdTableError::OutOfBounds)`.
pub fn expand_record_ids(container: &Value, ids: &[Value]) -> Result<Value, IdTableError> {
    let Some(obj) = container.as_object() else {
        return Err(IdTableError::ExpectedObject);
    };
    let mut out = Map::with_capacity(obj.len());
    for (key, value) in obj {
        let expanded = match value {
            Value::Array(items) => {
                let mut arr = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        Value::Object(record) => arr.push(expand_record(record, ids)?),
                        other => arr.push(other.clone()),
                    }
                }
                Value::Array(arr)
            }
            Value::Object(record) => expand_record(record, ids)?,
            other => other.clone(),
        };
        out.insert(key.clone(), expanded);
    }
    Ok(Value::Object(out))
}

/// Non-mutatingly encodes an envelope with an `ids` table when `"idTable": true` is set.
pub fn encode_with_id_table(
    encoded: &Value,
    target: IdTableTarget<'_>,
) -> Result<Value, IdTableError> {
    let Some(obj) = encoded.as_object() else {
        return Ok(encoded.clone());
    };
    if obj.get("idTable") != Some(&Value::Bool(true)) {
        return Ok(encoded.clone());
    }
    let mut rest = Map::with_capacity(obj.len());
    for (key, value) in obj {
        if key != "idTable" {
            rest.insert(key.clone(), value.clone());
        }
    }
    let mut ids = Vec::new();
    match target {
        IdTableTarget::None => Ok(Value::Object(rest)),
        IdTableTarget::Root => {
            let mut interned = intern_record_ids(&Value::Object(rest), &mut ids)?;
            if let Some(map) = interned.as_object_mut() {
                map.insert("ids".to_owned(), Value::Array(ids));
            }
            Ok(interned)
        }
        IdTableTarget::Field(field) => {
            let Some(container) = rest.get(field) else {
                return Ok(Value::Object(rest));
            };
            if !container.is_object() {
                return Ok(Value::Object(rest));
            }
            let interned = intern_record_ids(container, &mut ids)?;
            rest.insert(field.to_owned(), interned);
            rest.insert("ids".to_owned(), Value::Array(ids));
            Ok(Value::Object(rest))
        }
    }
}

/// Non-mutatingly expands an incoming envelope's `ids` table before schema decoding.
pub fn decode_with_id_table(raw: &Value, target: IdTableTarget<'_>) -> Result<Value, IdTableError> {
    let Some(obj) = raw.as_object() else {
        return Ok(raw.clone());
    };
    let Some(Value::Array(ids)) = obj.get("ids") else {
        return Ok(raw.clone());
    };
    let mut rest = Map::with_capacity(obj.len());
    for (key, value) in obj {
        if key != "ids" {
            rest.insert(key.clone(), value.clone());
        }
    }
    match target {
        IdTableTarget::None => Ok(Value::Object(rest)),
        IdTableTarget::Root => expand_record_ids(&Value::Object(rest), ids),
        IdTableTarget::Field(field) => {
            let Some(container) = rest.get(field) else {
                return Ok(Value::Object(rest));
            };
            if !container.is_object() {
                return Ok(Value::Object(rest));
            }
            let expanded = expand_record_ids(container, ids)?;
            rest.insert(field.to_owned(), expanded);
            Ok(Value::Object(rest))
        }
    }
}
