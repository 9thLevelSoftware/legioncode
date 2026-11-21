//! ISO/IEC 18004 QR matrix layout, mask evaluation, and Project Nayuki penalty scoring.

use crate::qr_code::QrEcc;
use crate::qr_code_tables::get_alignment_pattern_positions;

const PENALTY_N1: usize = 3;
const PENALTY_N2: usize = 3;
const PENALTY_N3: usize = 40;
const PENALTY_N4: usize = 10;

pub(crate) struct QrMatrix {
    pub(crate) size: u8,
    version: u8,
    ecc_format_bits: u8,
    pub(crate) modules: Vec<bool>,
    is_function: Vec<bool>,
}

impl QrMatrix {
    pub(crate) fn build(
        version: u8,
        ecl: QrEcc,
        all_codewords: &[u8],
        forced_mask: Option<u8>,
    ) -> (Self, u8) {
        let size = version * 4 + 17;
        let len = usize::from(size) * usize::from(size);
        let mut matrix = Self {
            size,
            version,
            ecc_format_bits: ecl.format_bits(),
            modules: vec![false; len],
            is_function: vec![false; len],
        };

        matrix.draw_function_patterns();
        matrix.draw_codewords(all_codewords);

        let mask = if let Some(m) = forced_mask {
            m
        } else {
            let mut best_mask = 0u8;
            let mut min_penalty = usize::MAX;
            for m in 0..8u8 {
                matrix.apply_mask(m);
                matrix.draw_format_bits(m);
                let penalty = matrix.get_penalty_score();
                if penalty < min_penalty {
                    min_penalty = penalty;
                    best_mask = m;
                }
                matrix.apply_mask(m);
            }
            best_mask
        };

        matrix.apply_mask(mask);
        matrix.draw_format_bits(mask);
        (matrix, mask)
    }

    fn size_usize(&self) -> usize {
        usize::from(self.size)
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.size_usize() + x
    }

    fn get(&self, x: usize, y: usize) -> bool {
        self.modules[self.idx(x, y)]
    }

    fn set_function_module(&mut self, x: usize, y: usize, is_dark: bool) {
        let index = self.idx(x, y);
        self.modules[index] = is_dark;
        self.is_function[index] = true;
    }

    fn draw_function_patterns(&mut self) {
        let size = self.size_usize();
        for i in 0..size {
            self.set_function_module(6, i, i % 2 == 0);
            self.set_function_module(i, 6, i % 2 == 0);
        }

        let size_i32 = i32::from(self.size);
        self.draw_finder_pattern(3, 3);
        self.draw_finder_pattern(size_i32 - 4, 3);
        self.draw_finder_pattern(3, size_i32 - 4);

        let align_pos = get_alignment_pattern_positions(self.version);
        let num_align = align_pos.len();
        for (i, &px) in align_pos.iter().enumerate() {
            for (j, &py) in align_pos.iter().enumerate() {
                let corner =
                    (i == 0 && (j == 0 || j + 1 == num_align)) || (i + 1 == num_align && j == 0);
                if !corner {
                    self.draw_alignment_pattern(i32::from(px), i32::from(py));
                }
            }
        }

        self.draw_format_bits(0);
        self.draw_version();
    }

    fn draw_format_bits(&mut self, mask: u8) {
        let data = (u32::from(self.ecc_format_bits) << 3) | u32::from(mask);
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let bits = ((data << 10) | rem) ^ 0x5412;

        let size = self.size_usize();
        for i in 0..=5usize {
            self.set_function_module(8, i, bit_is_set(bits, i));
        }
        self.set_function_module(8, 7, bit_is_set(bits, 6));
        self.set_function_module(8, 8, bit_is_set(bits, 7));
        self.set_function_module(7, 8, bit_is_set(bits, 8));
        for i in 9..15usize {
            self.set_function_module(14 - i, 8, bit_is_set(bits, i));
        }

        for i in 0..8usize {
            self.set_function_module(size - 1 - i, 8, bit_is_set(bits, i));
        }
        for i in 8..15usize {
            self.set_function_module(8, size - 15 + i, bit_is_set(bits, i));
        }
        self.set_function_module(8, size - 8, true);
    }

