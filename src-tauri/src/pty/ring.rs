//! Bounded scrollback of raw PTY bytes, replayed when a pane (re)attaches.

use std::collections::VecDeque;

/// Default scrollback per agent (PLAN.md feature 3: about 2 MB).
pub const DEFAULT_CAP: usize = 2 * 1024 * 1024;

pub struct Ring {
    buf: VecDeque<u8>,
    cap: usize,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self { buf: VecDeque::with_capacity(cap.min(64 * 1024)), cap }
    }

    /// Append, dropping the oldest bytes past `cap`. The front is trimmed to a
    /// UTF-8 character boundary so a replay never starts mid-character.
    pub fn push(&mut self, data: &[u8]) {
        let data = &data[data.len().saturating_sub(self.cap)..];
        let over = (self.buf.len() + data.len()).saturating_sub(self.cap);
        self.buf.drain(..over);
        self.buf.extend(data);
        let partial = self.buf.iter().take(3).take_while(|&&b| b & 0xC0 == 0x80).count();
        self.buf.drain(..partial);
    }

    pub fn snapshot(&self) -> Vec<u8> {
        let (a, b) = self.buf.as_slices();
        [a, b].concat()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_everything_under_cap() {
        let mut r = Ring::new(10);
        r.push(b"abc");
        r.push(b"def");
        assert_eq!(r.snapshot(), b"abcdef");
    }

    #[test]
    fn drops_oldest_past_cap() {
        let mut r = Ring::new(5);
        r.push(b"abcd");
        r.push(b"efg");
        assert_eq!(r.snapshot(), b"cdefg");
        r.push(b"0123456789");
        assert_eq!(r.snapshot(), b"56789");
    }

    #[test]
    fn trims_to_char_boundary() {
        let mut r = Ring::new(4);
        r.push("é".as_bytes()); // 2 bytes
        r.push(b"abc"); // drops the first byte of é, so its tail must go too
        assert_eq!(r.snapshot(), b"abc");
    }
}
