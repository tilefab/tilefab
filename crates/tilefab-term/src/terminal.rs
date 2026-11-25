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
//!   viewport above it.
//!   If the terminal does not answer the cursor query (slow SSH link, multiplexer), the
//!   position is estimated instead of failing: the viewport keeps its row, moved up just
//!   enough to stay on screen (anchored at the bottom) when the screen shrinks. A warning is
//!   logged.
//!
//! # Lines that cannot be shown yet
//!
//! If the viewport fills the entire screen (`height == terminal height`), there is no room
//! for inserted lines: they stay pending, and are applied on the first `draw` that leaves
//! room for them (smaller viewport, taller screen), in chunks if there are many. At most
//! [`MAX_PENDING_LINES`] lines wait from one `draw` to the next this way: beyond that, the
//! oldest are dropped (with one warning until the lines can be shown again), so that the
//! memory used stays bounded. The same limit applies on a screen with no rows or columns.
//!
//! A screen with no columns (a pty whose size is not set reports a width of 0) cannot show
//! inserted lines either, but this is transient: lines already queued stay pending and are
//! applied on the first `draw` where the screen has columns again. `insert_before` itself
//! ignores lines while the known width is 0 (there is no buffer to render them into): the
//! caller keeps them until then (see `Terminal::insert_before`).
//!
//! A screen with no rows is transient too: while its height is 0, `draw` writes nothing
//! (the render closure is not called, and nothing is erased) and keeps the pending lines;
//! the first `draw` with rows again repositions the viewport and applies them.

use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};

use super::backend::{Backend, ClearType};
use super::viewport::{ScrollMode, TerminalOptions, Viewport, plan_commit};
use tilefab_core::buffer::{Buffer, Cell};
use tilefab_core::convert::saturating_u16;
use tilefab_core::layout::{Position, Rect, Size};
use tilefab_core::widget::{StatefulWidget, Widget};

