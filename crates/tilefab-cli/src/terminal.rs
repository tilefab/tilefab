//! `Terminal` and `Frame`: double buffering, the rendering loop, and the inline viewport.
//!
//! # Principle
//!
//! `Terminal` keeps the `previous` buffer (what the terminal already displays in the viewport)
//! and renders each `draw` into a second one. At the end of each `draw`, compute the difference
//! (`Buffer::diff`), send only those cells to the backend, and make the new buffer
//! `previous`. All rendering for a frame is wrapped in **a single** synchronized update. The
//! buffers are kept from one frame to the next: once warmed up, a frame allocates nothing.
//!
//! # Two viewports
//!
//! - **Fullscreen**: the viewport covers the entire screen, often on the alternate screen.
//! - **Inline**: the viewport is a band of lines on the normal screen, anchored at the bottom.
//!   Its height can change (`set_height`) and lines can be inserted above it (`insert_before`).
//!   Neither call writes anything: `draw` applies **everything** (scrolling, lines, new
//!   height, redraw) in a single synchronized block. See [`super::viewport`] for the placement
//!   rule.
//!
//! # Resizing
//!
//! At the start of each `draw`, compare the terminal size with the known size.
//!
//! - Fullscreen: resize the buffers, clear the screen, and redraw everything.
//! - Inline: query the cursor position (`ESC[6n`), reposition the viewport while preserving
//!   the cursor offset within it, then clear from the viewport top to the screen bottom
//!   (`ESC[J`) and redraw everything. **Known limitation**: Terminal.app and iTerm2 reflow
//!   existing lines when the window shrinks, which may leave copies of the old
//!   viewport above it. This is the same strategy as ratatui.
//!   If the terminal does not answer the cursor query (slow SSH link, multiplexer), the
//!   position is estimated instead of failing: the viewport keeps its row, moved up just
//!   enough to stay on screen (anchored at the bottom) when the screen shrinks. A warning is
//!   logged.
//!
//! # Limitation
//!
//! If the viewport fills the entire screen (`height == terminal height`), there is no room
//! for inserted lines: they are ignored.
//!
//! A screen with no columns (a pty whose size is not set reports a width of 0) cannot show
//! inserted lines either, but this is transient: lines already queued stay pending and are
//! applied on the first `draw` where the screen has columns again. `insert_before` itself
//! ignores lines while the known width is 0 (there is no buffer to render them into): the
//! caller keeps them until then (see `Terminal::insert_before`).
//!
//! A screen with no rows is transient too: while its height is 0, `draw` draws nothing
//! (the render closure is not called) and keeps the pending lines; the first `draw` with
//! rows again repositions the viewport and applies them.

use std::io::{self, Write};

use super::backend::{Backend, ClearType};
use super::viewport::{ScrollMode, TerminalOptions, Viewport, plan_commit};
use tilefab_core::buffer::{Buffer, Cell};
use tilefab_core::convert::saturating_u16;
use tilefab_core::layout::{Position, Rect, Size};
use tilefab_core::widget::{StatefulWidget, Widget};

/// View of the current buffer passed to the rendering function of `Terminal::draw`.
#[derive(Debug)]
pub struct Frame<'a> {
    area: Rect,
    buffer: &'a mut Buffer,
    cursor_position: Option<Position>,
}

impl Frame<'_> {
    /// Drawable area: the entire terminal in fullscreen mode, the viewport band in inline mode.
    /// It may change between frames (resizing, `set_height`).
    #[must_use]
    pub fn area(&self) -> Rect {
        self.area
    }

    /// Draw a widget in `area`.
    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    /// Draw a stateful widget in `area`.
    pub fn render_stateful_widget<W: StatefulWidget>(
        &mut self,
        widget: W,
        area: Rect,
        state: &mut W::State,
    ) {
        widget.render(area, self.buffer, state);
    }

    /// Show the terminal cursor at this position (screen coordinates) for this frame.
    /// Without a call, the cursor is hidden.
    pub fn set_cursor_position(&mut self, position: impl Into<Position>) {
        self.cursor_position = Some(position.into());
    }

    /// Direct buffer access for drawing without a widget.
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffer
    }
}

