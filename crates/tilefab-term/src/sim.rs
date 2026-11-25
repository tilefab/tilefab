//! Small terminal emulator for tests (compiled only with `cargo test`).
//!
//! It interprets only sequences emitted by our backend: cursor movement (`H`),
//! clearing (`J`, `K`), scroll region (`r`), scrolling (`S`, `T`), line feeds,
//! cursor visibility (`?25`), synchronized updates (`?2026`), and styles (`m`, ignored).
//! It keeps scrollback and statistics for each synchronized block, checked
//! by the flicker prevention tests.
//!
//! Two behaviors of the scrollback (lines leaving the top of the screen):
//!
//! - **strict** ([`Sim::new`], the default): only a line feed on the last row of a
//!   full-screen region saves the top line. `CSI n S` and partial regions delete their lines.
//!   This is what many terminals do (xterm.js in VS Code, iTerm2 by default…);
//! - **xterm** ([`Sim::xterm`]): any scroll of a region whose top is row 0 saves its lines,
//!   `CSI n S` included.

use std::collections::BTreeSet;

use unicode_segmentation::UnicodeSegmentation;

use tilefab_core::buffer::grapheme_width;
use tilefab_core::convert::saturating_u16;

/// What happened inside an `ESC[?2026h … ESC[?2026l` block.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BlockStats {
    /// Total rows scrolled (`CSI n S`, `CSI n T`, and line feeds at the region bottom).
    pub scrolled: u32,
    /// `ESC[2J` or `ESC[J` (clear to bottom) seen in the block.
    pub erased_screen: bool,
    /// Screen rows where at least one character was written.
    pub rows_written: BTreeSet<u16>,
    /// Rows cleared by `ESC[2K`.
    pub rows_erased: BTreeSet<u16>,
}

#[derive(Debug)]
pub struct Sim {
    pub width: u16,
    pub height: u16,
    screen: Vec<Vec<String>>,
    pub scrollback: Vec<String>,
    pub cursor: (u16, u16),
    pub cursor_visible: bool,
    region: (u16, u16),
    /// Strict scrollback (see the module header); `false` for xterm's behavior.
    strict: bool,
    in_sync: bool,
    /// One block per completed synchronized update.
    pub blocks: Vec<BlockStats>,
    current: BlockStats,
    /// Bytes received outside any synchronized block (excluding `?2026h` and `?2026l` themselves).
    pub outside_sync: Vec<u8>,
    /// Sequences that reach past the screen (a cursor move, a scroll region or a scroll
    /// amount beyond its size): the simulator clamps them like a terminal, but the backend
    /// should never emit them. Tests check that this stays empty.
    pub out_of_range: Vec<String>,
    pending: Vec<u8>,
}

fn blank_row(width: u16) -> Vec<String> {
    vec![" ".to_string(); width as usize]
}

impl Sim {
    /// A terminal with the strict scrollback behavior (see the module header).
    pub fn new(width: u16, height: u16) -> Self {
        Self::with_scrollback(width, height, true)
    }

    /// A terminal with xterm's scrollback behavior (see the module header).
    pub fn xterm(width: u16, height: u16) -> Self {
        Self::with_scrollback(width, height, false)
    }