/// Maximum number of inserted lines kept pending from one inline `draw` to the next while
/// they cannot be shown (viewport filling the screen, screen with no rows or columns):
/// beyond that, `draw` drops the oldest. Lines queued between two draws are not limited:
/// when there is room, a large batch is applied in full, in chunks.
pub const MAX_PENDING_LINES: usize = 1_000;

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
    ///
    /// Change the cells, not the buffer area: a buffer replaced or reset to another area is
    /// discarded (the frame is drawn blank) and `Terminal::draw` returns an error.
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
    /// True once lines were dropped because more than `MAX_PENDING_LINES` were pending:
    /// warn once until the pending lines are applied (a warning per `draw` could feed
    /// itself, in an application that shows its logs with `insert_before`).
    pending_overflowed: bool,
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
                    pending_overflowed: false,
                    next: Buffer::empty(Rect::default()),
                    model: Buffer::empty(Rect::default()),
                    target: Buffer::empty(Rect::default()),
                    insert: Buffer::empty(Rect::default()),
                })
            }
            Viewport::Inline(height) => {
                let cursor = backend.cursor_position()?;
                let height = height.clamp(1, screen.height.max(1));
                let (append, top) = initial_placement(screen, cursor, height);

                let reserve = |backend: &mut Backend<W>| -> io::Result<()> {
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
                    backend.flush()
                };
                if let Err(error) = reserve(&mut backend) {
                    backend.abort_sync();
                    return Err(error);
                }

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
                    pending_overflowed: false,
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
        let area = self.insert.area();
        render(&mut self.insert);
        if self.insert.area() != area {
            tracing::warn!(
                "insert_before: the render function changed the buffer area: lines ignored"
            );
            return;
        }
        // Lines kept pending by a `draw` on a screen with no rows were queued at the width
        // known before it, which that `draw` may have changed: bring them to the current
        // width (truncated or padded, as `commit` would; it also replaces a wide character
        // clipped at the right edge), so that all pending lines share one width. Rare:
        // the allocation does not matter.
        if !self.pending.is_empty() && self.pending_width != width {
            let queued = std::mem::take(&mut self.pending);
            for row in queued.chunks(self.pending_width.max(1)) {
                self.pending.extend(row.iter().take(width).cloned());
                self.pending.extend(std::iter::repeat_n(
                    Cell::default(),
                    width.saturating_sub(row.len()),
                ));
            }
        }
        self.pending.extend_from_slice(self.insert.content());
        self.pending_width = width;
        // The bound holds even when `draw` is not called for a long time.
        self.limit_pending();
    }

    /// Called by `insert_before`, and by a `draw` that keeps the pending lines (no room to
    /// show them): keep only the newest `MAX_PENDING_LINES`, so that the memory used stays
    /// bounded.
    fn limit_pending(&mut self) {
        let lines = self.pending.len() / self.pending_width.max(1);
        if lines <= MAX_PENDING_LINES {
            return;
        }
        let dropped = lines - MAX_PENDING_LINES;
        self.pending.drain(..dropped * self.pending_width);
        if !self.pending_overflowed {
            self.pending_overflowed = true;
            tracing::warn!(
                "more than {MAX_PENDING_LINES} inserted lines waiting for room on screen: \
                 the oldest are dropped"
            );
        }
    }

    /// Draw a frame: `render` fills the buffer, then send the difference to the terminal.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the terminal fails, or an
    /// [`io::ErrorKind::InvalidInput`] error if `render` changed the frame buffer area
    /// (see [`Frame::buffer_mut`]): that frame is then drawn blank, and the terminal state
    /// stays consistent for the next one.
    ///
    /// A frame interrupted by a write error or a panic of `render` is closed on a best-effort
    /// basis (scroll region reset, synchronized update ended: see [`Backend::abort_sync`]),
    /// so that the terminal does not stay frozen; the panic then goes on.
    pub fn draw(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> io::Result<()> {
        let result = panic::catch_unwind(AssertUnwindSafe(|| match self.viewport {
            Viewport::Fullscreen => self.draw_fullscreen(render),
            Viewport::Inline(_) => self.draw_inline(render),
        }));
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.backend.abort_sync();
                Err(error)
            }
            Err(payload) => {
                self.backend.abort_sync();
                panic::resume_unwind(payload)
            }
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
        let mut replaced = false;
        render_checked(render, &mut frame, &mut replaced);
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
        replaced_frame_result(replaced)
    }

    // --- inline --------------------------------------------------------------------------

    fn draw_inline(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> io::Result<()> {
        // Query the terminal (size, cursor position) **before** opening the synchronized
        // block: the reply arrives on input, not in our output stream.
        let size = self.backend.size()?;
        // A screen with no rows (a pty whose height is set to 0) has no room for the
        // viewport: write nothing at all. No cursor query, and above all no erase: the
        // terminal may still display its rows, logs included. Only the size is noted (for
        // the width `insert_before` uses); the pending lines are kept. When rows come back,
        // the resize repositions the viewport and the lines are applied.
        if size.height == 0 {
            self.screen = size;
            self.limit_pending();
            return Ok(());
        }
        let resized = if size == self.screen {
            None
        } else {
            Some((size, self.cursor_after_resize(size)))
        };

        self.backend.begin_sync()?;
        if let Some((size, cursor)) = resized {
            self.resize_inline(size, cursor)?;
        }

        let screen_height = self.screen.height;
        let new_height = self.wanted_height.clamp(1, screen_height.max(1));
        // How many inserted lines fit on screen with the viewport: apply larger batches
        // in chunks.
        let per_chunk = usize::from(screen_height.saturating_sub(new_height));
        // A zero-width screen (possible after a resize since `insert_before`) cannot show
        // the inserted lines, nor a viewport filling the screen: keep them pending for the
        // first draw with room for them (see the module header).
        let taken = self.screen.width > 0 && per_chunk > 0;
        let mut rows = if taken {
            self.pending_overflowed = false;
            std::mem::take(&mut self.pending)
        } else {
            self.limit_pending();
            Vec::new()
        };
        let row_width = self.pending_width.max(1);
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
        let mut replaced = false;
        let checked = |frame: &mut Frame<'_>| render_checked(render, frame, &mut replaced);
        let (wanted_cursor, last_moved) =
            self.commit(last, row_width, new_height, Some(checked))?;
        self.place_cursor(wanted_cursor, moved | last_moved)?;
        if taken {
            // Give the (now applied) lines' memory back for the next ones.
            rows.clear();
            self.pending = rows;
        }

        self.backend.end_sync()?;
        self.backend.flush()?;
        replaced_frame_result(replaced)
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
        // Never a screen with no rows here (see `draw_inline`).
        let height = self.wanted_height.clamp(1, size.height.max(1));
        self.wanted_height = height;
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
                self.model.content_mut()[to..to + width]
                    .clone_from_slice(&self.previous.content()[from..from + width]);
            }
        }

        // The target: the model (copied into the kept buffer's memory) with inserted lines
        // at the top…
        self.target.clone_from(&self.model);
        let target = self.target.content_mut();
        for (i, row) in rows.chunks(row_width).enumerate() {
            for x in 0..width {
                target[i * width + x] = row.get(x).cloned().unwrap_or_default();
            }
            // A wide character clipped at the right edge would not fit: replace it. The last
            // cell of row `i` is the last one before row `i + 1` (none if the width is 0).
            if let Some(last) = target[..(i + 1) * width].last_mut()
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
            self.target.content_mut()[usize::from(lines) * width..]
                .clone_from_slice(self.next.content());
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
        self.previous.reset_to(new_area);
        self.previous
            .content_mut()
            .clone_from_slice(&self.target.content()[usize::from(lines) * width..]);
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
                let area = self.area;
                let clear = |backend: &mut Backend<W>| -> io::Result<()> {
                    backend.begin_sync()?;
                    for y in area.top()..area.bottom() {
                        backend.set_cursor_position(0, y)?;
                        backend.clear(ClearType::CurrentLine)?;
                    }
                    backend.set_cursor_position(0, area.y)?;
                    backend.end_sync()
                };
                if let Err(error) = clear(&mut self.backend) {
                    self.backend.abort_sync();
                    return Err(error);
                }
                self.cursor_row = self.area.y;
            }
        }
        // The “displayed” buffer becomes empty again, so the next diff includes everything.
        self.previous.reset_to(self.area);
        self.backend.flush()
    }
}

