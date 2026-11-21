//! Bounded backpressure windows for streaming Effect RPC responses.
//!
//! Tracks in-flight chunks per `(client_id, request_id)` so fast producers can
//! pipeline up to [`MAX_PENDING_CHUNKS`] or [`MAX_PENDING_BYTES`] before
//! waiting on client `Ack` frames.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;

use super::RpcId;

/// Maximum number of unacknowledged chunks allowed in flight per stream.
pub const MAX_PENDING_CHUNKS: usize = 8;

/// Maximum aggregate estimated payload bytes allowed in flight per byte-windowed stream.
pub const MAX_PENDING_BYTES: usize = 64 * 1024;

/// Fixed per-event and per-chunk framing overhead used in window byte accounting.
pub const EVENT_OVERHEAD: usize = 128;

/// Windowing strategy selected for an RPC method tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamWindowMode {
    /// Enforce both [`MAX_PENDING_CHUNKS`] and [`MAX_PENDING_BYTES`].
    ByteAndChunk,
    /// Enforce [`MAX_PENDING_CHUNKS`] only (treats each chunk as zero bytes).
    ChunkCountOnly,
    /// Stream has no output window applied.
    Unwindowed,
}

/// Disposition returned after recording an outbound `Chunk` frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkDisposition {
    /// Window remains below capacity; emit a synthetic `Ack` to keep streaming.
    EmitSyntheticAck,
    /// Window reached chunk or byte capacity; wait for client `Ack` before pulling more.
    AwaitClientAck,
}

/// Disposition returned when an inbound `Ack` arrives for a windowed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckDisposition {
    /// Window transitioned from full to below capacity; forward `Ack` to producer.
    ForwardAck,
    /// Producer already received a synthetic `Ack`, or request is still saturated; drop `Ack`.
    Swallow,
}

/// Disposition from [`OutputWindowRegistry::on_send_chunk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryChunkAction {
    /// Request has no active window; do not track or synthesize an `Ack`.
    NotWindowed,
    /// Active window disposition for the outbound chunk.
    Windowed(ChunkDisposition),
}

/// Classifies an RPC method tag into its stream backpressure mode.
pub fn stream_window_mode(tag: &str) -> StreamWindowMode {
    match tag {
        "terminal.attach" | "terminal.observe" => StreamWindowMode::ByteAndChunk,
        "subscribeServerConfig"
        | "orchestration.subscribeShell"
        | "orchestration.subscribeThread" => StreamWindowMode::ChunkCountOnly,
        _ => StreamWindowMode::Unwindowed,
    }
}

fn str_field_len(obj: &serde_json::Map<String, Value>, key: &str) -> usize {
    obj.get(key).and_then(Value::as_str).map_or(0, str::len)
}

fn json_payload_len(value: Option<&Value>) -> usize {
    match value {
        None | Some(Value::Null) => 0,
        Some(Value::String(text)) => text.len(),
        Some(other) => serde_json::to_string(other).map_or(0, |s| s.len()),
    }
}

/// Estimates buffered wire size for a single streamed terminal/orchestration event.
///
/// Includes [`EVENT_OVERHEAD`], `threadId`, `terminalId`, type-specific payloads
/// (`output.data`, `error.message`, `error.error`, `activity.label`), and any
/// embedded `snapshot` fields including `snapshot.error`.
pub fn queued_event_bytes(value: &Value) -> usize {
    let Some(obj) = value.as_object() else {
        return EVENT_OVERHEAD + json_payload_len(Some(value));
    };

    let mut total = EVENT_OVERHEAD
        .saturating_add(str_field_len(obj, "threadId"))
        .saturating_add(str_field_len(obj, "terminalId"));

    match obj.get("type").and_then(Value::as_str) {
        Some("output") => {
            total = total.saturating_add(str_field_len(obj, "data"));
        }
        Some("error") => {
            total = total
                .saturating_add(str_field_len(obj, "message"))
                .saturating_add(json_payload_len(obj.get("error")));
        }
        Some("activity") => {
            total = total.saturating_add(str_field_len(obj, "label"));
        }
        _ => {}
    }

    if let Some(snapshot) = obj.get("snapshot").and_then(Value::as_object) {
        total = total
            .saturating_add(str_field_len(snapshot, "threadId"))
            .saturating_add(str_field_len(snapshot, "terminalId"))
            .saturating_add(str_field_len(snapshot, "history"))
            .saturating_add(str_field_len(snapshot, "cwd"))
            .saturating_add(str_field_len(snapshot, "worktreePath"))
            .saturating_add(str_field_len(snapshot, "label"))
            .saturating_add(json_payload_len(snapshot.get("error")));
    }

    total
}

/// Computes total estimated bytes for an RPC `Chunk` values array.
pub fn chunk_bytes(values: &[Value]) -> usize {
    let event_sum = values.iter().fold(0usize, |acc, item| {
        acc.saturating_add(queued_event_bytes(item))
    });
    EVENT_OVERHEAD.saturating_add(event_sum)
}

/// Per-stream pending chunk and byte window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputWindow {
    mode: StreamWindowMode,
    pending_chunks: VecDeque<usize>,
    pending_bytes: usize,
}

impl OutputWindow {
    /// Creates an empty window for the given [`StreamWindowMode`].
    pub fn new(mode: StreamWindowMode) -> Self {
        Self {
            mode,
            pending_chunks: VecDeque::with_capacity(MAX_PENDING_CHUNKS),
            pending_bytes: 0,
        }
    }

    /// Returns the active [`StreamWindowMode`].
    pub fn mode(&self) -> StreamWindowMode {
        self.mode
    }

