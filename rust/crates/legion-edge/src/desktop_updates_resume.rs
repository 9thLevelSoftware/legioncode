//! HTTP `Range: bytes=<offset>-` download resume planner and SHA-512 artifact verifier.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use sha2::{Digest as _, Sha512};
use thiserror::Error;

/// Maximum automatic retries after an idle stall or `net::ERR_TIMED_OUT` (`electron-updater` patch).
pub const DOWNLOAD_STALL_RETRIES: u32 = 3;

/// Initial backoff delay before the first download retry (`2_000 ms`, doubling to `4_000`, `8_000`).
pub const RETRY_BASE_BACKOFF_MS: u64 = 2_000;

/// Errors emitted by SHA-512 artifact digest verification.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DigestError {
    /// Expected digest was neither 128 hex characters nor valid standard base64 for 64 bytes.
    #[error("expected sha512 digest is neither 128-char hex nor 64-byte base64")]
    InvalidExpectedFormat,
    /// Computed artifact digest did not match the release manifest digest.
    #[error("sha512 checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch {
        /// Digest string from the release manifest.
        expected: String,
        /// Computed lowercase hex SHA-512 digest of the downloaded bytes.
        actual: String,
    },
}

/// Computes the standard base64-encoded SHA-512 digest matching electron-builder `latest.yml`.
pub fn compute_sha512_base64(bytes: &[u8]) -> String {
    BASE64_STANDARD.encode(Sha512::digest(bytes))
}

/// Computes the 128-character lowercase hexadecimal SHA-512 digest of `bytes`.
pub fn compute_sha512_hex(bytes: &[u8]) -> String {
    hex::encode(Sha512::digest(bytes))
}

/// Verifies `bytes` against an expected SHA-512 digest in either 128-char hex or base64 encoding.
pub fn verify_sha512(bytes: &[u8], expected: &str) -> Result<(), DigestError> {
    let mut verifier = Sha512Verifier::new();
    verifier.update(bytes);
    verifier.finish(expected)
}

/// Streaming SHA-512 verifier for chunked or resumed update downloads.
#[derive(Debug, Clone, Default)]
pub struct Sha512Verifier {
    hasher: Sha512,
}

impl Sha512Verifier {
    /// Starts a new SHA-512 verification stream.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the next chunk of downloaded bytes into the SHA-512 state.
    pub fn update(&mut self, chunk: &[u8]) {
        self.hasher.update(chunk);
    }

    /// Finalizes the digest and verifies it in constant time against `expected` (hex or base64).
    pub fn finish(self, expected: &str) -> Result<(), DigestError> {
        let actual_bytes: [u8; 64] = self.hasher.finalize().into();
        let expected_bytes = decode_expected_sha512(expected)?;
        let diff = actual_bytes
            .iter()
            .zip(expected_bytes.iter())
            .fold(0_u8, |acc, (a, b)| acc | (a ^ b));
        if diff != 0 {
            return Err(DigestError::ChecksumMismatch {
                expected: expected.to_owned(),
                actual: hex::encode(actual_bytes),
            });
        }
        Ok(())
    }
}

fn decode_expected_sha512(expected: &str) -> Result<[u8; 64], DigestError> {
    let trimmed = expected.trim();
    if trimmed.len() == 128 && !trimmed.contains(['+', '/', '=']) {
        let decoded = hex::decode(trimmed).map_err(|_| DigestError::InvalidExpectedFormat)?;
        let array: [u8; 64] = decoded
            .try_into()
            .map_err(|_| DigestError::InvalidExpectedFormat)?;
        return Ok(array);
    }
    let decoded = BASE64_STANDARD
        .decode(trimmed)
        .map_err(|_| DigestError::InvalidExpectedFormat)?;
    decoded
        .try_into()
        .map_err(|_| DigestError::InvalidExpectedFormat)
}

/// Failure category deciding whether [`DownloadResumePlanner`] schedules a ranged retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadFailureKind {
    /// Request stalled (`Request timed out`) or timed out at network layer (`net::ERR_TIMED_OUT`).
    StallOrTimeout,
    /// Non-timeout failure (e.g. `ERR_CONNECTION_REFUSED`) that aborts immediately.
    Other,
}

/// Server HTTP response metadata evaluated by [`DownloadResumePlanner::plan_response`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpDownloadResponse {
    /// `200 OK` full response (server ignored `Range` or this was the initial attempt).
    Ok200,
    /// `206 Partial Content` carrying the `Content-Range` response header, if present.
    PartialContent206 {
        /// Raw `Content-Range` header value (e.g. `bytes 1048576-1048580/1048581`).
        content_range: Option<String>,
    },
    /// `416 Range Not Satisfiable` (file may already contain the entire release).
    RangeNotSatisfiable416,
    /// Any other HTTP status code (`>= 400`).
    OtherError {
        /// HTTP status code.
        status: u16,
    },
}

/// Concrete disk/digest action returned by [`DownloadResumePlanner::plan_response`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseAction {
    /// Truncate/overwrite destination from byte `0` and hash incoming bytes directly.
    RestartFromZero,
    /// Append to destination starting at `offset`, then verify SHA-512 over the whole file.
    AppendAndVerifyWholeFile {
        /// Byte offset verified against `Content-Range`.
        offset: u64,
    },
    /// Verify the SHA-512 digest of the bytes already on disk after a `416` response.
    VerifyExistingFile,
}

