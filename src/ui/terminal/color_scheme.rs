//! Lets programs follow the terminal's light or dark theme: palette colors for
//! the OSC 4/10/11/12 queries, and the color scheme reports of DEC private mode
//! 2031 with its `CSI ? 996 n` query.
use alacritty_terminal::vte::ansi::{NamedColor, Rgb};
use gpui::{Hsla, Rgba};
use gpui_terminal::ColorPalette;

const REPORT_MODE: &[u8] = b"2031";
const QUERY: &[u8] = b"996";
const MAX_PARAMETERS: usize = 64;

/// `CSI ? 997 ; 1 n` reports a dark scheme and `CSI ? 997 ; 2 n` a light one.
pub(super) fn report(dark: bool) -> &'static [u8] {
    if dark {
        b"\x1b[?997;1n"
    } else {
        b"\x1b[?997;2n"
    }
}

pub(super) fn is_dark(palette: &ColorPalette) -> bool {
    palette.background().l < 0.5
}

/// The theme color a query for `index` reports while the program has not
/// overridden it: the 256 indexed colors, then foreground, background, cursor.
pub(super) fn palette_color(palette: &ColorPalette, index: usize) -> Option<Rgb> {
    let color = match index {
        0..=15 => palette.ansi_colors()[index],
        16..=255 => palette.extended_colors()[index],
        _ if index == NamedColor::Foreground as usize => palette.foreground(),
        _ if index == NamedColor::Background as usize => palette.background(),
        _ if index == NamedColor::Cursor as usize => palette.cursor(),
        _ => return None,
    };
    Some(to_rgb(color))
}

fn to_rgb(color: Hsla) -> Rgb {
    let color = Rgba::from(color);
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    Rgb {
        r: channel(color.r),
        g: channel(color.g),
        b: channel(color.b),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    /// The program asked which scheme is in use.
    Query,
    /// The program turned the unsolicited reports on or off.
    Reports(bool),
}

#[derive(Default)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi,
    /// A control sequence this scanner does not handle, skipped up to its
    /// final byte.
    IgnoredCsi,
}

/// Finds the color scheme sequences in PTY output. The terminal parser drops
/// them as unknown, so they are read from the same bytes before it runs.
#[derive(Default)]
pub(super) struct Scanner {
    state: State,
    parameters: Vec<u8>,
}

impl Scanner {
    pub fn scan(&mut self, bytes: &[u8], mut on_event: impl FnMut(Event)) {
        for &byte in bytes {
            match self.state {
                State::Ground => {
                    if byte == b'\x1b' {
                        self.state = State::Escape;
                    }
                }
                State::Escape => match byte {
                    b'[' => {
                        self.parameters.clear();
                        self.state = State::Csi;
                    }
                    b'\x1b' => {}
                    b'c' => {
                        // A full reset also resets the mode.
                        on_event(Event::Reports(false));
                        self.state = State::Ground;
                    }
                    _ => self.state = State::Ground,
                },
                State::Csi | State::IgnoredCsi => match byte {
                    b'\x1b' => self.state = State::Escape,
                    // CAN and SUB cancel the sequence.
                    0x18 | 0x1a => self.state = State::Ground,
                    0x40..=0x7e => {
                        if matches!(self.state, State::Csi) {
                            self.finish(byte, &mut on_event);
                        }
                        self.state = State::Ground;
                    }
                    0x30..=0x3f if self.parameters.len() < MAX_PARAMETERS => {
                        self.parameters.push(byte);
                    }
                    // Too long, or with intermediate bytes: another sequence.
                    0x20..=0x3f => self.state = State::IgnoredCsi,
                    // Other control characters do not end the sequence.
                    _ => {}
                },
            }
        }
    }

