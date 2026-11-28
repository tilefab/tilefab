//! Backend: turn cells to rewrite into ANSI escape sequences.
//!
//! The [`Backend`] is deliberately simple: it knows neither buffers nor viewports. It
//! only writes cells (`draw`), moves and hides the cursor, clears, and scrolls.
//! It is generic over `W: Write` so it can be tested with a `Vec<u8>`.
//!
//! # Minimal output
//!
//! - **`MoveTo` only for jumps**: track the cursor position during `draw`. A
//!   cell written immediately after the previous one (same row, next column) needs no
//!   cursor movement.
//! - **Style only when it changes**: also track the current style (colors, modifiers).
//!   Adding a modifier costs only its own sequence. Removing one requires a
//!   full reset (`ESC[0m`) followed by reapplying the cell's style: this is simpler and
//!   safer than cancellation sequences (`ESC[22m` disables both bold and dim,
//!   for example).
//! - **Reset style at the end of `draw`**: outside `draw`, the terminal always uses the
//!   default style. Other writes (line feeds, clearing) therefore behave predictably.
//!
//! # Synchronized updates
//!
//! [`Backend::begin_sync`] and [`Backend::end_sync`] surround a group of writes that the
//! terminal displays **all at once** (private mode 2026). Terminals that do not support it
//! ignore the sequence. The caller (`Terminal`) places them: one block per frame. The
//! opening is written with the first write of the group: a group without writes (a frame
//! that changes nothing) writes nothing at all.
//!
//! # Buffered output
//!
//! To keep the backend from writing to the terminal before `flush`, provide a
//! `BufWriter<Stdout>` instead of `Stdout`, so the entire frame is sent in a single `write`.

use std::fmt;
use std::io::{self, Write};

use crossterm::cursor::{Hide, MoveTo, RestorePosition, SavePosition, Show};
use crossterm::queue;
use crossterm::style::{Color as CtColor, Print, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{
    BeginSynchronizedUpdate, Clear, ClearType as CtClearType, EndSynchronizedUpdate, ScrollDown,
    ScrollUp,
};

use tilefab_core::buffer::Cell;
use tilefab_core::convert::saturating_u16;
use tilefab_core::layout::{Position, Size};
use tilefab_core::style::{Color, Modifier};

/// What `clear` erases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// New variants may come in minor versions: `match` on it needs a `_` arm.
#[non_exhaustive]
pub enum ClearType {
    /// The entire visible screen (`ESC[2J`).
    All,
    /// From the cursor to the bottom of the screen (`ESC[J`).
    AfterCursor,
    /// The entire cursor line (`ESC[2K`).
    CurrentLine,
}

/// Set the scroll region: rows `top` through `bottom`, inclusive (zero-based).
///
/// Crossterm does not provide this command, hence this small type implementing
/// `crossterm::Command`. The sequence is DECSTBM: `ESC [ top ; bottom r`, with
/// **one-based** row numbers. One side effect: the cursor returns to the top-left
/// of the screen. Always reposition the cursor afterward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SetScrollRegion {
    /// First row of the region (zero-based, inclusive).
    pub top: u16,
    /// Last row of the region (zero-based, inclusive).
    pub bottom: u16,
}

impl crossterm::Command for SetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        // `ESC [` (CSI), then `top ; bottom`, then `r`: “set top and bottom margins”.
        write!(
            f,
            "\x1b[{};{}r",
            self.top.saturating_add(1),
            self.bottom.saturating_add(1)
        )
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other(
            "SetScrollRegion is only available through ANSI sequences",
        ))
    }
}

/// Restore the default scroll region (the entire screen).
///
/// DECSTBM sequence without parameters: `ESC [ r`. Like `SetScrollRegion`, it moves the cursor
/// to the top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResetScrollRegion;

impl crossterm::Command for ResetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str("\x1b[r")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other(
            "ResetScrollRegion is only available through ANSI sequences",
        ))
    }
}

/// The modifiers in [`Modifier`] and the SGR code that enables each one.
const ATTRIBUTES: [(Modifier, u8); 9] = [
    (Modifier::BOLD, 1),
    (Modifier::DIM, 2),
    (Modifier::ITALIC, 3),
    (Modifier::UNDERLINED, 4),
    (Modifier::SLOW_BLINK, 5),
    (Modifier::RAPID_BLINK, 6),
    (Modifier::REVERSED, 7),
    (Modifier::HIDDEN, 8),
    (Modifier::CROSSED_OUT, 9),
];

