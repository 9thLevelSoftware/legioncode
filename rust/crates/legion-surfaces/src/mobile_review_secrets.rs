//! Multi-value and show/hide secret request card state (`0bc9b57b93` #31257).

use std::collections::{BTreeMap, BTreeSet};

use legion_contracts::ids::{ThreadId, TurnItemId};
use legion_contracts::orchestration::SecretRequestField;
use legion_contracts::secrets::{SecretRequestAnswer, SecretRequestAnswerInput};
use serde::{Deserialize, Serialize};

/// Normalized input specification for a secret-request prompt card (`#31257`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretInputSpec {
    /// Empty string for legacy single-value cards; field `name` for multi-value cards.
    pub key: String,
    /// User-visible input label.
    pub label: String,
    /// Placeholder text.
    pub placeholder: String,
    /// Whether the input starts masked.
    pub secret: bool,
    /// Whether the UI exposes a show/hide toggle for this field.
    pub revealable: bool,
}

/// Builds normalized input specs for either a single-value card (`fields == None`) or a
/// multi-value card (`fields == Some(...)`, `#31257`).
#[must_use]
pub fn secret_request_inputs(
    label: &str,
    placeholder: Option<&str>,
    fields: Option<&[SecretRequestField]>,
) -> Vec<SecretInputSpec> {
    let Some(fields) = fields else {
        return vec![SecretInputSpec {
            key: String::new(),
            label: label.to_owned(),
            placeholder: placeholder.unwrap_or("Paste the secret").to_owned(),
            secret: true,
            revealable: true,
        }];
    };
    fields
        .iter()
        .map(|field| SecretInputSpec {
            key: field.name.clone(),
            label: field.label.clone(),
            placeholder: field.placeholder.clone().unwrap_or_default(),
            secret: field.is_secret(),
            revealable: field.revealable,
        })
        .collect()
}

/// Returns the user-facing privacy note for file-delivered vs. store-backed secret cards.
#[must_use]
pub const fn secret_request_privacy_note(delivery_file: bool) -> &'static str {
    if delivery_file {
        "Kept out of chat, in a private file this agent's commands can read for 24 hours"
    } else {
        "Stored securely, never shown to the agent"
    }
}

/// Interactive multi-value and show/hide mask state for a secret request card (`#31257`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretCardState {
    is_multi_field: bool,
    inputs: Vec<SecretInputSpec>,
    values: BTreeMap<String, String>,
    revealed: BTreeSet<String>,
}

impl SecretCardState {
    /// Initializes a card state for `inputs` (`is_multi_field` true when `fields` was provided).
    #[must_use]
    pub fn new(
        label: &str,
        placeholder: Option<&str>,
        fields: Option<&[SecretRequestField]>,
    ) -> Self {
        Self {
            is_multi_field: fields.is_some(),
            inputs: secret_request_inputs(label, placeholder, fields),
            values: BTreeMap::new(),
            revealed: BTreeSet::new(),
        }
    }

    /// Input specifications rendered in order.
    #[must_use]
    pub fn inputs(&self) -> &[SecretInputSpec] {
        &self.inputs
    }

    /// Updates the current typed value for `key`.
    pub fn set_value(&mut self, key: &str, value: impl Into<String>) {
        self.values.insert(key.to_owned(), value.into());
    }

    /// Returns the typed value for `key`, or `""` when unset.
    #[must_use]
    pub fn value(&self, key: &str) -> &str {
        self.values.get(key).map_or("", String::as_str)
    }

    /// Toggles plaintext visibility for secret input `key`; non-secret fields are always visible.
    pub fn toggle_reveal(&mut self, key: &str) -> bool {
        let Some(spec) = self.inputs.iter().find(|input| input.key == key) else {
            return false;
        };
        if !spec.secret {
            return true;
        }
        if !spec.revealable {
            return false;
        }
        if self.revealed.remove(key) {
            false
        } else {
            self.revealed.insert(key.to_owned());
            true
        }
    }

    /// Returns `true` when input `key` is displayed in plaintext (either non-secret or revealed).
    #[must_use]
    pub fn is_revealed(&self, key: &str) -> bool {
        self.inputs
            .iter()
            .find(|input| input.key == key)
            .is_some_and(|spec| !spec.secret || self.revealed.contains(key))
    }

    /// Returns `true` only once every input in the card has a non-empty trimmed value.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.inputs.is_empty()
            && self
                .inputs
                .iter()
                .all(|input| !self.value(&input.key).trim().is_empty())
    }

    /// Builds [`SecretRequestAnswer::Save`] when every field has a non-empty trimmed value.
    #[must_use]
    pub fn to_save_answer(&self) -> Option<SecretRequestAnswer> {
        if !self.is_ready() {
            return None;
        }
        if self.is_multi_field {
            let values = self
                .inputs
                .iter()
                .map(|input| (input.key.clone(), self.value(&input.key).trim().to_owned()))
                .collect();
            Some(SecretRequestAnswer::Save {
                secret: None,
                values: Some(values),
            })
        } else {
            let secret = self
                .inputs
                .first()
                .map(|input| self.value(&input.key).trim().to_owned())?;
            Some(SecretRequestAnswer::Save {
                secret: Some(secret),
                values: None,
            })
        }
    }

    /// Wraps [`Self::to_save_answer`] in a typed [`SecretRequestAnswerInput`] when ready.
    #[must_use]
    pub fn to_answer_input(
        &self,
        thread_id: ThreadId,
        turn_item_id: TurnItemId,
    ) -> Option<SecretRequestAnswerInput> {
        Some(SecretRequestAnswerInput {
            thread_id,
            turn_item_id,
            answer: self.to_save_answer()?,
        })
    }
}

/// Builds a typed decline payload for a secret request card.
#[must_use]
pub fn decline_secret_request_input(
    thread_id: ThreadId,
    turn_item_id: TurnItemId,
) -> SecretRequestAnswerInput {
    SecretRequestAnswerInput {
        thread_id,
        turn_item_id,
        answer: SecretRequestAnswer::Decline,
    }
}
