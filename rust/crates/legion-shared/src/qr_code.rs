//! ISO/IEC 18004 QR code generator (Project Nayuki port), Unicode half-block terminal
//! renderer, and remote pairing URL builders matching `packages/shared/src/{qrCode,remote}.ts`
//! and `apps/server/src/startupAccess.ts`.

use crate::qr_code_matrix::QrMatrix;
use crate::qr_code_segment::append_bits;
pub use crate::qr_code_segment::{QrEcc, QrError, QrSegment, QrSegmentMode};
use crate::qr_code_tables::{get_num_data_codewords, get_num_raw_data_modules};
use crate::url_authority::{build_pairing_url_impl, encode_form_urlencoded};

/// Encoded ISO/IEC 18004 QR Code matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrCode {
    version: u8,
    error_correction_level: QrEcc,
    size: u8,
    mask: u8,
    modules: Vec<bool>,
}

impl QrCode {
    /// Minimum supported QR symbol version (`1`, `21x21`).
    pub const MIN_VERSION: u8 = 1;
    /// Maximum supported QR symbol version (`40`, `177x177`).
    pub const MAX_VERSION: u8 = 40;

    /// Encodes a UTF-8 text string with `ecl` error correction and automatic version/mask/boost.
    ///
    /// # Examples
    ///
    /// ```
    /// use legion_shared::qr_code::{QrCode, QrEcc};
    ///
    /// let qr = QrCode::encode_text("http://127.0.0.1:3773/pair#token=test-token", QrEcc::Medium)
    ///     .expect("URL fits easily within QR version bounds");
    /// assert_eq!(qr.version(), 4);
    /// assert_eq!(qr.size(), 33);
    /// ```
    pub fn encode_text(text: &str, ecl: QrEcc) -> Result<Self, QrError> {
        let segs = QrSegment::make_segments(text);
        Self::encode_segments(&segs, ecl)
    }

    /// Encodes an arbitrary byte slice as a single `Byte` segment.
    pub fn encode_binary(data: &[u8], ecl: QrEcc) -> Result<Self, QrError> {
        let seg = QrSegment::make_bytes(data);
        Self::encode_segments(&[seg], ecl)
    }

    /// Encodes `segs` using `min_version = 1`, `max_version = 40`, auto mask, and `boost_ecl = true`.
    pub fn encode_segments(segs: &[QrSegment], ecl: QrEcc) -> Result<Self, QrError> {
        Self::encode_segments_with_options(
            segs,
            ecl,
            Self::MIN_VERSION,
            Self::MAX_VERSION,
            None,
            true,
        )
    }

    /// Full Project Nayuki `QrCode.encodeSegments` entry point with version, mask, and boost controls.
    pub fn encode_segments_with_options(
        segs: &[QrSegment],
        mut ecl: QrEcc,
        min_version: u8,
        max_version: u8,
        mask: Option<u8>,
        boost_ecl: bool,
    ) -> Result<Self, QrError> {
        if !(Self::MIN_VERSION <= min_version
            && min_version <= max_version
            && max_version <= Self::MAX_VERSION)
        {
            return Err(QrError::InvalidValue("version range out of bounds"));
        }
        if mask.is_some_and(|m| m > 7) {
            return Err(QrError::InvalidValue("mask value out of range"));
        }

        let mut chosen = None;
        for ver in min_version..=max_version {
            let capacity_bits = get_num_data_codewords(ver, ecl) * 8;
            if let Some(used_bits) = QrSegment::get_total_bits(segs, ver)
                && used_bits <= capacity_bits
            {
                chosen = Some((ver, used_bits));
                break;
            }
        }
        let (version, data_used_bits) = chosen.ok_or(QrError::DataTooLong)?;

        if boost_ecl {
            for new_ecl in [QrEcc::Medium, QrEcc::Quartile, QrEcc::High] {
                if data_used_bits <= get_num_data_codewords(version, new_ecl) * 8 {
                    ecl = new_ecl;
                }
            }
        }

        let capacity_bits = get_num_data_codewords(version, ecl) * 8;
        let mut bb = Vec::with_capacity(capacity_bits);
        for seg in segs {
            append_bits(u32::from(seg.mode.mode_bits()), 4, &mut bb);
            let count_u32 = u32::try_from(seg.num_chars)
                .map_err(|_| QrError::InvalidValue("segment length"))?;
            append_bits(
                count_u32,
                usize::from(seg.mode.num_char_count_bits(version)),
                &mut bb,
            );
            bb.extend_from_slice(&seg.bit_data);
        }

        append_bits(0, 4usize.min(capacity_bits - bb.len()), &mut bb);
        append_bits(0, (8 - (bb.len() % 8)) % 8, &mut bb);

        let mut pad_byte = 0xECu32;
        while bb.len() < capacity_bits {
            append_bits(pad_byte, 8, &mut bb);
            pad_byte ^= 0xEC ^ 0x11;
        }

        let (byte_chunks, _) = bb.as_chunks::<8>();
        let data_codewords: Vec<u8> = byte_chunks
            .iter()
            .map(|bits| bits.iter().fold(0u8, |acc, &bit| (acc << 1) | bit))
            .collect();

        Self::from_data_codewords(version, ecl, &data_codewords, mask)
    }

