#![forbid(unsafe_code)]
//! Shared runtime utilities for `LegionCode`: UTF-16 offsets and folding, WebSocket large-frame
//! splitting, ES256 `DPoP` proofs, async keyed locking and drainable workers, and `SoA` fuzzy
//! palette search.
//!
//! # Examples
//!
//! ```
//! use legion_shared::frame_parts::{FrameOutput, FramePartAssembler, split_socket_frame_with_limit};
//! use legion_shared::search::FuzzyPaletteIndex;
//! use legion_shared::utf16::utf16_slice_exact;
//!
//! let frame = "Hello 🚀 world!".repeat(4);
//! let parts = split_socket_frame_with_limit(&frame, 20).expect("limit >= 5");
//! let mut assembler = FramePartAssembler::new();
//! let mut reassembled = None;
//! for part in &parts {
//!     if let FrameOutput::Complete(whole) = assembler.push(part) {
//!         reassembled = Some(whole.into_owned());
//!     }
//! }
//! assert_eq!(reassembled.as_deref(), Some(frame.as_str()));
//!
//! let titles = ["Share auth across worktrees", "Fix oauth redirect"];
//! let index = FuzzyPaletteIndex::new(titles);
//! let hits = index.search("auth");
//! assert_eq!(hits[0].item_index, 0);
//! assert_eq!(
//!     utf16_slice_exact(titles[0], hits[0].ranges[0].start, hits[0].ranges[0].end),
//!     Ok("auth")
//! );
//! ```

#[cfg(feature = "concurrency")]
mod concurrency_lock;
#[cfg(feature = "concurrency")]
mod concurrency_worker;
#[cfg(feature = "dpop")]
mod dpop_htu;
#[cfg(feature = "dpop")]
mod dpop_verify;
mod qr_code_matrix;
mod qr_code_segment;
mod qr_code_tables;
mod search_build;
mod search_eval;
mod search_match;
mod search_query;
mod search_typo;
mod text_norm_tables;
mod unicode_so_table;
mod utf16_fold_table;

#[cfg(feature = "concurrency")]
pub mod concurrency;
pub mod desktop_app_control;
#[cfg(feature = "dpop")]
pub mod dpop;
pub mod frame_parts;
pub mod keybinding_matching;
pub mod markdown_chunks;
pub mod markdown_task_edits;
pub mod oauth_callback;
pub mod qr_code;
pub mod search;
pub mod text_norm;
pub mod url_authority;
pub mod utf16;
