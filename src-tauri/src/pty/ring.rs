//! Bounded scrollback of raw PTY bytes, replayed when a pane (re)attaches.

use super::modes::Modes;
use std::collections::VecDeque;

/// Default scrollback per agent (PLAN.md feature 3: about 2 MB).
pub const DEFAULT_CAP: usize = 2 * 1024 * 1024;

pub struct Ring {
    buf: VecDeque<u8>,
    cap: usize,
    /// Fed every byte that falls off the front: the modes in force at the
    /// first byte still in `buf`.
    head: Modes,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self { buf: VecDeque::with_capacity(cap.min(64 * 1024)), cap, head: Modes::default() }
    }

    /// Append, dropping the oldest bytes past `cap`. The front is then
    /// trimmed to a sequence and UTF-8 boundary, so a replay never starts
    /// inside an escape sequence or a character.
    pub fn push(&mut self, data: &[u8]) {
        let (lost, data) = data.split_at(data.len().saturating_sub(self.cap));
        let over = (self.buf.len() + data.len()).saturating_sub(self.cap);
        for b in self.buf.drain(..over).chain(lost.iter().copied()) {
            self.head.feed(b);
        }
        self.buf.extend(data);
        while let Some(&b) = self.buf.front() {
            if self.head.is_ground() && b & 0xC0 != 0x80 {
                break;
            }
            self.head.feed(b);
            self.buf.pop_front();
        }
    }

    /// Bytes to replay into a fresh terminal: the modes in force at the
    /// oldest byte, then the buffer.
    pub fn snapshot(&self) -> Vec<u8> {
        let (a, b) = self.buf.as_slices();
        [&self.head.preamble(), a, b].concat()
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

    #[test]
    fn replay_restores_modes_that_scrolled_out() {
        let mut r = Ring::new(8);
        r.push(b"\x1b[?2004h");
        r.push(b"12345678");
        assert_eq!(r.snapshot(), b"\x1b[?2004h12345678");
        r.push(b"\x1b[?2004l");
        r.push(b"abcdefgh");
        assert_eq!(r.snapshot(), b"abcdefgh");
    }

    #[test]
    fn never_starts_mid_sequence() {
        let mut r = Ring::new(8);
        r.push(b"\x1b[31mxyzab"); // the cut lands inside `ESC [ 31 m`: skip its tail
        assert_eq!(r.snapshot(), b"xyzab");
        r.push(b"\x1b]0;long title");
        r.push(b"\x07ok");
        assert_eq!(r.snapshot(), b"ok");
    }
}
