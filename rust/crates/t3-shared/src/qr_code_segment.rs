//! QR error types, error-correction levels, and segment encoders (`QrSegment`).

const ALPHANUMERIC_CHARSET: &[u8; 45] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

/// Errors returned by QR code encoding, terminal rendering, or pairing URL construction.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QrError {
    /// Provided version, mask, or segment input violated QR specification bounds.
    #[error("invalid QR code parameter: {0}")]
    InvalidValue(&'static str),
    /// Encoded segment stream exceeds the capacity of the maximum QR version.
    #[error("input exceeds maximum QR code capacity")]
    DataTooLong,
    /// Quiet-zone margin was negative or absurdly large.
    #[error("invalid QR quiet-zone margin: {0}")]
    InvalidMargin(i32),
    /// Pairing connection string could not be parsed as an absolute URL with a valid authority.
    #[error("invalid pairing connection URL")]
    InvalidUrl,
}

/// ISO/IEC 18004 error correction level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QrEcc {
    /// ~7% codeword error recovery capacity.
    Low,
    /// ~15% codeword error recovery capacity.
    Medium,
    /// ~25% codeword error recovery capacity.
    Quartile,
    /// ~30% codeword error recovery capacity.
    High,
}

impl QrEcc {
    /// Zero-based index `0..=3` matching Project Nayuki's `Ecc.ordinal`.
    pub const fn ordinal(self) -> usize {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::Quartile => 2,
            Self::High => 3,
        }
    }

    /// 2-bit format indicator stored in QR format information modules.
    pub const fn format_bits(self) -> u8 {
        match self {
            Self::Low => 1,
            Self::Medium => 0,
            Self::Quartile => 3,
            Self::High => 2,
        }
    }
}

/// QR segment encoding mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QrSegmentMode {
    /// Decimal digits (`0`-`9`).
    Numeric,
    /// Uppercase ASCII, digits, space, and `$%*+-./:`.
    Alphanumeric,
    /// Raw 8-bit bytes / UTF-8 octets.
    Byte,
    /// Kanji Shift JIS mode indicator.
    Kanji,
    /// Extended Channel Interpretation indicator.
    Eci,
}

impl QrSegmentMode {
    /// 4-bit mode indicator stored before each segment's character count.
    pub const fn mode_bits(self) -> u8 {
        match self {
            Self::Numeric => 0x1,
            Self::Alphanumeric => 0x2,
            Self::Byte => 0x4,
            Self::Kanji => 0x8,
            Self::Eci => 0x7,
        }
    }

    /// Bit width of the character-count header field for QR `version` (`1..=40`).
    pub const fn num_char_count_bits(self, version: u8) -> u8 {
        let bucket = match version {
            1..=9 => 0,
            10..=26 => 1,
            _ => 2,
        };
        match (self, bucket) {
            (Self::Eci, _) => 0,
            (Self::Byte | Self::Kanji, 0) => 8,
            (Self::Alphanumeric, 0) => 9,
            (Self::Numeric, 0) | (Self::Kanji, 1) => 10,
            (Self::Alphanumeric, 1) => 11,
            (Self::Numeric, 1) | (Self::Kanji, _) => 12,
            (Self::Alphanumeric, _) => 13,
            (Self::Numeric, _) => 14,
            (Self::Byte, _) => 16,
        }
    }
}

/// One typed data segment inside a QR symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrSegment {
    pub(crate) mode: QrSegmentMode,
    pub(crate) num_chars: usize,
    pub(crate) bit_data: Vec<u8>,
}

impl QrSegment {
    /// Returns the encoding mode of this segment.
    pub const fn mode(&self) -> QrSegmentMode {
        self.mode
    }

    /// Returns the character (or byte) count recorded in this segment's header.
    pub const fn num_chars(&self) -> usize {
        self.num_chars
    }

    /// Returns the number of payload bits in this segment (excluding 4-bit mode & count header).
    pub const fn bit_len(&self) -> usize {
        self.bit_data.len()
    }

    /// Creates a `Byte` segment from raw octets.
    pub fn make_bytes(data: &[u8]) -> Self {
        let mut bit_data = Vec::with_capacity(data.len() * 8);
        for &byte in data {
            append_bits(u32::from(byte), 8, &mut bit_data);
        }
        Self {
            mode: QrSegmentMode::Byte,
            num_chars: data.len(),
            bit_data,
        }
    }

