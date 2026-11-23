//! `?frameParts=1` UTF-16 `\u001e` transport frame splitting and reassembly helpers.

use legion_protocol::rpc_codec::{ServerRpcFrame, encode_server_frame};
use legion_shared::frame_parts::{FrameOutput, FramePartAssembler, split_socket_frame};

/// Result of feeding one incoming WebSocket text frame into the assembler.
pub(super) enum InboundAssembly {
    /// Intermediate frame part acknowledged; send `ack` and wait for more parts.
    IntermediateAck(String),
    /// Full RPC JSON frame assembled and ready for decoding.
    Complete(String),
}

/// Processes a single incoming WebSocket text message with optional `?frameParts=1` assembly.
pub(super) fn assemble_inbound_text(
    assembler: &mut FramePartAssembler,
    text: &str,
    frame_parts: bool,
) -> InboundAssembly {
    if !frame_parts {
        return InboundAssembly::Complete(text.to_owned());
    }
    match assembler.push(text) {
        FrameOutput::IntermediateAck(ack) => InboundAssembly::IntermediateAck(ack.to_owned()),
        FrameOutput::Complete(whole) => InboundAssembly::Complete(whole.into_owned()),
    }
}

/// Serializes `frame` to Effect RPC JSON and optionally splits oversized frames.
pub(super) fn encode_server_texts(
    frame: &ServerRpcFrame,
    frame_parts: bool,
) -> Result<Vec<String>, ()> {
    let encoded = encode_server_frame(frame).map_err(|_| ())?;
    if frame_parts {
        Ok(split_socket_frame(&encoded))
    } else {
        Ok(vec![encoded])
    }
}