/// The terminal: a backend, the displayed buffer, and viewport state.
#[derive(Debug)]
pub struct Terminal<W: Write> {
    backend: Backend<W>,
    viewport: Viewport,
    scroll_mode: ScrollMode,
    /// Screen size known at the last `draw`.
    screen: Size,
    /// Viewport area in screen coordinates. The entire screen in fullscreen mode.
    area: Rect,
    /// What the terminal displays in `area`.
    previous: Buffer,
    /// Cursor position displayed by the last frame, or `None` if hidden.
    cursor: Option<Position>,
    /// Approximate row of the real cursor: used to reposition the viewport after
    /// resizing.
    cursor_row: u16,
    /// Requested inline viewport height (applied on the next `draw`).
    wanted_height: u16,
    /// Lines to insert above the viewport on the next `draw`, one after the other, each
    /// `pending_width` cells wide (the screen width when they were queued).
    pending: Vec<Cell>,
    pending_width: usize,
    /// Buffers kept from one frame to the next, so that a frame allocates nothing once they
    /// have the right size (see `Buffer::reset_to`): `next` is the one the frame renders
    /// into (in fullscreen mode, swapped with `previous` afterwards); `model` and `target`
    /// are the screen before and after an inline commit; `insert` receives `insert_before`.
    next: Buffer,
    model: Buffer,
    target: Buffer,
    insert: Buffer,
}

impl<W: Write> Terminal<W> {
    /// Fullscreen terminal (the caller must have enabled the alternate screen and raw mode:
    /// see `TerminalGuard`).
    ///
    /// # Errors
    ///
    /// Same as [`Terminal::with_options`].
    pub fn new(backend: Backend<W>) -> io::Result<Self> {
        Self::with_options(backend, TerminalOptions::default())
    }

    /// Terminal with a chosen viewport.
    ///
    /// In inline mode, **query the cursor position once**, reserve the viewport lines
    /// (scrolling the screen if too close to the bottom), and clear them. Then track the
    /// position ourselves. A nonzero starting column (the shell left a prompt) makes
    /// the viewport start on the next line.
    ///
    /// # Errors
    ///
    /// Returns an error if the screen size cannot be read, the cursor position query fails (inline mode), or writing to the terminal fails.
    pub fn with_options(mut backend: Backend<W>, options: TerminalOptions) -> io::Result<Self> {
        let screen = backend.size()?;
        match options.viewport {
            Viewport::Fullscreen => {
                let area = Rect::new(0, 0, screen.width, screen.height);
                Ok(Self {
                    backend,
                    viewport: options.viewport,
                    scroll_mode: options.scroll_mode,
                    screen,
                    area,
                    previous: Buffer::empty(area),
                    cursor: None,
                    cursor_row: 0,
                    wanted_height: screen.height,
                    pending: Vec::new(),
                    pending_width: 0,
                    next: Buffer::empty(Rect::default()),
                    model: Buffer::empty(Rect::default()),
                    target: Buffer::empty(Rect::default()),
                    insert: Buffer::empty(Rect::default()),
                })
            }
            Viewport::Inline(height) => {
                let cursor = backend.cursor_position()?;
                let height = height.clamp(1, screen.height.max(1));
                // Rows to move down so the viewport fits: one to leave the prompt line
                // if not in column 0, then `height - 1` for the remaining rows.
                let extra = u16::from(cursor.x > 0);
                let append = extra + height - 1;
                // At the screen bottom, each line feed scrolls: `scroll` rows beyond
                // the available space below the cursor.
                let scroll = cursor
                    .y
                    .saturating_add(append)
                    .saturating_sub(screen.height.saturating_sub(1));
                let top = cursor.y + extra - scroll;

                backend.begin_sync()?;
                backend.append_lines(append)?;
                // Reserved lines may contain anything: clear them one by one
                // (never `ESC[J`, which would also clear below the viewport).
                for y in top..top + height {
                    backend.set_cursor_position(0, y)?;
                    backend.clear(ClearType::CurrentLine)?;
                }
                backend.set_cursor_position(0, top)?;
                backend.end_sync()?;
                backend.flush()?;

                let area = Rect::new(0, top, screen.width, height);
                Ok(Self {
                    backend,
                    viewport: options.viewport,
                    scroll_mode: options.scroll_mode,
                    screen,
                    area,
                    previous: Buffer::empty(area),
                    cursor: None,
                    cursor_row: top,
                    wanted_height: height,
                    pending: Vec::new(),
                    pending_width: 0,
                    next: Buffer::empty(Rect::default()),
                    model: Buffer::empty(Rect::default()),
                    target: Buffer::empty(Rect::default()),
                    insert: Buffer::empty(Rect::default()),
                })
            }
        }
    }

