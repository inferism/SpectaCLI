//! Terminal signals in an agent's output (PLAN.md feature 4, source 2): BEL,
//! OSC 9 / OSC 777 notifications and OSC 9;4 progress. The parser also hands
//! back the printable text, which the regex fallback matches against.

/// What the output said about the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// BEL, or an OSC 9 / OSC 777 desktop notification: it wants attention.
    Attention,
    /// OSC 9;4 progress: `true` while something is in progress, `false` when it was cleared.
    Progress(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum State {
    #[default]
    Ground,
    Esc,
    Csi,
    /// OSC body, ended by BEL or ESC \.
    Osc,
    OscEsc,
    /// DCS, APC, PM or SOS body, ended by ESC \.
    Skip,
    SkipEsc,
}

/// Longest OSC body kept; the rest of a longer one is dropped.
const MAX_OSC: usize = 512;

/// Incremental: a sequence may be split across `feed` calls.
#[derive(Default)]
pub struct Parser {
    state: State,
    osc: Vec<u8>,
}

impl Parser {
    /// Parse `data`, pushing signals to `out` and printable bytes (newlines
    /// for CR/LF, no escape sequences) to `text`.
    pub fn feed(&mut self, data: &[u8], out: &mut Vec<Signal>, text: &mut Vec<u8>) {
        for &b in data {
            self.step(b, out, text);
        }
    }

    fn step(&mut self, b: u8, out: &mut Vec<Signal>, text: &mut Vec<u8>) {
        self.state = match self.state {
            State::Ground => match b {
                0x1b => State::Esc,
                0x07 => {
                    out.push(Signal::Attention);
                    State::Ground
                }
                b'\n' => {
                    text.push(b'\n');
                    State::Ground
                }
                0..0x20 | 0x7f => State::Ground,
                _ => {
                    text.push(b);
                    State::Ground
                }
            },
            State::Esc => match b {
                b']' => {
                    self.osc.clear();
                    State::Osc
                }
                b'[' => State::Csi,
                b'P' | b'X' | b'^' | b'_' => State::Skip,
                0x1b => State::Esc,
                // Intermediates (`ESC ( B`): the final byte comes later.
                0x20..=0x2f => State::Esc,
                _ => State::Ground,
            },
            State::Csi => match b {
                0x40..=0x7e => State::Ground,
                0x1b => State::Esc,
                _ => State::Csi,
            },
            State::Osc => match b {
                0x07 => {
                    self.finish_osc(out);
                    State::Ground
                }
                0x1b => State::OscEsc,
                _ => {
                    if self.osc.len() < MAX_OSC {
                        self.osc.push(b);
                    }
                    State::Osc
                }
            },
            State::OscEsc => {
                if b == b'\\' {
                    self.finish_osc(out);
                    State::Ground
                } else {
                    // ESC aborted the OSC and starts a new sequence.
                    self.state = State::Esc;
                    return self.step(b, out, text);
                }
            }
            State::Skip => match b {
                0x1b => State::SkipEsc,
                0x18 | 0x1a => State::Ground,
                _ => State::Skip,
            },
            State::SkipEsc => {
                if b == b'\\' {
                    State::Ground
                } else {
                    self.state = State::Esc;
                    return self.step(b, out, text);
                }
            }
        };
    }

    fn finish_osc(&mut self, out: &mut Vec<Signal>) {
        let body = String::from_utf8_lossy(&self.osc);
        let (num, rest) = body.split_once(';').unwrap_or((&body, ""));
        match num {
            "9" => {
                if let Some(progress) = rest.strip_prefix("4;") {
                    // OSC 9;4;<state>;<percent>: 0 clears, 1 and 3 are in progress.
                    match progress.split(';').next() {
                        Some("0") => out.push(Signal::Progress(false)),
                        Some("1" | "3") => out.push(Signal::Progress(true)),
                        _ => {}
                    }
                } else if !is_conemu_command(rest) {
                    out.push(Signal::Attention);
                }
            }
            "777" if rest.starts_with("notify;") => out.push(Signal::Attention),
            _ => {}
        }
    }
}

/// ConEmu's `OSC 9;<n>;...` commands (the working-directory report of
/// Windows Terminal among them) are not notifications.
fn is_conemu_command(rest: &str) -> bool {
    rest.split_once(';').is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(chunks: &[&[u8]]) -> (Vec<Signal>, String) {
        let (mut p, mut sigs, mut text) = (Parser::default(), vec![], vec![]);
        for c in chunks {
            p.feed(c, &mut sigs, &mut text);
        }
        (sigs, String::from_utf8(text).unwrap())
    }

    #[test]
    fn bell_in_text_is_attention_but_an_osc_terminator_is_not() {
        assert_eq!(parse(&[b"done\x07"]), (vec![Signal::Attention], "done".into()));
        assert_eq!(parse(&[b"\x1b]0;title\x07"]), (vec![], String::new()));
    }

    #[test]
    fn notifications() {
        assert_eq!(parse(&[b"\x1b]9;Build done\x1b\\"]).0, [Signal::Attention]);
        assert_eq!(parse(&[b"\x1b]777;notify;Title;Body\x07"]).0, [Signal::Attention]);
        assert!(parse(&[b"\x1b]777;other;x\x07"]).0.is_empty());
        // Windows Terminal's cwd report is not a notification.
        assert!(parse(&[b"\x1b]9;9;C:\\work\x07"]).0.is_empty());
    }

    #[test]
    fn progress() {
        assert_eq!(parse(&[b"\x1b]9;4;3;\x07\x1b]9;4;1;40\x07\x1b]9;4;0;\x07"]).0, [
            Signal::Progress(true),
            Signal::Progress(true),
            Signal::Progress(false)
        ]);
        assert!(parse(&[b"\x1b]9;4;2;10\x07"]).0.is_empty(), "error state: no change");
    }

    #[test]
    fn sequences_split_across_chunks() {
        let (sigs, text) = parse(&[b"a\x1b]9;Hel", b"lo\x1b", b"\\b\x1b[3", b"1mc"]);
        assert_eq!((sigs, text.as_str()), (vec![Signal::Attention], "abc"));
    }

    #[test]
    fn text_has_no_escapes_and_skips_string_bodies() {
        let (sigs, text) = parse(&[b"\x1b[1;32mDo you want\x1b[0m to?\r\n\x1b(B\x1bPq\x07x\x1b\\ok"]);
        assert!(sigs.is_empty(), "a BEL inside DCS is not a bell");
        assert_eq!(text, "Do you want to?\nok");
    }

    #[test]
    fn esc_inside_osc_starts_a_new_sequence() {
        let (sigs, text) = parse(&[b"\x1b]9;x\x1b[31mred\x07"]);
        assert_eq!((sigs, text.as_str()), (vec![Signal::Attention], "red"));
    }
}
