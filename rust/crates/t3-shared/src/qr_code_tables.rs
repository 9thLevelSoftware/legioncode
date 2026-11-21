//! ISO/IEC 18004 QR code capacity tables, alignment geometry, and GF(256) Reed-Solomon math.

use crate::qr_code::QrEcc;

/// Number of error correction codewords per block, indexed by `[ecc.ordinal()][version - 1]`.
pub(crate) const ECC_CODEWORDS_PER_BLOCK: [[u8; 40]; 4] = [
    // Low
    [
        7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28, 30,
        30, 26, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
    // Medium
    [
        10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28,
        28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
    ],
    // Quartile
    [
        13, 22, 18, 26, 18, 24, 18, 22, 20, 24, 28, 26, 24, 20, 30, 24, 28, 28, 26, 30, 28, 30, 30,
        30, 30, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
    // High
    [
        17, 28, 22, 16, 22, 28, 26, 26, 24, 28, 24, 28, 22, 24, 24, 30, 28, 28, 26, 28, 30, 24, 30,
        30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
];

/// Number of Reed-Solomon error correction blocks, indexed by `[ecc.ordinal()][version - 1]`.
pub(crate) const NUM_ERROR_CORRECTION_BLOCKS: [[u8; 40]; 4] = [
    // Low
    [
        1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13,
        14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 24, 25,
    ],
    // Medium
    [
        1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23,
        25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
    ],
    // Quartile
    [
        1, 1, 2, 2, 4, 4, 6, 6, 8, 8, 8, 10, 12, 16, 12, 17, 16, 18, 21, 20, 23, 23, 25, 27, 29,
        34, 34, 35, 38, 40, 43, 45, 48, 51, 53, 56, 59, 62, 65, 68,
    ],
    // High
    [
        1, 1, 2, 4, 4, 4, 5, 6, 8, 8, 11, 11, 16, 16, 18, 16, 19, 21, 25, 25, 25, 34, 30, 32, 35,
        37, 40, 42, 45, 48, 51, 54, 57, 60, 63, 66, 70, 74, 77, 81,
    ],
];

/// Returns the number of data + ECC + remainder modules available in version `ver` (`1..=40`).
pub(crate) fn get_num_raw_data_modules(ver: u8) -> usize {
    let v = usize::from(ver);
    let mut result = (16 * v + 128) * v + 64;
    if ver >= 2 {
        let num_align = v / 7 + 2;
        result -= (25 * num_align - 10) * num_align - 55;
        if ver >= 7 {
            result -= 36;
        }
    }
    result
}

/// Returns the number of 8-bit data codewords storable in `(ver, ecl)`.
pub(crate) fn get_num_data_codewords(ver: u8, ecl: QrEcc) -> usize {
    let v_idx = usize::from(ver - 1);
    let ord = ecl.ordinal();
    let ecc_per_block = usize::from(ECC_CODEWORDS_PER_BLOCK[ord][v_idx]);
    let num_blocks = usize::from(NUM_ERROR_CORRECTION_BLOCKS[ord][v_idx]);
    get_num_raw_data_modules(ver) / 8 - ecc_per_block * num_blocks
}

/// Returns ascending alignment pattern center coordinates along both axes for version `ver`.
pub(crate) fn get_alignment_pattern_positions(ver: u8) -> Vec<u8> {
    if ver == 1 {
        return Vec::new();
    }
    let v = usize::from(ver);
    let size = v * 4 + 17;
    let num_align = v / 7 + 2;
    let step = if ver == 32 {
        26
    } else {
        (v * 4 + 4).div_ceil(num_align * 2 - 2) * 2
    };
    let mut result = Vec::with_capacity(num_align);
    result.push(6u8);
    let mut pos = size - 7;
    while result.len() < num_align {
        if let Ok(coord) = u8::try_from(pos) {
            result.insert(1, coord);
        }
        let Some(next) = pos.checked_sub(step) else {
            break;
        };
        pos = next;
    }
    result
}

/// Splits `data` into Reed-Solomon blocks, computes ECC codewords, and interleaves all codewords.
pub(crate) fn add_ecc_and_interleave(ver: u8, ecl: QrEcc, data: &[u8]) -> Vec<u8> {
    let v_idx = usize::from(ver - 1);
    let ord = ecl.ordinal();
    let num_blocks = usize::from(NUM_ERROR_CORRECTION_BLOCKS[ord][v_idx]);
    let block_ecc_len = usize::from(ECC_CODEWORDS_PER_BLOCK[ord][v_idx]);
    let raw_codewords = get_num_raw_data_modules(ver) / 8;
    let num_short_blocks = num_blocks - (raw_codewords % num_blocks);
    let short_block_len = raw_codewords / num_blocks;

    let rs_div = reed_solomon_compute_divisor(block_ecc_len);
    let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(num_blocks);
    let mut k = 0usize;
    for i in 0..num_blocks {
        let data_len = short_block_len - block_ecc_len + usize::from(i >= num_short_blocks);
        let slice = &data[k..k + data_len];
        k += data_len;
        let ecc = reed_solomon_compute_remainder(slice, &rs_div);
        let mut block = Vec::with_capacity(short_block_len + 1 + block_ecc_len);
        block.extend_from_slice(slice);
        if i < num_short_blocks {
            block.push(0);
        }
        block.extend_from_slice(&ecc);
        blocks.push(block);
    }

    let mut result = Vec::with_capacity(raw_codewords);
    let max_block_len = blocks.first().map_or(0, Vec::len);
    for col in 0..max_block_len {
        for (j, block) in blocks.iter().enumerate() {
            if (col != short_block_len - block_ecc_len || j >= num_short_blocks)
                && let Some(&cw) = block.get(col)
            {
                result.push(cw);
            }
        }
    }
    result
}

fn reed_solomon_compute_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    if let Some(last) = result.last_mut() {
        *last = 1;
    }
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..result.len() {
            result[j] = reed_solomon_multiply(result[j], root);
            if j + 1 < result.len() {
                result[j] ^= result[j + 1];
            }
        }
        root = reed_solomon_multiply(root, 0x02);
    }
    result
}

fn reed_solomon_compute_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &b in data {
        let factor = b ^ result.first().copied().unwrap_or(0);
        result.rotate_left(1);
        if let Some(last) = result.last_mut() {
            *last = 0;
        }
        for (slot, &coef) in result.iter_mut().zip(divisor.iter()) {
            *slot ^= reed_solomon_multiply(coef, factor);
        }
    }
    result
}

fn reed_solomon_multiply(x: u8, y: u8) -> u8 {
    let mut z = 0u16;
    let x16 = u16::from(x);
    let y16 = u16::from(y);
    for i in (0..8u8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11d);
        z ^= ((y16 >> i) & 1) * x16;
    }
    u8::try_from(z & 0xFF).unwrap_or(0)
}