    /// Creates a `Numeric` segment from ASCII decimal digits (`0`-`9`).
    pub fn make_numeric(digits: &str) -> Result<Self, QrError> {
        if !Self::is_numeric(digits) {
            return Err(QrError::InvalidValue(
                "string contains non-numeric characters",
            ));
        }
        let bytes = digits.as_bytes();
        let mut bit_data = Vec::with_capacity((bytes.len() * 10).div_ceil(3));
        for chunk in bytes.chunks(3) {
            let val = chunk
                .iter()
                .fold(0u32, |acc, &b| acc * 10 + u32::from(b - b'0'));
            append_bits(val, chunk.len() * 3 + 1, &mut bit_data);
        }
        Ok(Self {
            mode: QrSegmentMode::Numeric,
            num_chars: bytes.len(),
            bit_data,
        })
    }

    /// Creates an `Alphanumeric` segment from characters in `0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:`.
    pub fn make_alphanumeric(text: &str) -> Result<Self, QrError> {
        let mut bit_data = Vec::with_capacity((text.len() * 11).div_ceil(2));
        for chunk in text.as_bytes().chunks(2) {
            let first = alphanumeric_index(chunk[0]).ok_or(QrError::InvalidValue(
                "string contains unencodable characters in alphanumeric mode",
            ))?;
            if let Some(&second_byte) = chunk.get(1) {
                let second = alphanumeric_index(second_byte).ok_or(QrError::InvalidValue(
                    "string contains unencodable characters in alphanumeric mode",
                ))?;
                append_bits(first * 45 + second, 11, &mut bit_data);
            } else {
                append_bits(first, 6, &mut bit_data);
            }
        }
        Ok(Self {
            mode: QrSegmentMode::Alphanumeric,
            num_chars: text.len(),
            bit_data,
        })
    }

    /// Creates an `ECI` designator segment for `assign_val` (`0..1_000_000`).
    pub fn make_eci(assign_val: u32) -> Result<Self, QrError> {
        let mut bit_data = Vec::with_capacity(24);
        if assign_val < (1 << 7) {
            append_bits(assign_val, 8, &mut bit_data);
        } else if assign_val < (1 << 14) {
            append_bits(0b10, 2, &mut bit_data);
            append_bits(assign_val, 14, &mut bit_data);
        } else if assign_val < 1_000_000 {
            append_bits(0b110, 3, &mut bit_data);
            append_bits(assign_val, 21, &mut bit_data);
        } else {
            return Err(QrError::InvalidValue("ECI assignment value out of range"));
        }
        Ok(Self {
            mode: QrSegmentMode::Eci,
            num_chars: 0,
            bit_data,
        })
    }

    /// Chooses the single segment encoding matching `QrSegment.makeSegments` in `qrCode.ts`.
    pub fn make_segments(text: &str) -> Vec<Self> {
        if text.is_empty() {
            Vec::new()
        } else if let Ok(seg) = Self::make_numeric(text) {
            vec![seg]
        } else if let Ok(seg) = Self::make_alphanumeric(text) {
            vec![seg]
        } else {
            vec![Self::make_bytes(text.as_bytes())]
        }
    }

    /// Returns `true` when every byte of `text` is an ASCII decimal digit.
    pub fn is_numeric(text: &str) -> bool {
        text.bytes().all(|b| b.is_ascii_digit())
    }

    /// Returns `true` when every byte of `text` belongs to the QR alphanumeric charset.
    pub fn is_alphanumeric(text: &str) -> bool {
        text.bytes().all(|b| alphanumeric_index(b).is_some())
    }

    /// Total number of encoded bits required for `segs` at `version`, or `None` on header overflow.
    pub fn get_total_bits(segs: &[Self], version: u8) -> Option<usize> {
        let mut result = 0usize;
        for seg in segs {
            let ccbits = seg.mode.num_char_count_bits(version);
            if seg.num_chars >= (1usize << ccbits) {
                return None;
            }
            result = result
                .checked_add(4 + usize::from(ccbits))?
                .checked_add(seg.bit_data.len())?;
        }
        Some(result)
    }
}

fn alphanumeric_index(byte: u8) -> Option<u32> {
    ALPHANUMERIC_CHARSET
        .iter()
        .position(|&candidate| candidate == byte)
        .and_then(|pos| u32::try_from(pos).ok())
}

pub(crate) fn append_bits(val: u32, len: usize, bb: &mut Vec<u8>) {
    for shift in (0..len).rev() {
        bb.push(u8::try_from((val >> shift) & 1).unwrap_or(0));
    }
}
