//! Incremental byte-by-byte UTF-8 decoder preserving split multi-byte sequences across chunks.

#[derive(Debug, Clone, Default)]
pub(crate) struct Utf8Decoder {
    buf: [u8; 4],
    len: u8,
    expected: u8,
}

impl Utf8Decoder {
    pub(crate) fn reset(&mut self) {
        self.len = 0;
        self.expected = 0;
    }

    /// Consumes `byte`, returning `(replacement_before_ascii, decoded_char, ascii_byte)`.
    pub(crate) fn push(&mut self, byte: u8) -> (Option<char>, Option<char>, Option<u8>) {
        let mut interrupted = None;
        if self.expected > 0 {
            if (0x80..=0xBF).contains(&byte) {
                let idx = usize::from(self.len);
                if idx < 4 {
                    self.buf[idx] = byte;
                    self.len += 1;
                }
                if self.len == self.expected {
                    let len = usize::from(self.len);
                    let buf = self.buf;
                    self.reset();
                    let ch = std::str::from_utf8(&buf[..len])
                        .ok()
                        .and_then(|s| s.chars().next())
                        .unwrap_or(char::REPLACEMENT_CHARACTER);
                    return (None, Some(ch), None);
                }
                return (None, None, None);
            }
            self.reset();
            interrupted = Some(char::REPLACEMENT_CHARACTER);
        }

        if byte < 0x80 {
            return (interrupted, None, Some(byte));
        }

        match byte {
            0xC2..=0xDF => {
                self.buf[0] = byte;
                self.len = 1;
                self.expected = 2;
                (interrupted, None, None)
            }
            0xE0..=0xEF => {
                self.buf[0] = byte;
                self.len = 1;
                self.expected = 3;
                (interrupted, None, None)
            }
            0xF0..=0xF4 => {
                self.buf[0] = byte;
                self.len = 1;
                self.expected = 4;
                (interrupted, None, None)
            }
            _ => (interrupted, Some(char::REPLACEMENT_CHARACTER), None),
        }
    }
}