/// Initial placement of an inline viewport of `height` rows (1 to the screen height) when the
/// cursor is at `cursor`: the number of line feeds to write, and the top row of the viewport.
///
/// One line feed leaves the prompt line when the cursor is not in column 0, then
/// `height - 1` reserve the remaining rows; at the bottom of the screen each line feed
/// scrolls. Computed in `u32` (the sum overflows a `u16` at extreme sizes), with a cursor
/// reported below the screen taken as being on its last row.
fn initial_placement(screen: Size, cursor: Position, height: u16) -> (u16, u16) {
    let last_row = u32::from(screen.height.saturating_sub(1));
    let cursor_y = u32::from(cursor.y).min(last_row);
    let extra = u32::from(cursor.x > 0);
    let append = extra + u32::from(height) - 1;
    let scroll = (cursor_y + append).saturating_sub(last_row);
    let top = cursor_y + extra - scroll;
    // `height` is at most the screen height: `top + height` stays on screen, and
    // `append` below 65536.
    (
        u16::try_from(append).unwrap_or(u16::MAX),
        u16::try_from(top).unwrap_or(0),
    )
}

/// Run `render`, then check that it left the frame buffer covering the frame area. A buffer
/// replaced or reset through [`Frame::buffer_mut`] would no longer match the screen model
/// (diff length, viewport rows): it is reset, so the frame is drawn blank, and `*replaced`
/// is set for `draw` to report it.
fn render_checked(render: impl FnOnce(&mut Frame<'_>), frame: &mut Frame<'_>, replaced: &mut bool) {
    render(frame);
    if frame.buffer.area() != frame.area {
        frame.buffer.reset_to(frame.area);
        *replaced = true;
    }
}

/// The result of `draw` once the frame is written: an error if `render` replaced the buffer.
fn replaced_frame_result(replaced: bool) -> io::Result<()> {
    if replaced {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the render function changed the frame buffer area: frame drawn blank",
        ));
    }
    Ok(())
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
            "lost".render(b.area(), b);
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
            "ab漢".render(b.area(), b);
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
            "kept-line".render(b.area(), b);
        });
        term.backend_mut().set_fixed_size(Size::new(0, 10));
        term.draw(|_| {}).unwrap();
        // While the width is 0, a new insertion is ignored, and must not disturb the line
        // already kept (its width is still that of the screen when it was queued).
        term.insert_before(1, |b| {
            "ignored".render(b.area(), b);
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

    /// An inline terminal on a 10x`rows` screen, viewport `height` rows, first frame drawn.
    fn inline_terminal(rows: u16, height: u16) -> Terminal<Vec<u8>> {
        let backend = Backend::with_fixed(Vec::new(), Size::new(10, rows), Position::new(0, 0));
        let mut term = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(height),
                scroll_mode: ScrollMode::Region,
            },
        )
        .unwrap();
        term.draw(|_| {}).unwrap();
        take(&mut term);
        term
    }

    /// Queue `count` one-row lines labelled `#0`, `#1`…, from `first`.
    fn insert_numbered(term: &mut Terminal<Vec<u8>>, first: usize, count: usize) {
        for n in first..first + count {
            term.insert_before(1, |b| {
                format!("#{n}").as_str().render(b.area(), b);
            });
        }
    }

    #[test]
    fn lines_wait_while_the_viewport_fills_the_screen() {
        let mut term = inline_terminal(3, 3);
        insert_numbered(&mut term, 0, 2);
        term.draw(|_| {}).unwrap();
        term.draw(|_| {}).unwrap();
        let out = take(&mut term);
        assert!(
            !out.contains("#0") && !out.contains("#1"),
            "no room, yet written: {out:?}"
        );
        // Room again: both lines are written, exactly once.
        term.set_height(1);
        term.draw(|_| {}).unwrap();
        let out = take(&mut term);
        assert!(
            out.contains("#0") && out.contains("#1"),
            "pending lines lost: {out:?}"
        );
        term.draw(|_| {}).unwrap();
        assert!(
            !take(&mut term).contains('#'),
            "pending lines written twice"
        );
    }

    #[test]
    fn lines_waiting_for_room_are_limited_to_the_newest() {
        let mut term = inline_terminal(3, 3);
        insert_numbered(&mut term, 0, MAX_PENDING_LINES + 5);
        term.draw(|_| {}).unwrap();
        assert_eq!(term.pending.len(), MAX_PENDING_LINES * 10);
        assert_eq!(term.pending[0].symbol(), "#");
        assert_eq!(
            term.pending[1].symbol(),
            "5",
            "the oldest lines are dropped first"
        );
        assert!(term.pending_overflowed);
        // Applied once there is room; the next overflow warns again.
        term.set_height(1);
        term.draw(|_| {}).unwrap();
        assert!(term.pending.is_empty(), "pending lines not applied");
        assert!(!term.pending_overflowed);
    }

    #[test]
    fn a_large_batch_is_applied_in_full_when_there_is_room() {
        let mut term = inline_terminal(5, 1);
        insert_numbered(&mut term, 0, MAX_PENDING_LINES);
        term.draw(|_| {}).unwrap();
        let out = take(&mut term);
        assert!(
            out.contains("#0") && out.contains("#999"),
            "batch truncated"
        );
        assert!(term.pending.is_empty(), "lines left pending");
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
            "log".render(b.area(), b);
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
        // Nothing at all: no scrolling, no cell written, no erase.
        assert_eq!(take(&mut term), "");
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
            "kept-line".render(b.area(), b);
        });
        take(&mut term);
        term.backend_mut().set_fixed_size(Size::new(10, 0));
        term.draw(|_| {}).unwrap();
        term.draw(|_| {}).unwrap();
        // Nothing written with no rows, and above all nothing erased: the terminal may
        // still display its rows, logs included.
        assert_eq!(take(&mut term), "", "output on a screen with no rows");

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

    #[test]
    fn a_replaced_buffer_in_fullscreen_is_an_error_not_a_panic() {
        let mut term = terminal(6, 2);
        term.draw(|f| f.render_widget("hi", f.area())).unwrap();
        take(&mut term);
        let error = term
            .draw(|f| *f.buffer_mut() = Buffer::empty(Rect::new(0, 0, 3, 1)))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        // The frame is drawn blank: "hi" is erased, in a closed synchronized block.
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[1;1H  \x1b[?2026l");
        // The next frame draws normally.
        term.draw(|f| f.render_widget("ok", f.area())).unwrap();
        assert_eq!(*term.buffer(), Buffer::with_lines(["ok    ", "      "]));
    }

    #[test]
    fn a_buffer_reset_to_another_area_inline_is_an_error_not_a_panic() {
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
            "kept-line".render(b.area(), b);
        });
        let error = term
            .draw(|f| f.buffer_mut().reset_to(Rect::new(0, 0, 3, 1)))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        // The inserted line was still applied, and the viewport is consistent.
        assert!(take(&mut term).contains("kept-line"));
        assert_eq!(term.area(), Rect::new(0, 1, 10, 2));
        term.draw(|f| f.render_widget("ui", f.area())).unwrap();
        assert!(take(&mut term).contains("ui"));
    }

    #[test]
    fn insert_before_ignores_lines_whose_buffer_area_was_changed() {
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
            *b = Buffer::with_lines(["changed-area"]);
        });
        take(&mut term);
        term.draw(|_| {}).unwrap();
        assert!(!take(&mut term).contains("changed"), "lines not ignored");
        assert_eq!(term.area(), Rect::new(0, 0, 10, 2), "viewport moved");
    }

    #[test]
    fn lines_kept_at_zero_rows_take_the_new_width_of_the_next_insertion() {
        // Found by proptest: the draw that discovers a screen with no rows also changes the
        // known width, while it keeps a line queued at the old width.
        let backend = Backend::with_fixed(Vec::new(), Size::new(14, 9), Position::new(0, 0));
        let mut term = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(2),
                scroll_mode: ScrollMode::Region,
            },
        )
        .unwrap();
        term.draw(|_| {}).unwrap();
        term.backend_mut().set_fixed_size(Size::new(6, 0));
        term.insert_before(1, |b| {
            "first-line".render(b.area(), b);
        });
        term.draw(|_| {}).unwrap();
        // Queued at width 6, after one at width 14: no panic, and both are written.
        term.insert_before(1, |b| {
            "second".render(b.area(), b);
        });
        take(&mut term);
        term.backend_mut().set_fixed_size(Size::new(6, 9));
        term.draw(|_| {}).unwrap();
        let out = take(&mut term);
        assert!(out.contains("first-"), "first line lost: {out:?}");
        assert!(out.contains("second"), "second line lost: {out:?}");
        assert!(
            !out.contains("line"),
            "first line not cut to 6 columns: {out:?}"
        );
    }
    #[test]
    fn insert_before_alone_keeps_the_pending_lines_bounded() {
        let mut term = inline_terminal(3, 3);
        insert_numbered(&mut term, 0, 3 * MAX_PENDING_LINES);
        assert_eq!(term.pending.len() / term.pending_width, MAX_PENDING_LINES);
        assert!(term.pending_overflowed);
    }

    /// An inline terminal on a fixed screen, without the first draw.
    fn placed(size: Size, cursor: Position, height: u16) -> Terminal<Vec<u8>> {
        let backend = Backend::with_fixed(Vec::new(), size, cursor);
        Terminal::with_options(
            backend,
            TerminalOptions::default().viewport(Viewport::Inline(height)),
        )
        .unwrap()
    }

    #[test]
    fn the_initial_placement_does_not_overflow_at_extreme_sizes() {
        let cases = [
            (Size::new(2, 65535), Position::new(1, 0), 65535),
            (Size::new(2, 65535), Position::new(0, 65534), 3),
            (Size::new(2, 65535), Position::new(1, 65534), 3),
            (Size::new(2, 65535), Position::new(1, 65535), 65535),
            // A cursor reported below the screen.
            (Size::new(2, 10), Position::new(1, 400), 3),
        ];
        for (size, cursor, height) in cases {
            let term = placed(size, cursor, height);
            let area = term.area();
            assert!(
                area.bottom() <= size.height,
                "{size:?} {cursor:?} {height}: {area:?}"
            );
            assert_eq!(area.height, height.min(size.height), "{size:?} {cursor:?}");
        }
    }

    /// A writer that fails once, when its output would pass `fail_at` bytes, then works.
    struct FailOnce {
        bytes: Vec<u8>,
        fail_at: Option<usize>,
    }

    impl Write for FailOnce {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(at) = self.fail_at
                && self.bytes.len() + buf.len() > at
            {
                self.fail_at = None;
                return Err(io::Error::other("injected failure"));
            }
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// What closes an interrupted frame: the scroll region reset (the cursor saved around it,
    /// since `ESC[r` moves it), then the end of the synchronized update.
    const CLOSING: &[u8] = b"\x1b7\x1b[r\x1b8\x1b[?2026l";

    fn failing_terminal() -> Terminal<FailOnce> {
        let writer = FailOnce {
            bytes: Vec::new(),
            fail_at: None,
        };
        let backend = Backend::with_fixed(writer, Size::new(10, 6), Position::new(0, 5));
        let mut term = Terminal::with_options(
            backend,
            TerminalOptions::default().viewport(Viewport::Inline(2)),
        )
        .unwrap();
        term.draw(|_| {}).unwrap();
        term
    }

    #[test]
    fn a_write_error_in_a_frame_closes_the_synchronized_update() {
        for offset in [1, 8, 20, 40] {
            let mut term = failing_terminal();
            term.insert_before(1, |b| "log line".render(b.area(), b));
            let writer = term.backend_mut().writer_mut();
            writer.fail_at = Some(writer.bytes.len() + offset);
            let result = term.draw(|f| f.render_widget("hello", f.area()));
            assert!(result.is_err(), "offset {offset}: no error");
            let bytes = &term.backend().writer().bytes;
            assert!(
                bytes.ends_with(CLOSING),
                "offset {offset}: {:?}",
                String::from_utf8_lossy(bytes)
            );
            // The next frame works again.
            term.draw(|f| f.render_widget("again", f.area())).unwrap();
        }
    }

    #[test]
    fn a_panic_in_the_render_function_closes_the_synchronized_update() {
        let mut term = failing_terminal();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = term.draw(|_| panic!("render failed"));
        }));
        assert!(result.is_err());
        let bytes = &term.backend().writer().bytes;
        assert!(
            bytes.ends_with(CLOSING),
            "{:?}",
            String::from_utf8_lossy(bytes)
        );
    }
}