    fn finish(&self, final_byte: u8, on_event: &mut impl FnMut(Event)) {
        let Some(parameters) = self.parameters.strip_prefix(b"?") else {
            return;
        };
        match final_byte {
            b'h' | b'l' if parameters.split(|byte| *byte == b';').any(|mode| mode == REPORT_MODE) => {
                on_event(Event::Reports(final_byte == b'h'));
            }
            b'n' if parameters == QUERY => on_event(Event::Query),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(chunks: &[&[u8]]) -> Vec<Event> {
        let mut scanner = Scanner::default();
        let mut events = Vec::new();
        for chunk in chunks {
            scanner.scan(chunk, |event| events.push(event));
        }
        events
    }

    #[test]
    fn reports_follow_the_private_mode_and_a_full_reset() {
        assert_eq!(events(&[b"\x1b[?2031h"]), [Event::Reports(true)]);
        assert_eq!(events(&[b"\x1b[?2031l"]), [Event::Reports(false)]);
        assert_eq!(
            events(&[b"text\x1b[?1004;2031h\x1b[2J\x1b[?2031;1004l\x1bc"]),
            [
                Event::Reports(true),
                Event::Reports(false),
                Event::Reports(false)
            ]
        );
    }

    #[test]
    fn the_query_and_split_sequences_are_recognized() {
        assert_eq!(events(&[b"\x1b[?996n"]), [Event::Query]);
        assert_eq!(
            events(&[b"\x1b", b"[?20", b"31", b"h\x1b[?9", b"96n"]),
            [Event::Reports(true), Event::Query]
        );
        // An escape restarts the sequence it interrupts.
        assert_eq!(events(&[b"\x1b[?20\x1b[?2031h"]), [Event::Reports(true)]);
    }

    #[test]
    fn other_sequences_are_ignored() {
        for sequence in [
            &b"\x1b[2031h"[..],
            b"\x1b[?12031h",
            b"\x1b[?2031$p",
            b"\x1b[?2031\x18h",
            b"\x1b[996n",
            b"\x1b[?996;1n",
            b"\x1b[?997;1n",
            b"\x1b]0;\x1b\\?2031h",
            b"[?2031h",
        ] {
            assert_eq!(events(&[sequence]), [], "{sequence:?}");
        }
        let long = [&b"\x1b[?"[..], &b"1;".repeat(MAX_PARAMETERS), b"2031h"].concat();
        assert_eq!(events(&[&long]), []);
    }

    #[test]
    fn a_color_query_is_answered_before_the_requests_that_follow_it() {
        use super::super::{FastEventProxy, FastTerminalEvent, TermSize};
        use alacritty_terminal::{
            term::{Config, Term},
            vte::ansi::Processor,
        };

        let (sender, receiver) = flume::bounded(8);
        let size = TermSize { cols: 80, rows: 24 };
        let mut term = Term::new(Config::default(), &size, FastEventProxy { sender });
        let mut parser: Processor = Processor::new();
        // Programs send a device attributes request after their queries and
        // take its reply to mean that no other reply is coming.
        parser.advance(&mut term, b"\x1b]11;?\x07\x1b[c");

        let palette = ColorPalette::builder().background(0xf8, 0xf9, 0xfb).build();
        let replies = receiver
            .try_iter()
            .map(|event| match event {
                FastTerminalEvent::ColorRequest(index, format) => {
                    format(palette_color(&palette, index).unwrap())
                }
                FastTerminalEvent::PtyWrite(text) => text,
                _ => panic!("Expected a reply"),
            })
            .collect::<Vec<_>>();
        assert_eq!(replies, ["\x1b]11;rgb:f8f8/f9f9/fbfb\x07", "\x1b[?6c"]);
    }

    #[test]
    fn queried_colors_come_from_the_theme() {
        let palette = ColorPalette::builder()
            .background(0xf8, 0xf9, 0xfb)
            .foreground(0x24, 0x2b, 0x38)
            .cursor(0x18, 0x21, 0x2f)
            .red(0xc7, 0x3e, 0x4e)
            .build();
        let color = |index| palette_color(&palette, index).map(|c| (c.r, c.g, c.b));
        assert_eq!(color(NamedColor::Background as usize), Some((0xf8, 0xf9, 0xfb)));
        assert_eq!(color(NamedColor::Foreground as usize), Some((0x24, 0x2b, 0x38)));
        assert_eq!(color(NamedColor::Cursor as usize), Some((0x18, 0x21, 0x2f)));
        assert_eq!(color(1), Some((0xc7, 0x3e, 0x4e)));
        assert_eq!(color(231), Some((0xff, 0xff, 0xff)));
        assert_eq!(color(NamedColor::Cursor as usize + 1), None);

        assert!(!is_dark(&palette));
        assert_eq!(report(false), b"\x1b[?997;2n");
        let dark = ColorPalette::builder().background(0x0c, 0x0e, 0x12).build();
        assert!(is_dark(&dark));
        assert_eq!(report(true), b"\x1b[?997;1n");
    }
}
