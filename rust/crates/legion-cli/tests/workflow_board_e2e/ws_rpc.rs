//! Header-capable and `?frameParts=1` stream-safe RFC 6455 WebSocket RPC client.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::net::SocketAddr;

use legion_protocol::rpc_codec::{
    ClientRpcFrame, RpcExit, RpcId, ServerRpcFrame, decode_server_frames, encode_client_frame,
};
use legion_shared::frame_parts::{FrameOutput, FramePartAssembler};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::TestResult;

pub struct OpaqueWsClient {
    stream: TcpStream,
    buf: Vec<u8>,
    assembler: FramePartAssembler,
    pending_chunks: VecDeque<(RpcId, Value)>,
    pending_exits: VecDeque<(RpcId, RpcExit)>,
    pending_defect: Option<String>,
}

impl OpaqueWsClient {
    pub async fn connect(
        addr: SocketAddr,
        path_and_query: &str,
        extra_headers: &[(&str, &str)],
    ) -> TestResult<Self> {
        let mut stream = TcpStream::connect(addr).await?;
        let mut req = format!(
            "GET {path_and_query} HTTP/1.1\r\n\
             Host: {addr}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n"
        );
        for (name, value) in extra_headers {
            let _ = write!(req, "{name}: {value}\r\n");
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).await?;

        let mut raw = Vec::new();
        let mut tmp = [0_u8; 1024];
        let header_end = loop {
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                return Err("EOF waiting for HTTP 101 handshake".into());
            }
            raw.extend_from_slice(&tmp[..n]);
            if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };

        let head = std::str::from_utf8(&raw[..header_end])?;
        if !head.starts_with("HTTP/1.1 101") {
            return Err(format!("expected 101 Switching Protocols, got: {head}").into());
        }
        if !head.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=") {
            return Err(format!("missing Sec-WebSocket-Accept in: {head}").into());
        }

        Ok(Self {
            stream,
            buf: raw[header_end..].to_vec(),
            assembler: FramePartAssembler::new(),
            pending_chunks: VecDeque::new(),
            pending_exits: VecDeque::new(),
            pending_defect: None,
        })
    }

    pub async fn send_frame(&mut self, frame: &ClientRpcFrame) -> TestResult<()> {
        self.send_text(&encode_client_frame(frame)?).await
    }

    fn take_pending_exit(&mut self, target: &RpcId) -> Option<RpcExit> {
        let pos = self
            .pending_exits
            .iter()
            .position(|(rid, _)| rid == target)?;
        self.pending_exits.remove(pos).map(|(_, exit)| exit)
    }

    fn take_pending_chunk(&mut self, target: &RpcId) -> Option<Value> {
        let pos = self
            .pending_chunks
            .iter()
            .position(|(rid, _)| rid == target)?;
        self.pending_chunks.remove(pos).map(|(_, value)| value)
    }

    async fn pump_frames(&mut self) -> TestResult<()> {
        let frames = self.recv_rpc_frames().await?;
        let mut ack_ids = Vec::new();
        for frame in frames {
            match frame {
                ServerRpcFrame::Chunk { request_id, values } => {
                    ack_ids.push(request_id.clone());
                    for value in values {
                        self.pending_chunks.push_back((request_id.clone(), value));
                    }
                }
                ServerRpcFrame::Exit { request_id, exit } => {
                    self.pending_exits.push_back((request_id, exit));
                }
                ServerRpcFrame::Defect { defect } => {
                    if self.pending_defect.is_none() {
                        self.pending_defect = Some(defect.to_string());
                    }
                }
                ServerRpcFrame::Pong => {}
            }
        }
        for request_id in ack_ids {
            self.send_frame(&ClientRpcFrame::Ack { request_id }).await?;
        }
        Ok(())
    }

    pub async fn call_unary(&mut self, id: &str, tag: &str, payload: Value) -> TestResult<RpcExit> {
        let req_id = RpcId::from(id);
        self.send_request(&req_id, tag, payload).await?;
        loop {
            if let Some(defect) = self.pending_defect.take() {
                return Err(format!("WebSocket Defect during {tag}: {defect}").into());
            }
            if let Some(exit) = self.take_pending_exit(&req_id) {
                return Ok(exit);
            }
            self.pump_frames().await?;
        }
    }

    pub async fn call_ok(&mut self, id: &str, tag: &str, payload: Value) -> TestResult<Value> {
        match self.call_unary(id, tag, payload).await? {
            RpcExit::Success { value } => Ok(value),
            RpcExit::Failure { cause } => Err(format!("{tag} failed: {cause:?}").into()),
        }
    }

    pub async fn subscribe(&mut self, id: &str, tag: &str, payload: Value) -> TestResult<()> {
        let req_id = RpcId::from(id);
        self.send_request(&req_id, tag, payload).await
    }

    pub async fn next_chunk_for(&mut self, id: &str) -> TestResult<Value> {
        let target = RpcId::from(id);
        loop {
            if let Some(defect) = self.pending_defect.take() {
                return Err(format!("WebSocket Defect waiting for {id}: {defect}").into());
            }
            if let Some(value) = self.take_pending_chunk(&target) {
                return Ok(value);
            }
            if let Some(exit) = self.take_pending_exit(&target) {
                return Err(format!("stream {id} exited early: {exit:?}").into());
            }
            self.pump_frames().await?;
        }
    }

    pub async fn interrupt(&mut self, id: &str) -> TestResult<()> {
        let frame = ClientRpcFrame::Interrupt {
            request_id: RpcId::from(id),
        };
        self.send_frame(&frame).await
    }

    async fn send_request(&mut self, id: &RpcId, tag: &str, payload: Value) -> TestResult<()> {
        let frame = ClientRpcFrame::Request {
            id: id.clone(),
            tag: tag.to_owned(),
            payload,
            headers: Vec::new(),
            is_notification: false,
            discard: false,
            trace_id: None,
            span_id: None,
            sampled: None,
        };
        self.send_frame(&frame).await
    }

    async fn recv_rpc_frames(&mut self) -> TestResult<Vec<ServerRpcFrame>> {
        loop {
            let text = self.recv_text().await?;
            match self.assembler.push(&text) {
                FrameOutput::IntermediateAck(_) => {
                    // `PART_RECEIVED_FRAME` (`{"_tag":"Pong"}`) is a client-local liveness signal
                    // and must never be echoed back to the server.
                }
                FrameOutput::Complete(whole) => {
                    return Ok(decode_server_frames(whole.as_ref())?);
                }
            }
        }
    }

    async fn send_text(&mut self, text: &str) -> TestResult<()> {
        let payload = text.as_bytes();
        let mask = [0x12_u8, 0x34, 0x56, 0x78];
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(0x81);
        let len = payload.len();
        if len <= 125 {
            frame.push(0x80 | u8::try_from(len)?);
        } else if len <= 65_535 {
            frame.push(0x80 | 0x7e);
            frame.extend_from_slice(&u16::try_from(len)?.to_be_bytes());
        } else {
            frame.push(0x80 | 0x7f);
            frame.extend_from_slice(&u64::try_from(len)?.to_be_bytes());
        }
        frame.extend_from_slice(&mask);
        for (idx, byte) in payload.iter().enumerate() {
            frame.push(byte ^ mask[idx % 4]);
        }
        self.stream.write_all(&frame).await?;
        Ok(())
    }

    async fn recv_text(&mut self) -> TestResult<String> {
        loop {
            let (opcode, payload) = self.read_frame().await?;
            match opcode {
                0x1 => return Ok(String::from_utf8(payload)?),
                0x9 => {
                    let mut pong = vec![0x8A, 0x80, 0, 0, 0, 0];
                    if payload.len() <= 125 {
                        pong[1] = 0x80 | u8::try_from(payload.len())?;
                        pong.extend_from_slice(&payload);
                    }
                    self.stream.write_all(&pong).await?;
                }
                0xA => {}
                0x8 => return Err("unexpected WebSocket Close frame".into()),
                other => return Err(format!("unexpected WebSocket opcode 0x{other:x}").into()),
            }
        }
    }

    async fn read_frame(&mut self) -> TestResult<(u8, Vec<u8>)> {
        let mut assembled = Vec::new();
        let mut first_opcode = None;
        loop {
            self.ensure_bytes(2).await?;
            let (b0, b1) = (self.buf[0], self.buf[1]);
            self.buf.drain(..2);
            let fin = (b0 & 0x80) != 0;
            let opcode = b0 & 0x0f;
            if first_opcode.is_none() {
                first_opcode = Some(opcode);
            }
            let masked = (b1 & 0x80) != 0;
            let payload_len: usize = match b1 & 0x7f {
                len @ 0..=125 => usize::from(len),
                126 => {
                    self.ensure_bytes(2).await?;
                    let n = u16::from_be_bytes([self.buf[0], self.buf[1]]);
                    self.buf.drain(..2);
                    usize::from(n)
                }
                _ => {
                    self.ensure_bytes(8).await?;
                    let mut bytes = [0_u8; 8];
                    bytes.copy_from_slice(&self.buf[..8]);
                    self.buf.drain(..8);
                    usize::try_from(u64::from_be_bytes(bytes))?
                }
            };
            let mask_key = if masked {
                self.ensure_bytes(4).await?;
                let key = [self.buf[0], self.buf[1], self.buf[2], self.buf[3]];
                self.buf.drain(..4);
                Some(key)
            } else {
                None
            };
            self.ensure_bytes(payload_len).await?;
            let mut chunk: Vec<u8> = self.buf.drain(..payload_len).collect();
            if let Some(key) = mask_key {
                for (idx, b) in chunk.iter_mut().enumerate() {
                    *b ^= key[idx % 4];
                }
            }
            assembled.extend_from_slice(&chunk);
            if fin {
                return Ok((first_opcode.unwrap_or(opcode), assembled));
            }
        }
    }

    async fn ensure_bytes(&mut self, needed: usize) -> TestResult<()> {
        let mut tmp = [0_u8; 4096];
        while self.buf.len() < needed {
            let n = self.stream.read(&mut tmp).await?;
            if n == 0 {
                return Err("unexpected EOF reading WebSocket frame".into());
            }
            self.buf.extend_from_slice(&tmp[..n]);
        }
        Ok(())
    }
}