/// SGR code that resets every attribute and color.
const SGR_RESET: u8 = 0;

/// Select a graphic rendition: `ESC [ code m` (SGR), with one numeric code.
///
/// crossterm's `SetAttribute` writes the same sequence, but formats the code through a
/// `String` (`Attribute::sgr`): a frame that changes a modifier would allocate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SetGraphicRendition(u8);

impl crossterm::Command for SetGraphicRendition {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        // `ESC [` (CSI), the code, then `m`: “select graphic rendition”.
        write!(f, "\x1b[{}m", self.0)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other(
            "SetGraphicRendition is only available through ANSI sequences",
        ))
    }
}

/// Convert a library color to a crossterm color.
///
/// Crossterm's names reverse the usual convention: its `Red` is the *bright* version and
/// `DarkRed` the normal version. Our `Red` (ANSI color 1) therefore becomes `DarkRed`, and our
/// `LightRed` (color 9) becomes `Red`.
fn to_crossterm(color: Color) -> CtColor {
    match color {
        Color::Reset => CtColor::Reset,
        Color::Black => CtColor::Black,
        Color::Red => CtColor::DarkRed,
        Color::Green => CtColor::DarkGreen,
        Color::Yellow => CtColor::DarkYellow,
        Color::Blue => CtColor::DarkBlue,
        Color::Magenta => CtColor::DarkMagenta,
        Color::Cyan => CtColor::DarkCyan,
        Color::Gray => CtColor::Grey,
        Color::DarkGray => CtColor::DarkGrey,
        Color::LightRed => CtColor::Red,
        Color::LightGreen => CtColor::Green,
        Color::LightYellow => CtColor::Yellow,
        Color::LightBlue => CtColor::Blue,
        Color::LightMagenta => CtColor::Magenta,
        Color::LightCyan => CtColor::Cyan,
        Color::White => CtColor::White,
        Color::Indexed(i) => CtColor::AnsiValue(i),
        Color::Rgb(r, g, b) => CtColor::Rgb { r, g, b },
    }
}

/// Current terminal style, as tracked by the backend during `draw`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pen {
    fg: Color,
    bg: Color,
    modifier: Modifier,
}

impl Pen {
    /// The terminal style after `ESC[0m`.
    const DEFAULT: Pen = Pen {
        fg: Color::Reset,
        bg: Color::Reset,
        modifier: Modifier::empty(),
    };
}

/// Where the screen size and cursor position come from.
///
/// These values cannot be written: they must be *queried* from the real terminal. For
/// tests (writing to a `Vec<u8>`), set them manually.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// The real terminal: `crossterm::terminal::size()` and `crossterm::cursor::position()`.
    Terminal,
    /// Fixed values, adjustable with `set_fixed_size` and `set_fixed_cursor`. A `None` cursor
    /// simulates a terminal that does not reply to the position query.
    #[cfg(any(test, feature = "testing"))]
    Fixed {
        size: Size,
        cursor: Option<Position>,
    },
}

/// Write cells and cursor commands to a `W: Write`.
#[derive(Debug)]
pub struct Backend<W: Write> {
    writer: W,
    source: Source,
    /// Where the synchronized update stands (see [`Backend::begin_sync`]).
    sync: SyncState,
}

/// The state of the synchronized update of a [`Backend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncState {
    /// No update begun.
    Off,
    /// Begun, but nothing written yet: the opening waits for the first write.
    Requested,
    /// Opening written, closing not yet.
    Open,
}

