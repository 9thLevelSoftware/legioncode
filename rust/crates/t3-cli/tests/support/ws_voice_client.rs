//! Minimal RFC 6455 WebSocket client supporting text, binary, and close frames for `/ws/voice` tests.

use std::error::Error;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub enum WsIncomingFrame {
    Text(String),
    Close { code: Option<u16>, reason: String },
}

pub struct WsVoiceTestClient {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl WsVoiceTestClient {
    pub async fn connect(
        addr: std::net::SocketAddr,
        path_and_query: &str,
    ) -> Result<Self, Box<dyn Error>> {
        let mut stream = TcpStream::connect(addr).await?;
        let req = format!(
            "GET {path_and_query} HTTP/1.1\r\n\
             Host: {addr}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n"
        );
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
        if !head.starts_with("HTTP/1.1 101") || !head.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=") {
            return Err(format!("invalid WebSocket upgrade response: {head}").into());
        }
        Ok(Self {
            stream,
            buf: raw[header_end..].to_vec(),
        })
    }

    pub async fn send_text(&mut self, text: &str) -> Result<(), Box<dyn Error>> {
        self.send_masked(0x81, text.as_bytes()).await
    }

    pub async fn send_binary(&mut self, payload: &[u8]) -> Result<(), Box<dyn Error>> {
        self.send_masked(0x82, payload).await
    }

    pub async fn recv_frame(&mut self) -> Result<WsIncomingFrame, Box<dyn Error>> {
        loop {
            let (opcode, payload) = self.read_raw_frame().await?;
            match opcode {
                0x1 => return Ok(WsIncomingFrame::Text(String::from_utf8(payload)?)),
                0x8 => {
                    let code = if payload.len() >= 2 {
                        Some(u16::from_be_bytes([payload[0], payload[1]]))
                    } else {
                        None
                    };
                    let reason = if payload.len() > 2 {
                        String::from_utf8_lossy(&payload[2..]).into_owned()
                    } else {
                        String::new()
                    };
                    return Ok(WsIncomingFrame::Close { code, reason });
                }
                0x9 => {
                    self.send_masked(0x8A, &payload).await?;
                }
                0xA => {}
                other => return Err(format!("unexpected WebSocket opcode 0x{other:x}").into()),
            }
        }
    }

    async fn send_masked(&mut self, b0: u8, payload: &[u8]) -> Result<(), Box<dyn Error>> {
        let mask = [0x12_u8, 0x34, 0x56, 0x78];
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(b0);
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

    async fn read_raw_frame(&mut self) -> Result<(u8, Vec<u8>), Box<dyn Error>> {
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
            let payload_len: usize = match b1 & 0x7f {
                len7 @ 0..=125 => usize::from(len7),
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
            let mask_key = if (b1 & 0x80) != 0 {
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

    async fn ensure_bytes(&mut self, needed: usize) -> Result<(), Box<dyn Error>> {
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