    /// Returns the number of unacknowledged chunks currently tracked.
    pub fn pending_chunk_count(&self) -> usize {
        self.pending_chunks.len()
    }

    /// Returns the aggregate tracked byte count of unacknowledged chunks.
    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes
    }

    /// Returns `true` when chunk count or byte limits have been reached.
    pub fn is_full(&self) -> bool {
        self.pending_chunks.len() >= MAX_PENDING_CHUNKS || self.pending_bytes >= MAX_PENDING_BYTES
    }

    /// Records an outbound chunk's values and returns whether to synthesize an `Ack`.
    pub fn on_send_chunk(&mut self, values: &[Value]) -> ChunkDisposition {
        let tracked_bytes = match self.mode {
            StreamWindowMode::ByteAndChunk => chunk_bytes(values),
            StreamWindowMode::ChunkCountOnly | StreamWindowMode::Unwindowed => 0,
        };
        self.record_chunk_bytes(tracked_bytes)
    }

    /// Records a precomputed chunk byte size and returns whether to synthesize an `Ack`.
    pub fn record_chunk_bytes(&mut self, bytes: usize) -> ChunkDisposition {
        let tracked = match self.mode {
            StreamWindowMode::ByteAndChunk => bytes,
            StreamWindowMode::ChunkCountOnly | StreamWindowMode::Unwindowed => 0,
        };
        self.pending_chunks.push_back(tracked);
        self.pending_bytes = self.pending_bytes.saturating_add(tracked);
        if self.is_full() {
            ChunkDisposition::AwaitClientAck
        } else {
            ChunkDisposition::EmitSyntheticAck
        }
    }

    /// Records an inbound client `Ack` and reports whether to forward it upstream.
    pub fn on_client_ack(&mut self) -> AckDisposition {
        let was_full = self.is_full();
        if let Some(chunk_len) = self.pending_chunks.pop_front() {
            self.pending_bytes = self.pending_bytes.saturating_sub(chunk_len);
        }
        let now_full = self.is_full();
        if was_full && !now_full {
            AckDisposition::ForwardAck
        } else {
            AckDisposition::Swallow
        }
    }
}

/// Composite key identifying an active RPC stream across clients.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamWindowKey {
    /// Connected socket client identifier.
    pub client_id: u64,
    /// Wire RPC request identifier.
    pub request_id: RpcId,
}

impl StreamWindowKey {
    /// Builds a [`StreamWindowKey`] for `client_id` and `request_id`.
    pub fn new(client_id: u64, request_id: RpcId) -> Self {
        Self {
            client_id,
            request_id,
        }
    }
}

/// Registry tracking per-request [`OutputWindow`] instances for active clients.
#[derive(Debug, Default)]
pub struct OutputWindowRegistry {
    windows: HashMap<StreamWindowKey, OutputWindow>,
}

impl OutputWindowRegistry {
    /// Creates an empty [`OutputWindowRegistry`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a window when `tag` is windowed; prunes windows whose clients disconnected.
    pub fn open_for_request(
        &mut self,
        client_id: u64,
        request_id: RpcId,
        tag: &str,
        live_clients: &HashSet<u64>,
    ) -> StreamWindowMode {
        self.prune_disconnected(live_clients);
        let mode = stream_window_mode(tag);
        if mode != StreamWindowMode::Unwindowed {
            self.windows.insert(
                StreamWindowKey::new(client_id, request_id),
                OutputWindow::new(mode),
            );
        }
        mode
    }

    /// Tracks an outbound `Chunk` for `(client_id, request_id)` if windowed.
    pub fn on_send_chunk(
        &mut self,
        client_id: u64,
        request_id: &RpcId,
        values: &[Value],
    ) -> RegistryChunkAction {
        let key = StreamWindowKey::new(client_id, request_id.clone());
        match self.windows.get_mut(&key) {
            Some(window) => RegistryChunkAction::Windowed(window.on_send_chunk(values)),
            None => RegistryChunkAction::NotWindowed,
        }
    }

    /// Handles an inbound client `Ack` for `(client_id, request_id)`.
    pub fn on_client_ack(&mut self, client_id: u64, request_id: &RpcId) -> AckDisposition {
        let key = StreamWindowKey::new(client_id, request_id.clone());
        match self.windows.get_mut(&key) {
            Some(window) => window.on_client_ack(),
            None => AckDisposition::ForwardAck,
        }
    }

    /// Removes the window associated with `(client_id, request_id)` when `Exit` is sent.
    pub fn on_send_exit(&mut self, client_id: u64, request_id: &RpcId) -> Option<OutputWindow> {
        let key = StreamWindowKey::new(client_id, request_id.clone());
        self.windows.remove(&key)
    }

    /// Drops all windows owned by a disconnected `client_id`.
    pub fn close_client(&mut self, client_id: u64) {
        self.windows.retain(|key, _| key.client_id != client_id);
    }

    /// Removes windows whose `client_id` is no longer present in `live_clients`.
    pub fn prune_disconnected(&mut self, live_clients: &HashSet<u64>) {
        self.windows
            .retain(|key, _| live_clients.contains(&key.client_id));
    }

    /// Returns the number of active windowed streams currently tracked.
    pub fn active_count(&self) -> usize {
        self.windows.len()
    }

    /// Inspects a window by `(client_id, request_id)`.
    pub fn get(&self, client_id: u64, request_id: &RpcId) -> Option<&OutputWindow> {
        let key = StreamWindowKey::new(client_id, request_id.clone());
        self.windows.get(&key)
    }
}