    fn draw_version(&mut self) {
        if self.version < 7 {
            return;
        }
        let mut rem = u32::from(self.version);
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1f25);
        }
        let bits = (u32::from(self.version) << 12) | rem;
        let size = self.size_usize();
        for i in 0..18usize {
            let color = bit_is_set(bits, i);
            let a = size - 11 + (i % 3);
            let b = i / 3;
            self.set_function_module(a, b, color);
            self.set_function_module(b, a, color);
        }
    }

    fn draw_finder_pattern(&mut self, x: i32, y: i32) {
        let size_i32 = i32::from(self.size);
        for dy in -4..=4i32 {
            for dx in -4..=4i32 {
                let dist = dx.abs().max(dy.abs());
                let xx = x + dx;
                let yy = y + dy;
                if xx >= 0
                    && xx < size_i32
                    && yy >= 0
                    && yy < size_i32
                    && let (Ok(ux), Ok(uy)) = (usize::try_from(xx), usize::try_from(yy))
                {
                    self.set_function_module(ux, uy, dist != 2 && dist != 4);
                }
            }
        }
    }

    fn draw_alignment_pattern(&mut self, x: i32, y: i32) {
        for dy in -2..=2i32 {
            for dx in -2..=2i32 {
                let dist = dx.abs().max(dy.abs());
                if let (Ok(ux), Ok(uy)) = (usize::try_from(x + dx), usize::try_from(y + dy)) {
                    self.set_function_module(ux, uy, dist != 1);
                }
            }
        }
    }

    fn draw_codewords(&mut self, data: &[u8]) {
        let size = self.size_usize();
        let size_i32 = i32::from(self.size);
        let total_bits = data.len() * 8;
        let mut bit_idx = 0usize;
        let mut right = size_i32 - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            let right_usize = usize::try_from(right).unwrap_or(0);
            let upward = ((right_usize + 1) & 2) == 0;
            for vert in 0..size {
                let y = if upward { size - 1 - vert } else { vert };
                for j in 0..2usize {
                    let x = right_usize - j;
                    let idx = self.idx(x, y);
                    if !self.is_function[idx] && bit_idx < total_bits {
                        let byte = data[bit_idx >> 3];
                        self.modules[idx] = ((byte >> (7 - (bit_idx & 7))) & 1) != 0;
                        bit_idx += 1;
                    }
                }
            }
            right -= 2;
        }
    }

    fn apply_mask(&mut self, mask: u8) {
        let size = self.size_usize();
        for y in 0..size {
            for x in 0..size {
                let invert = match mask {
                    0 => (x + y).is_multiple_of(2),
                    1 => y.is_multiple_of(2),
                    2 => x.is_multiple_of(3),
                    3 => (x + y).is_multiple_of(3),
                    4 => ((x / 3) + (y / 2)).is_multiple_of(2),
                    5 => ((x * y) % 2) + ((x * y) % 3) == 0,
                    6 => (((x * y) % 2) + ((x * y) % 3)).is_multiple_of(2),
                    _ => (((x + y) % 2) + ((x * y) % 3)).is_multiple_of(2),
                };
                let idx = self.idx(x, y);
                if !self.is_function[idx] && invert {
                    self.modules[idx] = !self.modules[idx];
                }
            }
        }
    }

    fn get_penalty_score(&self) -> usize {
        let size = self.size_usize();
        let mut result = 0usize;

        for y in 0..size {
            result += score_line_runs(size, |x| self.get(x, y));
        }
        for x in 0..size {
            result += score_line_runs(size, |y| self.get(x, y));
        }

        for y in 0..size - 1 {
            for x in 0..size - 1 {
                let color = self.get(x, y);
                if color == self.get(x + 1, y)
                    && color == self.get(x, y + 1)
                    && color == self.get(x + 1, y + 1)
                {
                    result += PENALTY_N2;
                }
            }
        }

        let dark = self.modules.iter().filter(|&&m| m).count();
        let total = size * size;
        let diff = (dark * 20).abs_diff(total * 10);
        let k = diff.saturating_sub(1) / total;
        result + k * PENALTY_N4
    }
}

fn bit_is_set(word: u32, bit_pos: usize) -> bool {
    ((word >> bit_pos) & 1) != 0
}

fn score_line_runs(size: usize, mut cell_at: impl FnMut(usize) -> bool) -> usize {
    let mut score = 0usize;
    let mut run_color = false;
    let mut run_len = 0usize;
    let mut run_history = [0usize; 7];

    for pos in 0..size {
        let color = cell_at(pos);
        if color == run_color {
            run_len += 1;
            if run_len == 5 {
                score += PENALTY_N1;
            } else if run_len > 5 {
                score += 1;
            }
        } else {
            finder_penalty_add_history(size, run_len, &mut run_history);
            if !run_color {
                score += finder_penalty_count_patterns(&run_history) * PENALTY_N3;
            }
            run_color = color;
            run_len = 1;
        }
    }

    if run_color {
        finder_penalty_add_history(size, run_len, &mut run_history);
        run_len = 0;
    }
    run_len += size;
    finder_penalty_add_history(size, run_len, &mut run_history);
    score + finder_penalty_count_patterns(&run_history) * PENALTY_N3
}

fn finder_penalty_add_history(size: usize, mut current_run_len: usize, history: &mut [usize; 7]) {
    if history[0] == 0 {
        current_run_len += size;
    }
    history.rotate_right(1);
    history[0] = current_run_len;
}

fn finder_penalty_count_patterns(history: &[usize; 7]) -> usize {
    let n = history[1];
    let core =
        n > 0 && history[2] == n && history[3] == n * 3 && history[4] == n && history[5] == n;
    usize::from(core && history[0] >= n * 4 && history[6] >= n)
        + usize::from(core && history[6] >= n * 4 && history[0] >= n)
}
