//! Typed Cloudflare R2 object storage operations (`ARTIFACTS`, `CLIENTS`) for
//! `infra/task-cache`.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::task_cache::{
    CompletedPart, PartValidationError, validate_complete_parts, validate_part_number,
};

/// Target Cloudflare R2 object storage binding identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum R2BucketBinding {
    /// Task-cache binary metadata values and blob objects (`ARTIFACTS`).
    Artifacts,
    /// Native mobile dev client archives (`CLIENTS`).
    Clients,
}

impl R2BucketBinding {
    /// Canonical Wrangler/Alchemy R2 binding name (`"ARTIFACTS"` or `"CLIENTS"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Artifacts => "ARTIFACTS",
            Self::Clients => "CLIENTS",
        }
    }
}

impl fmt::Display for R2BucketBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Typed Cloudflare R2 bucket operation planned for `"ARTIFACTS"` or `"CLIENTS"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum R2BindingOp {
    /// Fetches an object by key (`bucket.get(key)`).
    Get {
        /// Target R2 bucket binding.
        bucket: R2BucketBinding,
        /// Object key.
        key: String,
    },
    /// Stores an object payload (`bucket.put(key, body)`).
    Put {
        /// Target R2 bucket binding.
        bucket: R2BucketBinding,
        /// Object key.
        key: String,
        /// Raw object payload bytes.
        body: Vec<u8>,
    },
    /// Deletes one or more object keys (`bucket.delete(keys)`).
    Delete {
        /// Target R2 bucket binding.
        bucket: R2BucketBinding,
        /// Keys scheduled for removal.
        keys: Vec<String>,
    },
    /// Starts a multipart upload (`bucket.createMultipartUpload(key, metadata)`).
    CreateMultipartUpload {
        /// Target R2 bucket binding.
        bucket: R2BucketBinding,
        /// Object key.
        key: String,
        /// Optional custom metadata headers (`digest`, `writer`, `buildMs`).
        custom_metadata: BTreeMap<String, String>,
    },
    /// Uploads a single multipart chunk (`1..=64`, enforcing `BUG-30`).
    UploadPart {
        /// Target R2 bucket binding.
        bucket: R2BucketBinding,
        /// Object key.
        key: String,
        /// Multipart upload identifier returned by R2.
        upload_id: String,
        /// Validated one-based part number (`1..=MAX_MULTIPART_PARTS`).
        part_number: u32,
        /// Chunk bytes uploaded for this part.
        body: Vec<u8>,
    },
    /// Finalizes a multipart upload with validated part `ETag` descriptors.
    CompleteMultipartUpload {
        /// Target R2 bucket binding.
        bucket: R2BucketBinding,
        /// Object key.
        key: String,
        /// Multipart upload identifier.
        upload_id: String,
        /// Non-empty unique part list (`1..=64`).
        parts: Vec<CompletedPart>,
    },
    /// Aborts an incomplete or expired multipart upload (`upload.abort()`).
    AbortMultipartUpload {
        /// Target R2 bucket binding.
        bucket: R2BucketBinding,
        /// Object key.
        key: String,
        /// Multipart upload identifier.
        upload_id: String,
    },
}

impl R2BindingOp {
    /// Constructs a `Get` operation.
    pub fn get(bucket: R2BucketBinding, key: impl Into<String>) -> Self {
        Self::Get {
            bucket,
            key: key.into(),
        }
    }

    /// Constructs a `Put` operation.
    pub fn put(bucket: R2BucketBinding, key: impl Into<String>, body: Vec<u8>) -> Self {
        Self::Put {
            bucket,
            key: key.into(),
            body,
        }
    }

    /// Constructs a `Delete` operation.
    pub fn delete(bucket: R2BucketBinding, keys: Vec<String>) -> Self {
        Self::Delete { bucket, keys }
    }

    /// Constructs a `CreateMultipartUpload` operation.
    pub fn create_multipart_upload(
        bucket: R2BucketBinding,
        key: impl Into<String>,
        custom_metadata: BTreeMap<String, String>,
    ) -> Self {
        Self::CreateMultipartUpload {
            bucket,
            key: key.into(),
            custom_metadata,
        }
    }

    /// Constructs an `UploadPart` operation after validating `part_number` in `1..=64` (`BUG-30`).
    pub fn upload_part(
        bucket: R2BucketBinding,
        key: impl Into<String>,
        upload_id: impl Into<String>,
        part_number: u32,
        body: Vec<u8>,
    ) -> Result<Self, PartValidationError> {
        let valid_part = validate_part_number(part_number)?;
        Ok(Self::UploadPart {
            bucket,
            key: key.into(),
            upload_id: upload_id.into(),
            part_number: valid_part,
            body,
        })
    }

    /// Constructs a `CompleteMultipartUpload` operation after validating `parts` (`1..=64`, unique).
    pub fn complete_multipart_upload(
        bucket: R2BucketBinding,
        key: impl Into<String>,
        upload_id: impl Into<String>,
        parts: Vec<CompletedPart>,
    ) -> Result<Self, PartValidationError> {
        validate_complete_parts(&parts)?;
        Ok(Self::CompleteMultipartUpload {
            bucket,
            key: key.into(),
            upload_id: upload_id.into(),
            parts,
        })
    }

    /// Constructs an `AbortMultipartUpload` operation.
    pub fn abort_multipart_upload(
        bucket: R2BucketBinding,
        key: impl Into<String>,
        upload_id: impl Into<String>,
    ) -> Self {
        Self::AbortMultipartUpload {
            bucket,
            key: key.into(),
            upload_id: upload_id.into(),
        }
    }

    /// Validates part invariants on any [`R2BindingOp`] (useful after deserialization).
    pub fn validate(&self) -> Result<(), PartValidationError> {
        match self {
            Self::UploadPart { part_number, .. } => validate_part_number(*part_number).map(|_| ()),
            Self::CompleteMultipartUpload { parts, .. } => validate_complete_parts(parts),
            Self::Get { .. }
            | Self::Put { .. }
            | Self::Delete { .. }
            | Self::CreateMultipartUpload { .. }
            | Self::AbortMultipartUpload { .. } => Ok(()),
        }
    }

    /// Returns the target R2 bucket binding for this operation.
    pub const fn bucket(&self) -> R2BucketBinding {
        match self {
            Self::Get { bucket, .. }
            | Self::Put { bucket, .. }
            | Self::Delete { bucket, .. }
            | Self::CreateMultipartUpload { bucket, .. }
            | Self::UploadPart { bucket, .. }
            | Self::CompleteMultipartUpload { bucket, .. }
            | Self::AbortMultipartUpload { bucket, .. } => *bucket,
        }
    }
}