    /// Constructs a QR code directly from packed data codewords of exact length for `(version, ecl)`.
    pub fn from_data_codewords(
        version: u8,
        ecl: QrEcc,
        data_codewords: &[u8],
        mask: Option<u8>,
    ) -> Result<Self, QrError> {
        if !(Self::MIN_VERSION..=Self::MAX_VERSION).contains(&version) {
            return Err(QrError::InvalidValue("version value out of range"));
        }
        if mask.is_some_and(|m| m > 7) {
            return Err(QrError::InvalidValue("mask value out of range"));
        }
        if data_codewords.len() != get_num_data_codewords(version, ecl) {
            return Err(QrError::InvalidValue("data codeword count mismatch"));
        }

        let all_codewords =
            crate::qr_code_tables::add_ecc_and_interleave(version, ecl, data_codewords);
        let expected_raw = get_num_raw_data_modules(version) / 8;
        if all_codewords.len() != expected_raw {
            return Err(QrError::InvalidValue("interleaved codeword count mismatch"));
        }

        let (matrix, chosen_mask) = QrMatrix::build(version, ecl, &all_codewords, mask);
        Ok(Self {
            version,
            error_correction_level: ecl,
            size: matrix.size,
            mask: chosen_mask,
            modules: matrix.modules,
        })
    }

    /// QR symbol version (`1..=40`).
    pub const fn version(&self) -> u8 {
        self.version
    }

    /// Effective error correction level (after any `boost_ecl` upgrade).
    pub const fn error_correction_level(&self) -> QrEcc {
        self.error_correction_level
    }

    /// Module width/height of the square QR symbol (`version * 4 + 17`).
    pub const fn size(&self) -> u8 {
        self.size
    }

    /// Applied data mask pattern (`0..=7`).
    pub const fn mask(&self) -> u8 {
        self.mask
    }

    /// Returns `true` if `(x, y)` is within `[0, size)` and dark, or `false` otherwise.
    pub fn get_module(&self, x: i32, y: i32) -> bool {
        let size_i32 = i32::from(self.size);
        if x < 0 || x >= size_i32 || y < 0 || y >= size_i32 {
            return false;
        }
        if let (Ok(ux), Ok(uy)) = (usize::try_from(x), usize::try_from(y)) {
            return self.modules[uy * usize::from(self.size) + ux];
        }
        false
    }
}

/// Renders `value` as a Unicode half-block terminal QR code (`█`, `▀`, `▄`, `' '`) using
/// [`QrEcc::Medium`], matching `renderTerminalQrCode` in `apps/server/src/startupAccess.ts`.
///
/// # Examples
///
/// ```
/// use legion_shared::qr_code::render_terminal_qr_code;
///
/// let rendered = render_terminal_qr_code("http://127.0.0.1:3773/pair#token=test-token", 2)
///     .expect("valid pairing URL fits cleanly");
/// assert!(rendered.contains('█'));
/// assert_eq!(rendered.lines().count(), 19);
/// ```
pub fn render_terminal_qr_code(value: &str, margin: i32) -> Result<String, QrError> {
    if !(0..=1024).contains(&margin) {
        return Err(QrError::InvalidMargin(margin));
    }
    let qr_code = QrCode::encode_text(value, QrEcc::Medium)?;
    let size = i32::from(qr_code.size());
    let mut rows = Vec::with_capacity(usize::try_from((size + margin * 2 + 1) / 2).unwrap_or(0));

    let mut y = -margin;
    while y < size + margin {
        let mut row = String::with_capacity(usize::try_from(size + margin * 2).unwrap_or(0));
        for x in -margin..(size + margin) {
            let cell = match (qr_code.get_module(x, y), qr_code.get_module(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            };
            row.push(cell);
        }
        rows.push(row);
        y += 2;
    }

    Ok(rows.join("\n"))
}

/// Normalizes `connection_string` to `/pair`, removes any `token` query parameter,
/// and puts `#token=<token>` in the URL fragment, matching `buildPairingUrl` in `startupAccess.ts`.
///
/// # Examples
///
/// ```
/// use legion_shared::qr_code::build_pairing_url;
///
/// let url = build_pairing_url("http://127.0.0.1:3773", "test-token").expect("valid URL");
/// assert_eq!(url, "http://127.0.0.1:3773/pair#token=test-token");
/// ```
pub fn build_pairing_url(connection_string: &str, token: &str) -> Result<String, QrError> {
    build_pairing_url_impl(connection_string, token).ok_or(QrError::InvalidUrl)
}

/// Wraps `pairing_url` in a `legioncode://pair?pairingUrl=...` deep link matching `buildPairingAppLink`
/// in `packages/shared/src/remote.ts`.
///
/// # Examples
///
/// ```
/// use legion_shared::qr_code::build_pairing_app_link;
///
/// assert_eq!(
///     build_pairing_app_link("http://127.0.0.1:3773/pair#token=test-token"),
///     "legioncode://pair?pairingUrl=http%3A%2F%2F127.0.0.1%3A3773%2Fpair%23token%3Dtest-token",
/// );
/// ```
pub fn build_pairing_app_link(pairing_url: &str) -> String {
    format!(
        "legioncode://pair?pairingUrl={}",
        encode_form_urlencoded(pairing_url)
    )
}