impl<W: Write> Backend<W> {
    /// Backend connected to the real terminal: size and cursor position are queried from the
    /// system.
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            source: Source::Terminal,
            sync: SyncState::Off,
        }
    }

    /// Test backend: fixed size and cursor position, no terminal required.
    ///
    /// With the `testing` feature only, like the other methods for tests
    /// (`set_fixed_size`, `set_fixed_cursor`, `fail_cursor_queries`, `writer_mut`).
    #[cfg(any(test, feature = "testing"))]
    pub fn with_fixed(writer: W, size: Size, cursor: Position) -> Self {
        Self {
            writer,
            source: Source::Fixed {
                size,
                cursor: Some(cursor),
            },
            sync: SyncState::Off,
        }
    }

    /// Change the simulated size (no effect on a backend connected to the real terminal).
    #[cfg(any(test, feature = "testing"))]
    pub fn set_fixed_size(&mut self, new_size: Size) {
        if let Source::Fixed { size, .. } = &mut self.source {
            *size = new_size;
        }
    }

    /// Change the simulated cursor position (no effect on a real terminal).
    #[cfg(any(test, feature = "testing"))]
    pub fn set_fixed_cursor(&mut self, new_cursor: Position) {
        if let Source::Fixed { cursor, .. } = &mut self.source {
            *cursor = Some(new_cursor);
        }
    }

    /// Make the simulated cursor query fail, as with a terminal that does not reply in time
    /// (no effect on a real terminal). `set_fixed_cursor` makes it succeed again.
    #[cfg(any(test, feature = "testing"))]
    pub fn fail_cursor_queries(&mut self) {
        if let Source::Fixed { cursor, .. } = &mut self.source {
            *cursor = None;
        }
    }

    /// Screen size in cells.
    ///
    /// # Errors
    ///
    /// Returns an error if the terminal size cannot be read.
    pub fn size(&self) -> io::Result<Size> {
        match self.source {
            Source::Terminal => {
                let (width, height) = crossterm::terminal::size()?;
                Ok(Size::new(width, height))
            }
            #[cfg(any(test, feature = "testing"))]
            Source::Fixed { size, .. } => Ok(size),
        }
    }

    /// Cursor position on the screen.
    ///
    /// On a real terminal, this is a **query**: write `ESC[6n` and the terminal replies with
    /// `ESC[row;columnR` on input. Crossterm reads this reply using the same reader as
    /// keystrokes, so no other thread should be blocked in `event::read()` (see the guide of
    /// the `tilefab` crate, "Why `poll` before `read`"); the query times out after 2 seconds if
    /// the terminal does not reply.
    ///
    /// # Errors
    ///
    /// Returns an error if the query cannot be sent or the terminal does not reply in time.
    pub fn cursor_position(&mut self) -> io::Result<Position> {
        match self.source {
            Source::Terminal => {
                let (x, y) = crossterm::cursor::position()?;
                Ok(Position::new(x, y))
            }
            #[cfg(any(test, feature = "testing"))]
            Source::Fixed { cursor, .. } => cursor.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "simulated terminal did not report the cursor position",
                )
            }),
        }
    }

    /// Read access to the underlying stream (tests read emitted bytes here).
    pub fn writer(&self) -> &W {
        &self.writer
    }

    /// Write access to the underlying stream (tests take the emitted bytes here). With the
    /// `testing` feature only: bytes written there bypass the synchronized update and the
    /// state the terminal keeps of the screen.
    #[cfg(any(test, feature = "testing"))]
    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    /// Write the given cells, `(column, row, cell)`, in the order received.
    ///
    /// Supplying cells in screen order, as `Buffer::diff` does, is most efficient.
    /// Continuation cells are ignored (the wide character's head is enough).
    /// A cell never holds a control character (see [`Cell`]): no escape sequence can
    /// come out of its symbol.
    /// At the end, reset the style if necessary.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the underlying stream fails.
    pub fn draw<'a>(
        &mut self,
        content: impl IntoIterator<Item = (u16, u16, &'a Cell)>,
    ) -> io::Result<()> {
        let mut pen = Pen::DEFAULT;
        // Cursor position after the last write; `None` = unknown.
        let mut next: Option<(u16, u16)> = None;

        for (x, y, cell) in content {
            if cell.is_continuation() {
                continue;
            }
            // Move only if the cell is not immediately after the previous one.
            if next != Some((x, y)) {
                queue!(self.out()?, MoveTo(x, y))?;
            }
            self.apply_style(&mut pen, cell)?;
            // Never a raw control character: it could be an escape sequence.
            queue!(self.out()?, Print(cell.symbol()))?;
            // The cursor advances by the symbol's width (2 for a wide character).
            next = Some((x.saturating_add(saturating_u16(cell.width().max(1))), y));
        }

        if pen != Pen::DEFAULT {
            queue!(self.out()?, SetGraphicRendition(SGR_RESET))?;
        }
        Ok(())
    }

    /// Bring the terminal style (`pen`) to `cell`'s style with as few sequences as possible.
    fn apply_style(&mut self, pen: &mut Pen, cell: &Cell) -> io::Result<()> {
        if cell.modifier != pen.modifier {
            // A modifier needs removing: reset fully, then start from the default style.
            if !(pen.modifier - cell.modifier).is_empty() {
                queue!(self.out()?, SetGraphicRendition(SGR_RESET))?;
                *pen = Pen::DEFAULT;
            }
            // Only modifiers to add remain.
            for (flag, code) in ATTRIBUTES {
                if cell.modifier.contains(flag) && !pen.modifier.contains(flag) {
                    queue!(self.out()?, SetGraphicRendition(code))?;
                }
            }
            pen.modifier = cell.modifier;
        }
        if cell.fg != pen.fg {
            queue!(self.out()?, SetForegroundColor(to_crossterm(cell.fg)))?;
            pen.fg = cell.fg;
        }
        if cell.bg != pen.bg {
            queue!(self.out()?, SetBackgroundColor(to_crossterm(cell.bg)))?;
            pen.bg = cell.bg;
        }
        Ok(())
    }

    /// Begin a synchronized update: the terminal keeps the display unchanged until `end_sync`.
    ///
    /// The opening (`ESC[?2026h`) is written just before the first write that follows, and
    /// `end_sync` closes only an update that was opened: without writes in between, nothing
    /// is written (an idle application that draws unchanged frames sends no bytes).
    ///
    /// # Errors
    ///
    /// Never fails: the signature is that of the other writes.
    pub fn begin_sync(&mut self) -> io::Result<()> {
        if self.sync == SyncState::Off {
            self.sync = SyncState::Requested;
        }
        Ok(())
    }

    /// End the synchronized update: the terminal displays everything at once.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the underlying stream fails.
    pub fn end_sync(&mut self) -> io::Result<()> {
        if self.sync == SyncState::Open {
            queue!(self.writer, EndSynchronizedUpdate)?;
        }
        self.sync = SyncState::Off;
        Ok(())
    }

    /// The writer, after the opening of a synchronized update begun and not written yet.
    fn out(&mut self) -> io::Result<&mut W> {
        if self.sync == SyncState::Requested {
            // Set first: even a partly written opening must be closed.
            self.sync = SyncState::Open;
            queue!(self.writer, BeginSynchronizedUpdate)?;
        }
        Ok(&mut self.writer)
    }

    /// Close a frame interrupted by an error or a panic, on a best-effort basis (errors are
    /// ignored): reset the scroll region, which an interrupted `scroll_region_up` may leave
    /// set, then end the synchronized update, so that the terminal does not keep its display
    /// frozen. `ESC[r` moves the cursor to the top-left corner: it is saved before (`ESC 7`)
    /// and restored after (`ESC 8`). Nothing is written when no update is open.
    pub fn abort_sync(&mut self) {
        if std::mem::replace(&mut self.sync, SyncState::Off) == SyncState::Open {
            let _ = queue!(
                self.writer,
                SavePosition,
                ResetScrollRegion,
                RestorePosition,
                EndSynchronizedUpdate
            );
            let _ = self.writer.flush();
        }
    }

    /// Hide the terminal cursor.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the underlying stream fails.
    pub fn hide_cursor(&mut self) -> io::Result<()> {
        queue!(self.out()?, Hide)
    }

    /// Show the terminal cursor.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the underlying stream fails.
    pub fn show_cursor(&mut self) -> io::Result<()> {
        queue!(self.out()?, Show)
    }

    /// Position the cursor (column `x`, row `y`, zero-based screen coordinates).
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the underlying stream fails.
    pub fn set_cursor_position(&mut self, x: u16, y: u16) -> io::Result<()> {
        queue!(self.out()?, MoveTo(x, y))
    }

    /// Clear part of the screen. Does not move the cursor.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the underlying stream fails.
    pub fn clear(&mut self, what: ClearType) -> io::Result<()> {
        let what = match what {
            ClearType::All => CtClearType::All,
            ClearType::AfterCursor => CtClearType::FromCursorDown,
            ClearType::CurrentLine => CtClearType::CurrentLine,
        };
        queue!(self.out()?, Clear(what))
    }

    /// Move the cursor down `n` rows; at the bottom of the screen, content scrolls upward.
    ///
    /// Write plain line feeds (`\n`). In raw mode, the terminal does not
    /// convert them to carriage returns: the cursor column stays unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the underlying stream fails.
    pub fn append_lines(&mut self, n: u16) -> io::Result<()> {
        for _ in 0..n {
            self.out()?.write_all(b"\n")?;
        }
        Ok(())
    }

    /// Scroll the area `top..=bottom` upward by `n` rows (zero-based screen rows).
    ///
    /// The first `n` rows enter scrollback **only in some terminals, and only if `top` is 0**
    /// (xterm does it); many terminals (xterm.js in VS Code, iTerm2 by default…) delete them
    /// in every case, as when `top` is not 0. To keep them, scroll the whole screen with line
    /// feeds on its last row instead (`append_lines`). The last `n` rows of the region are
    /// cleared. Three sequences: set the region, `CSI n S`, restore the region. The cursor
    /// ends up at the top-left: reposition it.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the underlying stream fails.
    pub fn scroll_region_up(&mut self, top: u16, bottom: u16, n: u16) -> io::Result<()> {
        if n == 0 {
            return Ok(());
        }
        queue!(
            self.out()?,
            SetScrollRegion { top, bottom },
            ScrollUp(n),
            ResetScrollRegion
        )
    }

    /// Scroll the area `top..=bottom` downward by `n` rows (`CSI n T`).
    /// The same remarks as for [`Backend::scroll_region_up`] apply.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the underlying stream fails.
    pub fn scroll_region_down(&mut self, top: u16, bottom: u16, n: u16) -> io::Result<()> {
        if n == 0 {
            return Ok(());
        }
        queue!(
            self.out()?,
            SetScrollRegion { top, bottom },
            ScrollDown(n),
            ResetScrollRegion
        )
    }

    /// Flush the write buffer: this is where bytes are sent to the terminal.
    ///
    /// # Errors
    ///
    /// Returns an error if flushing the underlying stream fails.
    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tilefab_core::{Buffer, Rect, Style};

    /// Bytes emitted by the backend, as readable text.
    fn out(backend: &Backend<Vec<u8>>) -> String {
        String::from_utf8(backend.writer().clone()).unwrap()
    }

    fn backend() -> Backend<Vec<u8>> {
        Backend::new(Vec::new())
    }

    /// Draw all of `buf` (by diffing against an empty buffer).
    fn draw_buffer(backend: &mut Backend<Vec<u8>>, buf: &Buffer) {
        let blank = Buffer::empty(buf.area());
        backend.draw(buf.diff(&blank)).unwrap();
    }

    #[test]
    fn empty_diff_emits_nothing() {
        let mut b = backend();
        b.draw(std::iter::empty()).unwrap();
        assert_eq!(out(&b), "");
    }

    #[test]
    fn an_empty_synchronized_update_writes_nothing() {
        let mut b = backend();
        b.begin_sync().unwrap();
        b.draw(std::iter::empty()).unwrap();
        b.end_sync().unwrap();
        assert_eq!(out(&b), "");
        b.abort_sync();
        assert_eq!(out(&b), "", "nothing to close");
    }

    #[test]
    fn the_opening_comes_with_the_first_write() {
        let mut b = backend();
        b.begin_sync().unwrap();
        b.set_cursor_position(0, 0).unwrap();
        b.append_lines(1).unwrap();
        b.end_sync().unwrap();
        // Private mode 2026: `ESC[?2026h` opens, `ESC[?2026l` closes; one of each.
        assert_eq!(out(&b), "\x1b[?2026h\x1b[1;1H\n\x1b[?2026l");
    }

    #[test]
    fn adjacent_cells_with_same_style_need_one_move_and_no_style() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
        buf.set_string(2, 1, "abc", Style::new());
        let mut b = backend();
        draw_buffer(&mut b, &buf);
        // A single MoveTo (row 2, column 3, one-based), then the three letters.
        assert_eq!(out(&b), "\x1b[2;3Habc");
    }

    #[test]
    fn same_colored_cells_set_the_color_once_and_reset_at_the_end() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        buf.set_string(0, 0, "abc", Style::new().fg(Color::Rgb(1, 2, 3)));
        let mut b = backend();
        draw_buffer(&mut b, &buf);
        assert_eq!(out(&b), "\x1b[1;1H\x1b[38;2;1;2;3mabc\x1b[0m");
    }

    #[test]
    fn jump_emits_a_new_move() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        buf.set_string(0, 0, "a", Style::new());
        buf.set_string(5, 2, "b", Style::new());
        let mut b = backend();
        draw_buffer(&mut b, &buf);
        assert_eq!(out(&b), "\x1b[1;1Ha\x1b[3;6Hb");
    }

    #[test]
    fn gap_between_cells_on_the_same_row_emits_a_move() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        buf.set_string(0, 0, "a", Style::new());
        buf.set_string(2, 0, "b", Style::new());
        let mut b = backend();
        draw_buffer(&mut b, &buf);
        assert_eq!(out(&b), "\x1b[1;1Ha\x1b[1;3Hb");
    }

    #[test]
    fn wide_character_advances_the_cursor_by_two() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        buf.set_string(0, 0, "日x", Style::new());
        let mut b = backend();
        draw_buffer(&mut b, &buf);
        // The `x` is in column 2: immediately after the two columns of “日”, so no MoveTo.
        assert_eq!(out(&b), "\x1b[1;1H日x");
    }

    #[test]
    fn style_changes_only_what_differs() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        buf.set_string(0, 0, "a", Style::new().fg(Color::Rgb(1, 2, 3)));
        buf.set_string(
            1,
            0,
            "b",
            Style::new().fg(Color::Rgb(1, 2, 3)).bg(Color::Rgb(4, 5, 6)),
        );
        let mut b = backend();
        draw_buffer(&mut b, &buf);
        // The second character changes only the background.
        assert_eq!(
            out(&b),
            "\x1b[1;1H\x1b[38;2;1;2;3ma\x1b[48;2;4;5;6mb\x1b[0m"
        );
    }

    #[test]
    fn adding_a_modifier_is_incremental_and_removing_one_resets_everything() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        buf.set_string(0, 0, "a", Style::new().add_modifier(Modifier::BOLD));
        buf.set_string(
            1,
            0,
            "b",
            Style::new().add_modifier(Modifier::BOLD | Modifier::ITALIC),
        );
        buf.set_string(
            2,
            0,
            "c",
            Style::new()
                .add_modifier(Modifier::ITALIC)
                .fg(Color::Rgb(9, 9, 9)),
        );
        let mut b = backend();
        draw_buffer(&mut b, &buf);
        assert_eq!(
            out(&b),
            // a: bold. b: add only italic (SGR 3).
            // c: bold is removed → reset (SGR 0), then reapply italic and color.
            "\x1b[1;1H\x1b[1ma\x1b[3mb\x1b[0m\x1b[3m\x1b[38;2;9;9;9mc\x1b[0m"
        );
    }

    #[test]
    fn default_style_cells_never_emit_style_sequences() {
        let buf = Buffer::with_lines(["xy"]);
        let mut b = backend();
        draw_buffer(&mut b, &buf);
        assert_eq!(out(&b), "\x1b[1;1Hxy");
    }

    #[test]
    fn named_colors_use_the_expected_ansi_codes() {
        // Our `Red` is the normal color (palette 1), `LightRed` the bright one (palette 9).
        assert_eq!(to_crossterm(Color::Red), CtColor::DarkRed);
        assert_eq!(to_crossterm(Color::LightRed), CtColor::Red);
        assert_eq!(to_crossterm(Color::Gray), CtColor::Grey);
        assert_eq!(to_crossterm(Color::DarkGray), CtColor::DarkGrey);
        assert_eq!(to_crossterm(Color::Indexed(7)), CtColor::AnsiValue(7));
    }

    #[test]
    fn continuation_cells_are_skipped() {
        // Only a wide character creates a continuation, right after it.
        let mut wide = Buffer::empty(Rect::new(0, 0, 2, 1));
        wide.set_string(0, 0, "日", Style::new());
        let cont = wide.get(1, 0);
        assert!(cont.is_continuation());
        let mut b = backend();
        b.draw([(1, 0, cont)]).unwrap();
        assert_eq!(out(&b), "");
    }

    #[test]
    fn symbols_with_control_characters_are_printed_as_spaces() {
        // A raw escape sequence given as a symbol never reaches the terminal
        // (`Cell::new` already replaces it with a space).
        let title = Cell::new("\x1b]0;pwned\x07");
        let clear = Cell::new("\x1b[2J");
        let next = Cell::new("z");
        let mut b = backend();
        b.draw([(0, 0, &title), (1, 0, &clear), (2, 0, &next)])
            .unwrap();
        // One move, then three adjacent single-column cells: no other move.
        assert_eq!(out(&b), "\x1b[1;1H  z");
    }

    #[test]
    fn cursor_commands() {
        let mut b = backend();
        b.hide_cursor().unwrap();
        b.show_cursor().unwrap();
        b.set_cursor_position(4, 7).unwrap();
        assert_eq!(out(&b), "\x1b[?25l\x1b[?25h\x1b[8;5H");
    }

    #[test]
    fn clear_commands() {
        let mut b = backend();
        b.clear(ClearType::All).unwrap();
        b.clear(ClearType::AfterCursor).unwrap();
        b.clear(ClearType::CurrentLine).unwrap();
        assert_eq!(out(&b), "\x1b[2J\x1b[J\x1b[2K");
    }

    #[test]
    fn append_lines_writes_line_feeds() {
        let mut b = backend();
        b.append_lines(3).unwrap();
        assert_eq!(out(&b), "\n\n\n");
        let mut b = backend();
        b.append_lines(0).unwrap();
        assert_eq!(out(&b), "");
    }

    #[test]
    fn scroll_region_sequences_are_exact() {
        // Region 3..=9 (zero-based) → “ESC[4;10r” (one-based); “ESC[2S” scrolls by 2; “ESC[r” restores.
        let mut b = backend();
        b.scroll_region_up(3, 9, 2).unwrap();
        assert_eq!(out(&b), "\x1b[4;10r\x1b[2S\x1b[r");

        let mut b = backend();
        b.scroll_region_down(0, 4, 1).unwrap();
        assert_eq!(out(&b), "\x1b[1;5r\x1b[1T\x1b[r");
    }

    #[test]
    fn scrolling_zero_lines_emits_nothing() {
        let mut b = backend();
        b.scroll_region_up(0, 4, 0).unwrap();
        b.scroll_region_down(0, 4, 0).unwrap();
        assert_eq!(out(&b), "");
    }

    #[test]
    fn fixed_backend_reports_the_simulated_size_and_cursor() {
        let mut b = Backend::with_fixed(Vec::new(), Size::new(80, 24), Position::new(3, 5));
        assert_eq!(b.size().unwrap(), Size::new(80, 24));
        assert_eq!(b.cursor_position().unwrap(), Position::new(3, 5));
        b.set_fixed_size(Size::new(10, 4));
        b.set_fixed_cursor(Position::new(0, 1));
        assert_eq!(b.size().unwrap(), Size::new(10, 4));
        assert_eq!(b.cursor_position().unwrap(), Position::new(0, 1));
        b.fail_cursor_queries();
        assert_eq!(
            b.cursor_position().unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        b.set_fixed_cursor(Position::new(2, 3));
        assert_eq!(b.cursor_position().unwrap(), Position::new(2, 3));
        // Querying the cursor writes nothing: the query exists only on a real terminal.
        assert_eq!(b.writer(), b"");
    }

    #[test]
    fn flush_reaches_the_writer() {
        // `BufWriter` sends nothing before `flush`: check that the backend forwards it.
        let mut b = Backend::new(io::BufWriter::new(Vec::new()));
        b.hide_cursor().unwrap();
        assert_eq!(b.writer().get_ref(), b"");
        b.flush().unwrap();
        assert_eq!(b.writer().get_ref().as_slice(), b"\x1b[?25l");
    }
}