    fn with_scrollback(width: u16, height: u16, strict: bool) -> Self {
        Self {
            strict,
            width,
            height,
            screen: (0..height).map(|_| blank_row(width)).collect(),
            scrollback: Vec::new(),
            cursor: (0, 0),
            cursor_visible: true,
            region: (0, height - 1),
            in_sync: false,
            blocks: Vec::new(),
            current: BlockStats::default(),
            outside_sync: Vec::new(),
            out_of_range: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Screen lines, without trailing spaces.
    pub fn screen_lines(&self) -> Vec<String> {
        self.screen
            .iter()
            .map(|row| row.concat().trim_end().to_string())
            .collect()
    }

    /// Scrollback followed by the screen, without trailing blank lines.
    pub fn all_lines(&self) -> Vec<String> {
        let mut all: Vec<String> = self.scrollback.clone();
        all.extend(self.screen_lines());
        while all.last().is_some_and(std::string::String::is_empty) {
            all.pop();
        }
        all
    }

    /// Resize the screen like a terminal without reflow: truncate content or
    /// pad it with blanks.
    ///
    /// A pty can report 0 rows or 0 columns, but a terminal emulator still has at least
    /// one cell: a zero size is kept at 1, so the simulator never has an empty screen.
    pub fn resize(&mut self, width: u16, height: u16) {
        let (width, height) = (width.max(1), height.max(1));
        for row in &mut self.screen {
            row.resize(width as usize, " ".to_string());
        }
        while self.screen.len() > height as usize {
            // Top lines enter scrollback, as on a real terminal.
            let row = self.screen.remove(0);
            self.scrollback.push(row.concat().trim_end().to_string());
            self.cursor.1 = self.cursor.1.saturating_sub(1);
        }
        while self.screen.len() < height as usize {
            self.screen.push(blank_row(width));
        }
        self.width = width;
        self.height = height;
        self.region = (0, height - 1);
        self.cursor.0 = self.cursor.0.min(width - 1);
        self.cursor.1 = self.cursor.1.min(height - 1);
    }

    /// Write plain text at the cursor position (screen setup before a test).
    pub fn feed_str(&mut self, s: &str) {
        self.feed(s.as_bytes());
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        let data = std::mem::take(&mut self.pending);
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            if b == 0x1b {
                // Incomplete sequence at the end of the data: keep it for the next `feed`.
                let Some((consumed, seq)) = parse_csi(&data[i..]) else {
                    self.pending = data[i..].to_vec();
                    return;
                };
                self.csi(&seq);
                i += consumed;
            } else if b == b'\n' {
                self.note_outside(&data[i..=i]);
                self.line_feed();
                i += 1;
            } else if b == b'\r' {
                self.note_outside(&data[i..=i]);
                self.cursor.0 = 0;
                i += 1;
            } else {
                // A text chunk up to the next control character.
                let end = data[i..]
                    .iter()
                    .position(|&c| c == 0x1b || c == b'\n' || c == b'\r')
                    .map_or(data.len(), |p| i + p);
                // A UTF-8 character split at the end of the data: keep it for later.
                let (text, rest) = match std::str::from_utf8(&data[i..end]) {
                    Ok(t) => (t, 0),
                    Err(e) if e.error_len().is_none() => {
                        let ok = i + e.valid_up_to();
                        (std::str::from_utf8(&data[i..ok]).unwrap(), end - ok)
                    }
                    Err(e) => panic!("invalid UTF-8 in the output: {e}"),
                };
                self.note_outside(text.as_bytes());
                self.print(text);
                if rest > 0 {
                    self.pending = data[end - rest..end].to_vec();
                }
                i = end;
            }
        }
    }

    fn note_out_of_range(&mut self, seq: &Csi) {
        self.out_of_range
            .push(String::from_utf8_lossy(&seq.raw).into_owned());
    }

    fn note_outside(&mut self, bytes: &[u8]) {
        if !self.in_sync {
            self.outside_sync.extend_from_slice(bytes);
        }
    }

    fn print(&mut self, text: &str) {
        for g in text.graphemes(true) {
            let w = saturating_u16(grapheme_width(g).max(1));
            if self.cursor.0 + w > self.width {
                // Automatic line wrap (our sequences should not trigger it).
                self.cursor.0 = 0;
                self.line_feed();
            }
            let (x, y) = self.cursor;
            self.screen[y as usize][x as usize] = g.to_string();
            if w == 2 {
                self.screen[y as usize][x as usize + 1] = String::new();
            }
            self.current.rows_written.insert(y);
            self.cursor.0 += w;
        }
    }

    fn line_feed(&mut self) {
        if self.cursor.1 == self.region.1 {
            self.scroll_up(1, true);
        } else if self.cursor.1 + 1 < self.height {
            self.cursor.1 += 1;
        }
    }

    /// Scroll the region up by `n` rows, by line feeds (`line_feed`) or by `CSI n S`.
    fn scroll_up(&mut self, n: u16, line_feed: bool) {
        let (top, bottom) = self.region;
        let saved = if self.strict {
            line_feed && (top, bottom) == (0, self.height - 1)
        } else {
            top == 0
        };
        for _ in 0..n {
            let row = self.screen.remove(top as usize);
            if saved {
                self.scrollback.push(row.concat().trim_end().to_string());
            }
            self.screen.insert(bottom as usize, blank_row(self.width));
        }
        self.current.scrolled += u32::from(n);
    }

    fn scroll_down(&mut self, n: u16) {
        let (top, bottom) = self.region;
        for _ in 0..n {
            self.screen.remove(bottom as usize);
            self.screen.insert(top as usize, blank_row(self.width));
        }
        self.current.scrolled += u32::from(n);
    }

    fn csi(&mut self, seq: &Csi) {
        let p = |i: usize, default: u16| -> u16 {
            seq.params
                .get(i)
                .copied()
                .flatten()
                .filter(|&v| v != 0)
                .unwrap_or(default)
        };
        match (seq.private, seq.final_byte) {
            (true, b'h' | b'l') => {
                let on = seq.final_byte == b'h';
                match seq.params.first().copied().flatten() {
                    Some(25) => self.cursor_visible = on,
                    Some(2026) => {
                        if on {
                            assert!(!self.in_sync, "nested synchronized block");
                            self.in_sync = true;
                            self.current = BlockStats::default();
                        } else {
                            assert!(self.in_sync, "block end without a start");
                            self.in_sync = false;
                            self.blocks.push(std::mem::take(&mut self.current));
                        }
                    }
                    _ => {}
                }
            }
            (false, b'H') => {
                self.note_outside(&seq.raw);
                if p(0, 1) > self.height || p(1, 1) > self.width {
                    self.note_out_of_range(seq);
                }
                let row = p(0, 1).min(self.height) - 1;
                let col = p(1, 1).min(self.width) - 1;
                self.cursor = (col, row);
            }
            (false, b'J') => {
                self.note_outside(&seq.raw);
                self.current.erased_screen = true;
                if seq.params.first().copied().flatten() == Some(2) {
                    // `ESC[2J`: the entire visible screen.
                    for row in &mut self.screen {
                        *row = blank_row(self.width);
                    }
                } else {
                    // `ESC[J`: from the cursor to the screen bottom.
                    let (x, y) = (self.cursor.0 as usize, self.cursor.1 as usize);
                    for cell in &mut self.screen[y][x..] {
                        *cell = " ".to_string();
                    }
                    for row in &mut self.screen[y + 1..] {
                        *row = blank_row(self.width);
                    }
                }
            }
            (false, b'K') => {
                self.note_outside(&seq.raw);
                assert_eq!(
                    seq.params.first().copied().flatten(),
                    Some(2),
                    "only ESC[2K is handled"
                );
                self.screen[self.cursor.1 as usize] = blank_row(self.width);
                self.current.rows_erased.insert(self.cursor.1);
            }
            (false, b'r') => {
                self.note_outside(&seq.raw);
                if p(1, self.height) > self.height || p(0, 1) > p(1, self.height) {
                    self.note_out_of_range(seq);
                }
                self.region = (p(0, 1) - 1, p(1, self.height).min(self.height) - 1);
                self.cursor = (0, 0);
            }
            (false, b'S') => {
                self.note_outside(&seq.raw);
                if p(0, 1) > self.height {
                    self.note_out_of_range(seq);
                }
                self.scroll_up(p(0, 1).min(self.height), false);
            }
            (false, b'T') => {
                self.note_outside(&seq.raw);
                if p(0, 1) > self.height {
                    self.note_out_of_range(seq);
                }
                self.scroll_down(p(0, 1).min(self.height));
            }
            (false, b'm') => self.note_outside(&seq.raw),
            (_, other) => panic!("sequence not handled by the simulator: {seq:?} ({other})"),
        }
    }
}

#[derive(Debug)]
struct Csi {
    private: bool,
    params: Vec<Option<u16>>,
    final_byte: u8,
    raw: Vec<u8>,
}

/// Parse `ESC [ [?] params final` at the start of `data`. `None` if the sequence is incomplete.
fn parse_csi(data: &[u8]) -> Option<(usize, Csi)> {
    if data.len() < 2 {
        return None;
    }
    assert_eq!(data[1], b'[', "only CSI sequences are handled");
    let mut i = 2;
    let private = data.get(i) == Some(&b'?');
    if private {
        i += 1;
    }
    let start = i;
    while i < data.len() && (data[i].is_ascii_digit() || data[i] == b';') {
        i += 1;
    }
    let final_byte = *data.get(i)?;
    let params = std::str::from_utf8(&data[start..i])
        .unwrap()
        .split(';')
        .map(|s| s.parse().ok())
        .collect();
    Some((
        i + 1,
        Csi {
            private,
            params,
            final_byte,
            raw: data[..=i].to_vec(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_moves_and_erases() {
        let mut s = Sim::new(6, 3);
        s.feed(b"\x1b[2;3Hab\x1b[3;1Hxyz\x1b[3;1H\x1b[2K");
        assert_eq!(s.screen_lines(), vec!["", "  ab", ""]);
    }

    #[test]
    fn line_feed_at_the_bottom_scrolls_into_the_scrollback() {
        let mut s = Sim::new(4, 2);
        s.feed_str("a\r\nb\r\nc");
        assert_eq!(s.scrollback, vec!["a"]);
        assert_eq!(s.screen_lines(), vec!["b", "c"]);
    }

    #[test]
    fn scroll_region_only_moves_its_rows_and_keeps_history_only_from_the_top() {
        let mut s = Sim::xterm(4, 4);
        s.feed_str("0\r\n1\r\n2\r\n3");
        s.feed(b"\x1b[1;2r\x1b[1S\x1b[r");
        assert_eq!(s.screen_lines(), vec!["1", "", "2", "3"]);
        assert_eq!(s.scrollback, vec!["0"]);

        let mut s = Sim::xterm(4, 4);
        s.feed_str("0\r\n1\r\n2\r\n3");
        s.feed(b"\x1b[2;3r\x1b[1S\x1b[r");
        assert_eq!(s.screen_lines(), vec!["0", "2", "", "3"]);
        assert_eq!(s.scrollback, Vec::<String>::new());
    }

    #[test]
    fn strict_scrollback_keeps_only_lines_fed_out_of_the_whole_screen() {
        // `CSI S` and a partial region from the top: the lines are deleted.
        let mut s = Sim::new(4, 4);
        s.feed_str("0\r\n1\r\n2\r\n3");
        s.feed(b"\x1b[1;2r\x1b[1S\x1b[r");
        assert_eq!(s.screen_lines(), vec!["1", "", "2", "3"]);
        assert_eq!(s.scrollback, Vec::<String>::new());
        s.feed(b"\x1b[1S");
        assert_eq!(s.scrollback, Vec::<String>::new());
        // A line feed at the bottom of a partial region from the top: deleted too.
        s.feed(b"\x1b[1;3r\x1b[3;1H\n\x1b[r");
        assert_eq!(s.scrollback, Vec::<String>::new());
        // A line feed on the last row of the whole screen: saved.
        s.feed(b"\x1b[4;1H\n");
        assert_eq!(s.scrollback.len(), 1);
    }

    #[test]
    fn sync_blocks_collect_statistics_and_outside_bytes_are_tracked() {
        let mut s = Sim::new(5, 3);
        s.feed(b"\x1b[?2026h\x1b[1;1Hab\x1b[3;1H\x1b[2K\x1b[?2026l");
        assert_eq!(s.blocks.len(), 1);
        assert_eq!(s.blocks[0].rows_written, BTreeSet::from([0]));
        assert_eq!(s.blocks[0].rows_erased, BTreeSet::from([2]));
        assert_eq!(s.outside_sync, b"");
        s.feed(b"\x1b[1;1Hx");
        assert_ne!(s.outside_sync, b"");
    }

    #[test]
    fn incomplete_sequences_and_characters_wait_for_the_next_chunk() {
        let mut s = Sim::new(5, 1);
        let bytes = "é\x1b[1;3Hz".as_bytes();
        s.feed(&bytes[..1]); // half of “é”
        s.feed(&bytes[1..4]); // the end of “é”, then the start of a sequence
        s.feed(&bytes[4..]);
        assert_eq!(s.screen_lines(), vec!["é z"]);
    }
}