    /// The backend (tests read emitted bytes here).
    pub fn backend(&self) -> &Backend<W> {
        &self.backend
    }

    /// Mutable access to the backend.
    pub fn backend_mut(&mut self) -> &mut Backend<W> {
        &mut self.backend
    }

    /// Viewport area in screen coordinates, as it was in the last frame.
    pub fn area(&self) -> Rect {
        self.area
    }

    /// What the terminal displays in the viewport (the last frame's result). Useful for
    /// testing rendering without a terminal.
    pub fn buffer(&self) -> &Buffer {
        &self.previous
    }

    /// Request a new inline viewport height, clamped between 1 and the terminal
    /// height. Writes nothing: the change is applied on the next `draw`, along with
    /// inserted lines and the redraw. No effect in fullscreen mode.
    pub fn set_height(&mut self, height: u16) {
        if matches!(self.viewport, Viewport::Inline(_)) {
            self.wanted_height = height.clamp(1, self.screen.height.max(1));
        }
    }

    /// Queue lines for insertion above the inline viewport. `render` fills a buffer of
    /// `height` rows and screen width. Writes nothing: applied on the next `draw`.
    /// The screen width is the one known at the last `draw`, i.e. `self.area().width`:
    /// use that width (not the current backend size) to compute `height` for wrapped text.
    /// Lines are stored at that width: if the screen width changes before they are applied,
    /// they are truncated or padded to the new width.
    /// No effect in fullscreen mode, nor while that width is 0: keep the lines and insert them
    /// once `self.area().width` is positive again.
    pub fn insert_before(&mut self, height: u16, render: impl FnOnce(&mut Buffer)) {
        let width = usize::from(self.screen.width);
        if !matches!(self.viewport, Viewport::Inline(_)) || height == 0 || width == 0 {
            return;
        }
        self.insert
            .reset_to(Rect::new(0, 0, self.screen.width, height));
        render(&mut self.insert);
        // Between two draws the known width does not change, and lines kept pending at a
        // zero width are applied before any new one can be queued: all pending lines
        // share one width.
        debug_assert!(self.pending.is_empty() || self.pending_width == width);
        self.pending.extend_from_slice(&self.insert.content);
        self.pending_width = width;
    }

