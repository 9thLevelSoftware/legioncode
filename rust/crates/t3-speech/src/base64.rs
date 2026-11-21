//! Pure Safe `RFC 4648` Base64 encoding and decoding for speech worker `NDJSON` frames.

use thiserror::Error;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Errors returned when decoding Base64 `PCM` payloads.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Base64Error {
    /// Input length modulo 4 was 1 or padding appeared in an invalid position.
    #[error("invalid base64 length or padding")]
    InvalidLength,
    /// Input contained a non-Base64 byte.
    #[error("invalid base64 byte {byte:#04x} at index {index}")]
    InvalidByte {
        /// Zero-based byte offset in the input string.
        index: usize,
        /// Invalid ASCII byte encountered.
        byte: u8,
    },
}

/// Encodes `input` bytes into standard padded Base64.
pub fn encode_base64(input: &[u8]) -> String {
    if input.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = usize::from(chunk[0]);
        let b1 = usize::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = usize::from(chunk.get(2).copied().unwrap_or(0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(ALPHABET[(triple >> 18) & 0x3f]));
        out.push(char::from(ALPHABET[(triple >> 12) & 0x3f]));
        if chunk.len() > 1 {
            out.push(char::from(ALPHABET[(triple >> 6) & 0x3f]));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(ALPHABET[triple & 0x3f]));
        } else {
            out.push('=');
        }
    }
    out
}

/// Decodes standard padded Base64 into raw bytes.
pub fn decode_base64(input: &str) -> Result<Vec<u8>, Base64Error> {
    let bytes = input.as_bytes();
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.len().is_multiple_of(4) {
        return Err(Base64Error::InvalidLength);
    }

    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    let (chunks, _) = bytes.as_chunks::<4>();
    for (chunk_idx, chunk) in chunks.iter().enumerate() {
        let base = chunk_idx * 4;
        let is_last = base + 4 == bytes.len();
        let v0 = decode_char(chunk[0], base)?;
        let v1 = decode_char(chunk[1], base + 1)?;
        let pad2 = chunk[2] == b'=';
        let pad3 = chunk[3] == b'=';

        if (pad2 && !pad3) || (pad3 && !is_last) {
            return Err(Base64Error::InvalidLength);
        }

        let v2 = if pad2 {
            0
        } else {
            decode_char(chunk[2], base + 2)?
        };
        let v3 = if pad3 {
            0
        } else {
            decode_char(chunk[3], base + 3)?
        };

        let triple =
            (u32::from(v0) << 18) | (u32::from(v1) << 12) | (u32::from(v2) << 6) | u32::from(v3);

        let decoded_bytes = triple.to_be_bytes();

        out.push(decoded_bytes[1]);
        if !pad2 {
            out.push(decoded_bytes[2]);
        }
        if !pad3 {
            out.push(decoded_bytes[3]);
        }
    }
    Ok(out)
}

fn decode_char(byte: u8, index: usize) -> Result<u8, Base64Error> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(Base64Error::InvalidByte { index, byte }),
    }
}