/// Failure retry decision returned by [`DownloadResumePlanner::plan_failure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryPlan {
    /// Retry after `backoff_ms`, sending `Range: bytes=<range_offset>-` when `range_offset > 0`.
    Retry {
        /// 1-based retry attempt number (`1..=max_retries`).
        retry_index: u32,
        /// Exponential backoff duration (`2_000`, `4_000`, `8_000` ms).
        backoff_ms: u64,
        /// Byte offset written by this download session (`0` if nothing written yet).
        range_offset: u64,
    },
    /// Exhausted `max_retries` or hit a non-stall error; abort the download.
    Abort,
}

/// Errors produced when a server response violates HTTP range resume invariants.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResumeError {
    /// Server returned `206` without a matching `bytes <offset>-` `Content-Range`.
    #[error("cannot resume download at byte {offset}, got Content-Range {content_range:?}")]
    InvalidContentRange {
        /// Requested resume byte offset.
        offset: u64,
        /// Received `Content-Range` header, if any.
        content_range: Option<String>,
    },
    /// Server returned an unexpected HTTP error status.
    #[error("unexpected HTTP status {status} during update download")]
    UnexpectedStatus {
        /// HTTP status code.
        status: u16,
    },
}

/// Plans `Range: bytes=<offset>-` retries and digest checks for stalled update downloads.
///
/// Mirrors `patches/electron-updater@6.8.9.patch`: only bytes written by the current session
/// count toward `offset` (pre-existing differential output is never resumed blindly), stalls
/// retry up to `max_retries = 3` times with `2s / 4s / 8s` backoff, `206` verifies the whole
/// file on completion, `200` restarts from `0`, and `416` when `offset > 0` verifies on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadResumePlanner {
    bytes_written: u64,
    retries_used: u32,
    max_retries: u32,
}

impl Default for DownloadResumePlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadResumePlanner {
    /// Creates a planner with `max_retries = DOWNLOAD_STALL_RETRIES` (`3`).
    pub fn new() -> Self {
        Self::with_max_retries(DOWNLOAD_STALL_RETRIES)
    }

    /// Creates a planner with a custom maximum stall retry count.
    pub fn with_max_retries(max_retries: u32) -> Self {
        Self {
            bytes_written: 0,
            retries_used: 0,
            max_retries,
        }
    }

    /// Total bytes written to disk by attempts within this download session.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Number of retries already scheduled in this download session.
    pub fn retries_used(&self) -> u32 {
        self.retries_used
    }

    /// Configured maximum number of stall retries (`3` by default).
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// Returns `Some("bytes=<offset>-")` when `bytes_written > 0`, else `None`.
    pub fn range_header(&self) -> Option<String> {
        (self.bytes_written > 0).then(|| format!("bytes={}-", self.bytes_written))
    }

    /// Records `delta` bytes committed to disk by the active download attempt.
    pub fn record_bytes_written(&mut self, delta: u64) {
        self.bytes_written = self.bytes_written.saturating_add(delta);
    }

    /// Validates an HTTP response against the current `bytes_written` offset.
    pub fn plan_response(
        &mut self,
        response: HttpDownloadResponse,
    ) -> Result<ResponseAction, ResumeError> {
        match response {
            HttpDownloadResponse::Ok200 => {
                self.bytes_written = 0;
                Ok(ResponseAction::RestartFromZero)
            }
            HttpDownloadResponse::PartialContent206 { content_range } => {
                let expected_prefix = format!("bytes {}-", self.bytes_written);
                let valid = self.bytes_written > 0
                    && content_range
                        .as_deref()
                        .is_some_and(|cr| cr.starts_with(&expected_prefix));
                if !valid {
                    return Err(ResumeError::InvalidContentRange {
                        offset: self.bytes_written,
                        content_range,
                    });
                }
                Ok(ResponseAction::AppendAndVerifyWholeFile {
                    offset: self.bytes_written,
                })
            }
            HttpDownloadResponse::RangeNotSatisfiable416 => {
                if self.bytes_written > 0 {
                    Ok(ResponseAction::VerifyExistingFile)
                } else {
                    Err(ResumeError::UnexpectedStatus { status: 416 })
                }
            }
            HttpDownloadResponse::OtherError { status } => {
                Err(ResumeError::UnexpectedStatus { status })
            }
        }
    }

    /// Plans retry backoff on failure, aborting non-stall errors or once `retries_used == max_retries`.
    pub fn plan_failure(&mut self, kind: DownloadFailureKind) -> RetryPlan {
        if kind != DownloadFailureKind::StallOrTimeout || self.retries_used >= self.max_retries {
            return RetryPlan::Abort;
        }
        let backoff_ms = RETRY_BASE_BACKOFF_MS.saturating_mul(1_u64 << self.retries_used.min(62));
        self.retries_used += 1;
        RetryPlan::Retry {
            retry_index: self.retries_used,
            backoff_ms,
            range_offset: self.bytes_written,
        }
    }
}