    /// Draw a frame: `render` fills the buffer, then send the difference to the terminal.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the terminal fails.
    pub fn draw(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> io::Result<()> {
        match self.viewport {
            Viewport::Fullscreen => self.draw_fullscreen(render),
            Viewport::Inline(_) => self.draw_inline(render),
        }
    }

    // --- fullscreen ---------------------------------------------------------------------

    fn draw_fullscreen(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> io::Result<()> {
        let size = self.backend.size()?;
        // Start the synchronized block: everything that follows will appear at once.
        self.backend.begin_sync()?;
        if size != self.screen {
            // The terminal may have reflowed or lost lines: start with an empty screen, and the
            // previous (empty) buffer now matches what is displayed.
            self.screen = size;
            self.area = Rect::new(0, 0, size.width, size.height);
            self.previous.reset_to(self.area);
            self.backend.set_cursor_position(0, 0)?;
            self.backend.clear(ClearType::All)?;
        }

        self.next.reset_to(self.area);
        let mut frame = Frame {
            area: self.area,
            buffer: &mut self.next,
            cursor_position: None,
        };
        render(&mut frame);
        let wanted_cursor = frame.cursor_position;

        let mut changed = false;
        self.backend.draw(
            self.next
                .diff_iter(&self.previous)
                .inspect(|_| changed = true),
        )?;
        self.place_cursor(wanted_cursor, changed)?;

        self.backend.end_sync()?;
        self.backend.flush()?;
        // What was just drawn becomes “what the terminal displays”; the old buffer is
        // reused for the next frame (double buffering).
        std::mem::swap(&mut self.previous, &mut self.next);
        Ok(())
    }

    // --- inline --------------------------------------------------------------------------

    fn draw_inline(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> io::Result<()> {
        // Query the terminal (size, cursor position) **before** opening the synchronized
        // block: the reply arrives on input, not in our output stream.
        let size = self.backend.size()?;
        let resized = if size == self.screen {
            None
        } else {
            Some((size, self.cursor_after_resize(size)))
        };

        self.backend.begin_sync()?;
        if let Some((size, cursor)) = resized {
            self.resize_inline(size, cursor)?;
        }
        // A screen with no rows (a pty whose height is set to 0) has no room for the
        // viewport: draw nothing and keep the pending lines until rows come back (the
        // resize at that point repositions the viewport).
        if self.screen.height == 0 {
            self.backend.end_sync()?;
            return self.backend.flush();
        }

        let screen_height = self.screen.height;
        let new_height = self.wanted_height.clamp(1, screen_height.max(1));
        // A zero-width screen (possible after a resize since `insert_before`) cannot show
        // the inserted lines: keep them pending for the first draw with columns again.
        let taken = self.screen.width > 0;
        let mut rows = if taken {
            std::mem::take(&mut self.pending)
        } else {
            Vec::new()
        };
        let row_width = self.pending_width.max(1);
        // How many inserted lines fit on screen with the viewport: apply larger batches
        // in chunks. No space at all: ignore them (see the module header).
        let per_chunk = usize::from(screen_height.saturating_sub(new_height));
        if per_chunk == 0 {
            rows.clear();
        }
        // The last chunk is applied with the frame; the earlier ones, in order, before it.
        let mut chunks = rows.chunks(per_chunk.max(1) * row_width);
        let last = chunks.next_back().unwrap_or(&[]);

        // Did any chunk move the real cursor? (With several chunks, the last one always
        // scrolls and reports a move anyway: `cargo mutants` sees `|=` → `&=` survive, but
        // the accumulation keeps this correct without relying on that.)
        let mut moved = false;
        for chunk in chunks {
            moved |= self
                .commit(chunk, row_width, new_height, None::<fn(&mut Frame<'_>)>)?
                .1;
        }
        let (wanted_cursor, last_moved) = self.commit(last, row_width, new_height, Some(render))?;
        self.place_cursor(wanted_cursor, moved | last_moved)?;
        if taken {
            // Give the (now applied) lines' memory back for the next ones.
            rows.clear();
            self.pending = rows;
        }

        self.backend.end_sync()?;
        self.backend.flush()
    }

    /// Cursor position after a resize: queried from the terminal, or estimated if the query
    /// fails, so that a terminal that does not reply in time does not stop the application.
    fn cursor_after_resize(&mut self, size: Size) -> Position {
        match self.backend.cursor_position() {
            Ok(cursor) => cursor,
            Err(error) => {
                let estimate = self.estimated_cursor(size);
                tracing::warn!(
                    "cursor position query failed after resize ({error}); \
                     estimating row {}",
                    estimate.y
                );
                estimate
            }
        }
    }

    /// Best guess of the cursor position on the resized screen, without querying the terminal:
    /// the last known cursor row, limited to the new screen. When the screen grows, most
    /// terminals add rows at the bottom, so the viewport has not moved; when it shrinks,
    /// `resize_inline` keeps the viewport on screen, anchored at the bottom.
    fn estimated_cursor(&self, size: Size) -> Position {
        Position::new(0, self.cursor_row.min(size.height.saturating_sub(1)))
    }

    /// Reposition the viewport after a screen size change (see the module header).
    fn resize_inline(&mut self, size: Size, cursor: Position) -> io::Result<()> {
        // Where the cursor was within the viewport: the shell repositions it elsewhere itself,
        // but this offset remains the best indication of the viewport's starting row.
        let offset = self
            .cursor_row
            .saturating_sub(self.area.y)
            .min(self.area.height.saturating_sub(1));
        let height = self.wanted_height.clamp(1, size.height.max(1));
        // With no rows, nothing is drawn (see `draw_inline`): keep the requested height for
        // when rows come back instead of collapsing it to 1.
        if size.height > 0 {
            self.wanted_height = height;
        }
        let top = cursor
            .y
            .saturating_sub(offset)
            .min(size.height.saturating_sub(height));

        // `ESC[J` is allowed here: the screen content must be redrawn anyway.
        self.backend.set_cursor_position(0, top)?;
        self.backend.clear(ClearType::AfterCursor)?;

        self.screen = size;
        self.area = Rect::new(0, top, size.width, height);
        self.previous.reset_to(self.area);
        self.cursor_row = top;
        Ok(())
    }

    /// Apply a commit: scroll if necessary, write `rows` above the new viewport and,
    /// if `render` is provided, draw the viewport of `new_height` rows. Return the cursor
    /// position requested by the frame and a flag indicating that the real cursor moved.
    ///
    /// Everything goes through **a single diff**: a model of the screen content after
    /// scrolling, compared with the target (inserted lines + viewport). Only differing
    /// cells are written, whether in the logs or the viewport.
    fn commit<F: FnOnce(&mut Frame<'_>)>(
        &mut self,
        rows: &[Cell],
        row_width: usize,
        new_height: u16,
        render: Option<F>,
    ) -> io::Result<(Option<Position>, bool)> {
        let screen = self.screen;
        let width = usize::from(screen.width);
        let lines = saturating_u16(rows.len().div_ceil(row_width));
        let (top, height) = (self.area.y, self.area.height);
        // Outside the placement preconditions (callers do not commit on a screen with no
        // rows, nor more lines than fit), committing nothing keeps the state consistent.
        let Some(plan) = plan_commit(screen.height, top, height, lines, new_height) else {
            return Ok((None, false));
        };
        let old_bottom = top + height;

        let mut moved = false;
        let mut erase = plan.erase.clone();
        let mut model_is_blank = false;
        if plan.scroll > 0 {
            moved = true;
            if self.scroll_mode == ScrollMode::Region && top >= 2 {
                // Only the area above the viewport scrolls; the viewport stays put.
                self.backend.scroll_region_up(0, top - 1, plan.scroll)?;
            } else {
                // Fallback (or region too small: terminals ignore single-line regions).
                // Clear the old viewport, then scroll the whole screen by writing
                // line feeds at the bottom: the result is the same, since everything below
                // the logs is empty. The viewport will be fully rewritten.
                for y in top..old_bottom {
                    self.backend.set_cursor_position(0, y)?;
                    self.backend.clear(ClearType::CurrentLine)?;
                }
                self.backend
                    .set_cursor_position(0, screen.height.saturating_sub(1))?;
                self.backend.append_lines(plan.scroll)?;
                model_is_blank = true;
                erase = 0..0;
            }
        }

        // The model: screen content from `lo` to the bottom of the new viewport.
        let lo = top - plan.scroll;
        let span = Rect::new(0, lo, screen.width, lines + new_height);
        self.model.reset_to(span);
        if !model_is_blank {
            // The old viewport lines have not moved on screen: reuse them.
            for y in lo.max(top)..old_bottom.min(span.bottom()) {
                let from = usize::from(y - top) * width;
                let to = usize::from(y - lo) * width;
                self.model.content[to..to + width]
                    .clone_from_slice(&self.previous.content[from..from + width]);
            }
        }

        // The target: the model (copied into the kept buffer's memory) with inserted lines
        // at the top…
        self.target.area = span;
        self.target.content.clone_from(&self.model.content);
        let target = &mut self.target;
        for (i, row) in rows.chunks(row_width).enumerate() {
            for x in 0..width {
                target.content[i * width + x] = row.get(x).cloned().unwrap_or_default();
            }
            // A wide character clipped at the right edge would not fit: replace it. The last
            // cell of row `i` is the last one before row `i + 1` (none if the width is 0).
            if let Some(last) = target.content[..(i + 1) * width].last_mut()
                && last.width() == 2
            {
                last.reset();
            }
        }
        // …and, if drawing, the viewport rendered by the application.
        let new_area = Rect::new(0, plan.top, screen.width, new_height);
        let mut wanted_cursor = None;
        if let Some(render) = render {
            self.next.reset_to(new_area);
            let mut frame = Frame {
                area: new_area,
                buffer: &mut self.next,
                cursor_position: None,
            };
            render(&mut frame);
            wanted_cursor = frame.cursor_position;
            self.target.content[usize::from(lines) * width..].clone_from_slice(&self.next.content);
        }

        let mut last_row = None;
        self.backend.draw(
            self.target
                .diff_iter(&self.model)
                .inspect(|&(_, y, _)| last_row = Some(y)),
        )?;
        if let Some(y) = last_row {
            moved = true;
            self.cursor_row = y;
        }

        // Stale lines below the viewport when it shrinks more than the logs fill.
        for y in erase {
            self.backend.set_cursor_position(0, y)?;
            self.backend.clear(ClearType::CurrentLine)?;
            moved = true;
            self.cursor_row = y;
        }

        self.area = new_area;
        self.previous.area = new_area;
        self.previous.content.clear();
        self.previous
            .content
            .extend_from_slice(&self.target.content[usize::from(lines) * width..]);
        Ok((wanted_cursor, moved))
    }

    // --- shared --------------------------------------------------------------------------

    /// Show, move, or hide the cursor as requested by the frame.
    ///
    /// `moved`: writes have moved the real cursor since its last placement; reposition
    /// it even if the desired position has not changed.
    fn place_cursor(&mut self, wanted: Option<Position>, moved: bool) -> io::Result<()> {
        match wanted {
            Some(position) => {
                if self.cursor.is_none() {
                    self.backend.show_cursor()?;
                }
                if moved || self.cursor != Some(position) {
                    self.backend.set_cursor_position(position.x, position.y)?;
                }
                self.cursor_row = position.y;
            }
            None if self.cursor.is_some() => self.backend.hide_cursor()?,
            None => {}
        }
        self.cursor = wanted;
        Ok(())
    }

    /// Clear the viewport and force the next `draw` to rewrite everything.
    ///
    /// Fullscreen: clear the entire screen. Inline: clear only the viewport lines and
    /// leave the cursor at the viewport start, where the shell will resume.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the terminal fails.
    pub fn clear(&mut self) -> io::Result<()> {
        match self.viewport {
            Viewport::Fullscreen => {
                self.backend.set_cursor_position(0, 0)?;
                self.backend.clear(ClearType::All)?;
            }
            Viewport::Inline(_) => {
                self.backend.begin_sync()?;
                for y in self.area.top()..self.area.bottom() {
                    self.backend.set_cursor_position(0, y)?;
                    self.backend.clear(ClearType::CurrentLine)?;
                }
                self.backend.set_cursor_position(0, self.area.y)?;
                self.backend.end_sync()?;
                self.cursor_row = self.area.y;
            }
        }
        // The “displayed” buffer becomes empty again, so the next diff includes everything.
        self.previous.reset_to(self.area);
        self.backend.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tilefab_core::style::Style;

    fn terminal(width: u16, height: u16) -> Terminal<Vec<u8>> {
        let backend =
            Backend::with_fixed(Vec::new(), Size::new(width, height), Position::new(0, 0));
        Terminal::new(backend).unwrap()
    }

    /// Take the bytes emitted since the last call.
    fn take(term: &mut Terminal<Vec<u8>>) -> String {
        let bytes = std::mem::take(term.backend_mut().writer_mut());
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn first_draw_writes_the_content_in_one_sync_block() {
        let mut term = terminal(6, 2);
        term.draw(|f| f.render_widget("hi", f.area())).unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[1;1Hhi\x1b[?2026l");
    }

    #[test]
    fn unchanged_frame_emits_only_the_sync_frame() {
        let mut term = terminal(6, 2);
        let render = |f: &mut Frame| f.render_widget("hi", f.area());
        term.draw(render).unwrap();
        take(&mut term);
        term.draw(render).unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[?2026l");
    }

    #[test]
    fn only_changed_cells_are_rewritten() {
        let mut term = terminal(6, 1);
        term.draw(|f| f.render_widget("abcdef", f.area())).unwrap();
        take(&mut term);
        term.draw(|f| f.render_widget("abXdef", f.area())).unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[1;3HX\x1b[?2026l");
    }

    #[test]
    fn content_that_disappears_is_erased_with_spaces() {
        let mut term = terminal(4, 1);
        term.draw(|f| f.render_widget("ab", f.area())).unwrap();
        take(&mut term);
        term.draw(|f| f.render_widget("a", f.area())).unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[1;2H \x1b[?2026l");
    }

    #[test]
    fn cursor_is_hidden_by_default_and_shown_then_hidden_on_request() {
        let mut term = terminal(6, 2);
        term.draw(|_| {}).unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[?2026l");

        term.draw(|f| f.set_cursor_position((2, 1))).unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[?25h\x1b[2;3H\x1b[?2026l");

        // Same position, nothing to redraw: no cursor sequence.
        term.draw(|f| f.set_cursor_position((2, 1))).unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[?2026l");

        term.draw(|_| {}).unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[?25l\x1b[?2026l");
    }

    #[test]
    fn cursor_is_replaced_after_cells_were_written() {
        let mut term = terminal(6, 1);
        term.draw(|f| f.set_cursor_position((0, 0))).unwrap();
        take(&mut term);
        term.draw(|f| {
            f.render_widget("x", f.area());
            f.set_cursor_position((0, 0));
        })
        .unwrap();
        // Writing `x` at (0,0) leaves the cursor at (1,0): move it back to (0,0).
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[1;1Hx\x1b[1;1H\x1b[?2026l");
    }

    #[test]
    fn resize_clears_the_screen_and_redraws_everything() {
        let mut term = terminal(4, 1);
        let render = |f: &mut Frame| f.render_widget("abcd", f.area());
        term.draw(render).unwrap();
        take(&mut term);

        term.backend_mut().set_fixed_size(Size::new(6, 2));
        term.draw(render).unwrap();
        assert_eq!(term.area(), Rect::new(0, 0, 6, 2));
        // Cursor at (0,0), full clear, then all the content.
        assert_eq!(
            take(&mut term),
            "\x1b[?2026h\x1b[1;1H\x1b[2J\x1b[1;1Habcd\x1b[?2026l"
        );
    }

    #[test]
    fn clear_forces_a_full_redraw() {
        let mut term = terminal(3, 1);
        let render = |f: &mut Frame| f.render_widget("abc", f.area());
        term.draw(render).unwrap();
        take(&mut term);
        term.clear().unwrap();
        assert_eq!(take(&mut term), "\x1b[1;1H\x1b[2J");
        term.draw(render).unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[1;1Habc\x1b[?2026l");
    }

    #[test]
    fn frame_exposes_the_area_and_the_buffer() {
        let mut term = terminal(5, 3);
        term.draw(|f| {
            assert_eq!(f.area(), Rect::new(0, 0, 5, 3));
            f.buffer_mut().set_string(0, 0, "z", Style::default());
        })
        .unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[1;1Hz\x1b[?2026l");
    }

    #[test]
    fn insert_before_is_ignored_while_the_known_width_is_zero() {
        let backend = Backend::with_fixed(Vec::new(), Size::new(0, 10), Position::new(0, 0));
        let mut term = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(2),
                scroll_mode: ScrollMode::Region,
            },
        )
        .unwrap();
        term.draw(|_| {}).unwrap();
        // No buffer to render into: the caller keeps the line (see `queue_output`).
        term.insert_before(1, |b| {
            "lost".render(b.area, b);
        });
        term.backend_mut().set_fixed_size(Size::new(10, 10));
        term.draw(|_| {}).unwrap();
        assert!(!take(&mut term).contains("lost"));
    }

    #[test]
    fn wide_character_clipped_by_a_narrower_screen_is_replaced() {
        let backend = Backend::with_fixed(Vec::new(), Size::new(4, 10), Position::new(0, 0));
        let mut term = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(2),
                scroll_mode: ScrollMode::Region,
            },
        )
        .unwrap();
        term.draw(|_| {}).unwrap();
        // At width 4, "ab漢" fills the row: the wide character takes columns 2 and 3.
        term.insert_before(1, |b| {
            "ab漢".render(b.area, b);
        });
        take(&mut term);
        // The screen narrows to 3 columns before the line is applied: half of the wide
        // character would be cut off, so it is not written.
        term.backend_mut().set_fixed_size(Size::new(3, 10));
        term.draw(|_| {}).unwrap();
        let out = take(&mut term);
        assert!(out.contains("ab"), "line lost: {out:?}");
        assert!(
            !out.contains('漢'),
            "clipped wide character written: {out:?}"
        );
    }

    #[test]
    fn frame_renders_stateful_widgets_and_updates_their_state() {
        /// Draws the number of times it was rendered, then increments it.
        struct Counter;
        impl StatefulWidget for Counter {
            type State = u32;
            fn render(self, area: Rect, buf: &mut Buffer, state: &mut u32) {
                *state += 1;
                state.to_string().as_str().render(area, buf);
            }
        }
        let mut term = terminal(4, 2);
        let mut state = 6;
        term.draw(|frame| frame.render_stateful_widget(Counter, frame.area(), &mut state))
            .unwrap();
        assert_eq!(
            state, 7,
            "the frame passes the caller's state to the widget"
        );
        assert_eq!(*term.buffer(), Buffer::with_lines(["7   ", "    "]));
    }

    #[test]
    fn a_line_kept_at_zero_width_survives_an_insertion_during_that_time() {
        let backend = Backend::with_fixed(Vec::new(), Size::new(10, 10), Position::new(0, 0));
        let mut term = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(2),
                scroll_mode: ScrollMode::Region,
            },
        )
        .unwrap();
        term.draw(|_| {}).unwrap();
        term.insert_before(1, |b| {
            "kept-line".render(b.area, b);
        });
        term.backend_mut().set_fixed_size(Size::new(0, 10));
        term.draw(|_| {}).unwrap();
        // While the width is 0, a new insertion is ignored, and must not disturb the line
        // already kept (its width is still that of the screen when it was queued).
        term.insert_before(1, |b| {
            "ignored".render(b.area, b);
        });
        take(&mut term);
        term.backend_mut().set_fixed_size(Size::new(10, 10));
        term.draw(|_| {}).unwrap();
        let out = take(&mut term);
        // No space in the text: the diff skips unchanged cells, so a space would split it.
        assert!(
            out.contains("kept-line"),
            "kept line garbled or lost: {out:?}"
        );
        assert!(!out.contains("ignored"));
    }

    #[test]
    fn pending_lines_are_kept_while_the_width_is_zero() {
        let backend = Backend::with_fixed(Vec::new(), Size::new(10, 10), Position::new(0, 0));
        let mut term = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(2),
                scroll_mode: ScrollMode::Region,
            },
        )
        .unwrap();
        term.draw(|_| {}).unwrap();
        term.insert_before(1, |b| {
            "log".render(b.area, b);
        });
        // The screen loses all its columns between `insert_before` and `draw`: no panic
        // (width 0 from crossterm), and the line is not lost.
        term.backend_mut().set_fixed_size(Size::new(0, 10));
        term.draw(|_| {}).unwrap();
        assert_eq!(term.area().width, 0);
        // Still kept after several frames at width 0.
        term.draw(|_| {}).unwrap();
        take(&mut term);
        // Columns again: the line is written, exactly once.
        term.backend_mut().set_fixed_size(Size::new(10, 10));
        term.draw(|_| {}).unwrap();
        assert!(take(&mut term).contains("log"), "pending line lost");
        term.draw(|_| {}).unwrap();
        assert!(
            !take(&mut term).contains("log"),
            "pending line written twice"
        );
    }

    #[test]
    fn inline_draw_on_a_screen_without_rows_does_not_panic() {
        let backend = Backend::with_fixed(Vec::new(), Size::new(10, 0), Position::new(0, 0));
        let mut term = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(1),
                ..TerminalOptions::default()
            },
        )
        .unwrap();
        term.insert_before(1, |b| {
            b.set_string(0, 0, "kept", Style::new());
        });
        take(&mut term);
        let mut rendered = false;
        term.draw(|_| rendered = true).unwrap();
        term.draw(|_| rendered = true).unwrap();
        assert!(!rendered, "a frame was rendered on a screen with no rows");
        // Nothing but the empty synchronized blocks: no scrolling, no cell written.
        assert_eq!(
            take(&mut term),
            "\x1b[?2026h\x1b[?2026l\x1b[?2026h\x1b[?2026l"
        );
    }

    #[test]
    fn rows_coming_back_after_zero_restore_the_viewport_and_pending_lines() {
        let backend = Backend::with_fixed(Vec::new(), Size::new(10, 10), Position::new(0, 0));
        let mut term = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(2),
                scroll_mode: ScrollMode::Region,
            },
        )
        .unwrap();
        term.draw(|_| {}).unwrap();
        term.insert_before(1, |b| {
            "kept-line".render(b.area, b);
        });
        term.backend_mut().set_fixed_size(Size::new(10, 0));
        term.draw(|_| {}).unwrap();
        term.draw(|_| {}).unwrap();
        let out = take(&mut term);
        assert!(
            !out.contains("kept-line"),
            "line written with no rows: {out:?}"
        );
        assert!(!out.contains("65535"), "out-of-range sequence: {out:?}");

        term.backend_mut().set_fixed_size(Size::new(10, 10));
        term.draw(|f| f.render_widget("ui", f.area())).unwrap();
        let out = take(&mut term);
        assert!(out.contains("kept-line"), "pending line lost: {out:?}");
        assert!(out.contains("ui"), "viewport not redrawn: {out:?}");
        assert_eq!(term.area().height, 2, "viewport height not restored");
        assert!(term.area().bottom() <= 10, "viewport off screen");
    }

    #[test]
    fn set_height_and_insert_before_do_nothing_in_fullscreen() {
        let mut term = terminal(5, 3);
        term.set_height(1);
        term.insert_before(1, |b| {
            b.set_string(0, 0, "x", Style::default());
        });
        // Nothing is queued either: lines kept here would never be applied.
        assert!(term.pending.is_empty(), "lines queued in fullscreen mode");
        term.draw(|_| {}).unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[?2026l");
        assert_eq!(term.area(), Rect::new(0, 0, 5, 3));
    }
}
