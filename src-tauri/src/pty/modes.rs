//! Terminal modes a CLI sets once and relies on (bracketed paste, mouse,
//! alternate screen, ...). The ring feeds this the bytes it drops, so a replay
//! can start in the modes that were in force at its first byte even after the
//! sequences that set them scrolled out.

/// DEC private modes worth restoring, with their power-on default.
const MODES: [(u16, bool); 12] = [
    (1, false),    // application cursor keys
    (7, true),     // autowrap
    (25, true),    // cursor visible
    (47, false),   // alternate screen
    (1047, false), // alternate screen
    (1049, false), // alternate screen, saving the cursor
    (1000, false), // mouse: clicks
    (1002, false), // mouse: drags
    (1003, false), // mouse: all motion
    (1006, false), // mouse: SGR encoding
    (1004, false), // focus in/out reports
    (2004, false), // bracketed paste
];

const MAX_PARAMS: usize = 16;

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Ground,
    Esc,
    /// ESC plus intermediates, e.g. `ESC ( B`.
    EscInter,
    Csi,
    /// OSC, DCS, APC, PM or SOS body, ended by BEL or ESC \.
    Str,
}

pub struct Modes {
    state: State,
    on: [bool; MODES.len()],
    /// DECKPAM (`ESC =`) vs DECKPNM (`ESC >`).
    keypad: bool,
    // The CSI being parsed.
    private: bool,
    /// A prefix other than `?`, or an intermediate: not a mode we track.
    foreign: bool,
    params: [u16; MAX_PARAMS],
    n_params: usize,
    cur: u16,
}

impl Default for Modes {
    fn default() -> Self {
        Self {
            state: State::Ground,
            on: MODES.map(|(_, def)| def),
            keypad: false,
            private: false,
            foreign: false,
            params: [0; MAX_PARAMS],
            n_params: 0,
            cur: 0,
        }
    }
}

impl Modes {
    /// Between sequences: the next byte starts text or a new sequence.
    pub fn is_ground(&self) -> bool {
        self.state == State::Ground
    }

    pub fn feed(&mut self, b: u8) {
        self.state = match self.state {
            State::Ground => {
                if b == 0x1b {
                    State::Esc
                } else {
                    State::Ground
                }
            }
            State::Esc => self.esc(b),
            State::EscInter => match b {
                0x1b => State::Esc,
                0x20..=0x2f => State::EscInter,
                _ => State::Ground,
            },
            State::Csi => self.csi(b),
            // ESC inside a string ends it (as ST, or by starting a new sequence).
            State::Str => match b {
                0x07 | 0x18 | 0x1a => State::Ground,
                0x1b => State::Esc,
                _ => State::Str,
            },
        };
    }

    fn esc(&mut self, b: u8) -> State {
        match b {
            b'[' => {
                self.private = false;
                self.foreign = false;
                self.n_params = 0;
                self.cur = 0;
                State::Csi
            }
            b']' | b'P' | b'_' | b'^' | b'X' => State::Str,
            b'=' => {
                self.keypad = true;
                State::Ground
            }
            b'>' => {
                self.keypad = false;
                State::Ground
            }
            b'c' => {
                // RIS: full reset.
                *self = Self::default();
                State::Ground
            }
            0x1b => State::Esc,
            0x20..=0x2f => State::EscInter,
            _ => State::Ground,
        }
    }

    fn csi(&mut self, b: u8) -> State {
        match b {
            b'0'..=b'9' => {
                self.cur = self.cur.saturating_mul(10).saturating_add(u16::from(b - b'0'));
                State::Csi
            }
            b';' | b':' => {
                self.end_param();
                State::Csi
            }
            b'?' if self.n_params == 0 && self.cur == 0 && !self.private => {
                self.private = true;
                State::Csi
            }
            0x3c..=0x3f | 0x20..=0x2f => {
                self.foreign = true;
                State::Csi
            }
            0x40..=0x7e => {
                if self.private && !self.foreign && (b == b'h' || b == b'l') {
                    self.end_param();
                    for &p in &self.params[..self.n_params] {
                        if let Some(i) = MODES.iter().position(|&(m, _)| m == p) {
                            self.on[i] = b == b'h';
                        }
                    }
                }
                State::Ground
            }
            0x1b => State::Esc,
            0x18 | 0x1a => State::Ground,
            // Other C0 controls run in place without ending the sequence.
            _ => State::Csi,
        }
    }

    fn end_param(&mut self) {
        if self.n_params < MAX_PARAMS {
            self.params[self.n_params] = self.cur;
            self.n_params += 1;
        }
        self.cur = 0;
    }

    /// Sequences that put a fresh terminal into these modes. Empty when
    /// everything is at its default.
    pub fn preamble(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (&(mode, def), &on) in MODES.iter().zip(&self.on) {
            if on != def {
                out.extend_from_slice(format!("\x1b[?{mode}{}", if on { 'h' } else { 'l' }).as_bytes());
            }
        }
        if self.keypad {
            out.extend_from_slice(b"\x1b=");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fed(bytes: &[u8]) -> Modes {
        let mut m = Modes::default();
        bytes.iter().for_each(|&b| m.feed(b));
        m
    }

    #[test]
    fn defaults_need_no_preamble() {
        assert!(fed(b"hello \x1b[31mred\x1b[0m").preamble().is_empty());
    }

    #[test]
    fn tracks_set_and_reset() {
        let m = fed(b"\x1b[?2004h\x1b[?1000;1006h\x1b[?25l\x1b[?1000l\x1b=");
        assert_eq!(m.preamble(), b"\x1b[?25l\x1b[?1006h\x1b[?2004h\x1b=");
        assert!(m.is_ground());
    }

    #[test]
    fn ignores_non_private_and_foreign_csi() {
        // Plain `CSI 4 h` is insert mode; `CSI > 1 u` is kitty keyboard.
        assert!(fed(b"\x1b[2004h\x1b[>2004h\x1b[?2004$p").preamble().is_empty());
    }

    #[test]
    fn strings_hide_their_contents() {
        let m = fed(b"\x1b]0;title [?2004h\x07\x1b]8;;x\x1b\\");
        assert!(m.preamble().is_empty());
        assert!(m.is_ground());
        assert!(!fed(b"\x1b]0;unterminated").is_ground());
    }

    #[test]
    fn full_reset_clears_modes() {
        assert!(fed(b"\x1b[?1049h\x1bc").preamble().is_empty());
    }
}
