//! `Terminal` and `Frame`: double buffering, the rendering loop, and the inline viewport.
//!
//! # Principle
//!
//! `Terminal` keeps the `previous` buffer (what the terminal already displays in the viewport)
//! and renders each `draw` into a second one. At the end of each `draw`, compute the difference
//! (`Buffer::diff`), send only those cells to the backend, and make the new buffer
//! `previous`. All rendering for a frame is wrapped in **a single** synchronized update. The
//! buffers are kept from one frame to the next: once warmed up, a frame allocates nothing,
//! except once per symbol longer than a cell holds inline (22 bytes, some emoji sequences)
//! that the application writes.
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
//! In both modes, a `draw` that changes nothing on screen writes nothing, not even an empty
//! synchronized block (see [`Backend::begin_sync`]).
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
//! [`Terminal::clear`], which an application calls before quitting, first writes the lines
//! still pending above a viewport of one row, so that they are not lost.
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
use super::viewport::{CommitPlan, ScrollMode, TerminalOptions, Viewport, plan_commit};
use tilefab_core::buffer::{Buffer, Cell};
use tilefab_core::convert::saturating_u16;
use tilefab_core::layout::{Position, Rect, Size};
use tilefab_core::text::Line;
use tilefab_core::widget::{StatefulWidget, Widget};
use tilefab_core::wrap::{render_rows, wrap_lines};

/// Maximum number of inserted lines kept pending until a `draw` can show them (viewport
/// filling the screen, screen with no rows or columns, or simply no `draw` yet): beyond that,
/// `insert_before` and `draw` drop the oldest. A single `insert_before` renders at most that
/// many rows.
pub const MAX_PENDING_LINES: usize = 1_000;

/// Most columns a `Terminal` draws: a wider screen is drawn in its first `MAX_SCREEN_WIDTH`
/// columns, the others are left as they are. The size of the screen comes from the terminal
/// (`TIOCGWINSZ`), which anyone can set to 65535 × 65535: the buffers, 40 bytes per cell,
/// would not fit in memory. The limits are above the largest real terminals (an 8K screen
/// with a tiny font has about 1900 columns and 540 rows).
pub const MAX_SCREEN_WIDTH: u16 = 2048;

/// Most rows a `Terminal` draws: in fullscreen, a taller screen is drawn in its first
/// `MAX_SCREEN_HEIGHT` rows; an inline viewport is at most that tall (see
/// [`MAX_SCREEN_WIDTH`]).
pub const MAX_SCREEN_HEIGHT: u16 = 1024;

/// The screen as `Terminal` uses it: the width capped to [`MAX_SCREEN_WIDTH`], and in
/// fullscreen the height to [`MAX_SCREEN_HEIGHT`]. An inline viewport keeps the real height,
/// which its placement needs (it is capped itself, see [`max_viewport_height`]).
fn capped(size: Size, viewport: Viewport) -> Size {
    let height = if matches!(viewport, Viewport::Fullscreen) {
        size.height.min(MAX_SCREEN_HEIGHT)
    } else {
        size.height
    };
    Size::new(size.width.min(MAX_SCREEN_WIDTH), height)
}

/// How the old viewport moved on screen when a commit made room for its lines (see
/// [`Terminal::commit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scrolled {
    /// Nothing scrolled.
    None,
    /// The area above the viewport scrolled (a scroll region): the viewport stayed put.
    Region,
    /// The whole screen scrolled, the viewport with it (`ScrollMode::Screen`).
    Shifted,
    /// The old viewport was cleared, then the screen scrolled: nothing of it is left.
    Blank,
}

/// The tallest inline viewport on `screen`: its height, capped to [`MAX_SCREEN_HEIGHT`], and
/// at least 1.
fn max_viewport_height(screen: Size) -> u16 {
    screen.height.clamp(1, MAX_SCREEN_HEIGHT)
}

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
    /// True after a frame interrupted by a write error or a panic: what the terminal shows
    /// no longer matches `previous`, so the next `draw` starts over as after a resize (clear,
    /// then redraw everything).
    damaged: bool,
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
        let screen = capped(backend.size()?, options.viewport);
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
                    damaged: false,
                    next: Buffer::empty(Rect::default()),
                    model: Buffer::empty(Rect::default()),
                    target: Buffer::empty(Rect::default()),
                    insert: Buffer::empty(Rect::default()),
                })
            }
            Viewport::Inline(height) => {
                let cursor = backend.cursor_position()?;
                let height = height.clamp(1, max_viewport_height(screen));
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
                    damaged: false,
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

    /// Mutable access to the backend, for tests (`testing` feature): to change the size of a
    /// test backend, or take the bytes it received. Writing to the terminal through it would
    /// bypass what `Terminal` knows of the screen (a cursor moved there is not moved back
    /// by the next `draw`), hence no access outside tests.
    #[cfg(any(test, feature = "testing"))]
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
            self.wanted_height = height.clamp(1, max_viewport_height(self.screen));
        }
    }

    /// Queue one text line for insertion above the inline viewport, wrapped at the screen
    /// width: the simple way to print a log line. See [`Terminal::insert_lines`].
    ///
    /// ```no_run
    /// # fn f(terminal: &mut tilefab_term::Terminal<std::io::Stdout>) {
    /// terminal.insert_line("[INFO] build started");
    /// # }
    /// ```
    pub fn insert_line<'a>(&mut self, line: impl Into<Line<'a>>) {
        self.insert_lines([line]);
    }

    /// Queue text lines for insertion above the inline viewport. Each one (a `&str`, a
    /// `String`, a [`Span`](tilefab_core::Span) or a styled [`Line`]) is wrapped at the screen
    /// width known at the last `draw` (see [`wrap_lines`]), so that a long line is never cut.
    /// Writes nothing: applied on the next `draw`, in the same frame as the height change.
    ///
    /// At most [`MAX_PENDING_LINES`] rows are kept from one call: when the lines wrap into more,
    /// the newest rows are kept, with a warning. No effect in fullscreen mode; while the screen
    /// has no columns (a pty without a size), the lines are dropped. For full control over the
    /// rows (reusing their allocations, keeping lines while the width is 0), use
    /// [`wrap_lines_into`](tilefab_core::wrap::wrap_lines_into) and
    /// [`Terminal::insert_before`].
    pub fn insert_lines<'a, L: Into<Line<'a>>>(&mut self, lines: impl IntoIterator<Item = L>) {
        let width = self.screen.width;
        if !matches!(self.viewport, Viewport::Inline(_)) || width == 0 {
            return;
        }
        let rows = wrap_lines(lines.into_iter().map(Into::into), width);
        let newest = &rows[rows.len().saturating_sub(MAX_PENDING_LINES)..];
        if newest.len() < rows.len() {
            tracing::warn!(
                "insert_lines: {} wrapped row(s) dropped, at most {MAX_PENDING_LINES} are kept",
                rows.len() - newest.len()
            );
        }
        self.insert_before(saturating_u16(newest.len()), |buf| render_rows(newest, buf));
    }

    /// Queue lines for insertion above the inline viewport. `render` fills a buffer of
    /// `height` rows and screen width. Writes nothing: applied on the next `draw`.
    /// The screen width is the one known at the last `draw`, i.e. `self.area().width`:
    /// use that width (not the current backend size) to compute `height` for wrapped text.
    /// Lines are stored at that width: if the screen width changes before they are applied,
    /// they are truncated or padded to the new width.
    /// No effect in fullscreen mode, nor while that width is 0: keep the lines and insert them
    /// once `self.area().width` is positive again.
    ///
    /// `height` is capped at [`MAX_PENDING_LINES`], the most lines kept pending: the buffer
    /// then has that many rows, and the rows `render` would draw below them are lost (with a
    /// warning). To keep the newest of a longer text, pass only its last `MAX_PENDING_LINES`
    /// rows.
    pub fn insert_before(&mut self, height: u16, render: impl FnOnce(&mut Buffer)) {
        let width = usize::from(self.screen.width);
        if !matches!(self.viewport, Viewport::Inline(_)) || height == 0 || width == 0 {
            return;
        }
        // Rendering more rows would allocate (and keep, as the capacity of `insert`) a buffer
        // of `height` rows only to drop all but `MAX_PENDING_LINES` of them.
        let max = saturating_u16(MAX_PENDING_LINES);
        if height > max && !self.pending_overflowed {
            self.pending_overflowed = true;
            tracing::warn!(
                "insert_before: {height} lines inserted at once, more than {MAX_PENDING_LINES}: \
                 the last ones are dropped"
            );
        }
        let height = height.min(max);
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
        // Cells changed in place may break the pairing of wide characters.
        self.insert.normalize_wide();
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
    /// `render` runs before anything is written: a panic in it writes nothing (no byte stays
    /// in a `BufWriter`, to be sent after the panic report) and keeps the pending lines for
    /// the next `draw`; the panic then goes on. A frame interrupted by a write error is closed
    /// on a best-effort basis (scroll region reset, synchronized update ended: see
    /// [`Backend::abort_sync`]), so that the terminal does not stay frozen. After either, the
    /// next `draw` starts over as after a resize (the viewport is cleared and redrawn
    /// entirely), since the screen may show part of the interrupted frame. In inline mode,
    /// the inserted lines of a frame interrupted by a write error that may not have been
    /// written are queued again: a line can then appear twice, but none is lost.
    pub fn draw(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> io::Result<()> {
        let result = panic::catch_unwind(AssertUnwindSafe(|| match self.viewport {
            Viewport::Fullscreen => self.draw_fullscreen(render),
            Viewport::Inline(_) => self.draw_inline(render),
        }));
        match result {
            Ok(Ok(replaced)) => replaced_frame_result(replaced),
            Ok(Err(error)) => {
                self.backend.abort_sync();
                self.damaged = true;
                Err(error)
            }
            Err(payload) => {
                self.backend.abort_sync();
                self.damaged = true;
                panic::resume_unwind(payload)
            }
        }
    }

    // --- fullscreen ---------------------------------------------------------------------

    /// Return whether `render` replaced the frame buffer (see [`replaced_frame_result`]).
    fn draw_fullscreen(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> io::Result<bool> {
        let size = capped(self.backend.size()?, self.viewport);
        // The terminal may have reflowed or lost lines, or shows part of an interrupted
        // frame: start with an empty screen, and the previous (empty) buffer then matches
        // what is displayed.
        let start_over = size != self.screen || self.damaged;
        if start_over {
            self.screen = size;
            self.area = Rect::new(0, 0, size.width, size.height);
            self.previous.reset_to(self.area);
            // Until the screen is cleared below: a panic of `render` leaves the next frame
            // to start over too.
            self.damaged = true;
        }

        // Render before writing anything (see `draw`): a panic of `render` leaves no byte
        // queued, that the writer would send after the panic report.
        self.next.reset_to(self.area);
        let mut frame = Frame {
            area: self.area,
            buffer: &mut self.next,
            cursor_position: None,
        };
        let mut replaced = false;
        render_checked(render, &mut frame, &mut replaced);
        let wanted_cursor = frame.cursor_position;
        // The diff needs the pairing of wide characters, which cells changed in place may
        // break (a head without its continuation, a continuation copied elsewhere).
        self.next.normalize_wide();

        // Start the synchronized block: everything that follows will appear at once.
        self.backend.begin_sync()?;
        if start_over {
            self.backend.set_cursor_position(0, 0)?;
            self.backend.clear(ClearType::All)?;
            self.damaged = false;
        }
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
        Ok(replaced)
    }

    // --- inline --------------------------------------------------------------------------

    /// Return whether `render` replaced the frame buffer (see [`replaced_frame_result`]).
    fn draw_inline(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> io::Result<bool> {
        // Query the terminal (size, cursor position) **before** opening the synchronized
        // block: the reply arrives on input, not in our output stream.
        let size = capped(self.backend.size()?, self.viewport);
        // A screen with no rows (a pty whose height is set to 0) has no room for the
        // viewport: write nothing at all. No cursor query, and above all no erase: the
        // terminal may still display its rows, logs included. Only the size is noted (for
        // the width `insert_before` uses); the pending lines are kept. When rows come back,
        // the resize repositions the viewport and the lines are applied.
        if size.height == 0 {
            self.screen = size;
            self.limit_pending();
            return Ok(false);
        }
        // After an interrupted frame, start over as after a resize (see `damaged`).
        let resized = if size == self.screen && !self.damaged {
            None
        } else {
            let cursor = self.cursor_after_resize(size);
            Some((size, self.resized_area(size, cursor)))
        };
        let (screen, area) = resized.unwrap_or((self.screen, self.area));

        let new_height = self.wanted_height.clamp(1, max_viewport_height(screen));
        // How many inserted lines fit on screen with the viewport: apply larger batches
        // in chunks.
        let per_chunk = usize::from(screen.height.saturating_sub(new_height));
        // A zero-width screen (possible after a resize since `insert_before`) cannot show
        // the inserted lines, nor a viewport filling the screen: keep them pending for the
        // first draw with room for them (see the module header).
        let taken = screen.width > 0 && per_chunk > 0;
        let row_width = self.pending_width.max(1);
        let chunk_cells = per_chunk.max(1) * row_width;

        // Render before writing anything, and before taking the pending lines (see `draw`):
        // a panic of `render` leaves no byte queued, that the writer would send after the
        // panic report, and keeps every pending line. The frame is rendered where the
        // viewport will be once the lines are applied.
        let rows: &[Cell] = if taken { &self.pending } else { &[] };
        let frame_area = final_area(
            screen,
            area,
            chunk_lines(rows, chunk_cells, row_width),
            new_height,
        );
        let mut replaced = false;
        let frame = match frame_area {
            Some(frame_area) => {
                self.next.reset_to(frame_area);
                let mut frame = Frame {
                    area: frame_area,
                    buffer: &mut self.next,
                    cursor_position: None,
                };
                render_checked(render, &mut frame, &mut replaced);
                let wanted_cursor = frame.cursor_position;
                // As in fullscreen: the diff needs the pairing of wide characters.
                self.next.normalize_wide();
                Some(RenderedFrame { wanted_cursor })
            }
            None => None,
        };

        self.backend.begin_sync()?;
        if let Some((size, area)) = resized {
            self.resize_inline(size, area)?;
            self.damaged = false;
        }
        let mut rows = if taken {
            self.pending_overflowed = false;
            std::mem::take(&mut self.pending)
        } else {
            self.limit_pending();
            Vec::new()
        };
        let mut applied = 0;
        let result = self.commit_rows(
            &rows,
            row_width,
            chunk_cells,
            new_height,
            frame,
            &mut applied,
        );
        if taken {
            if result.is_err() {
                // The chunks not committed in full may not have reached the screen: queue
                // them again for the next frame (the interrupted one may show twice).
                rows.drain(..applied);
            } else {
                // Give the (now applied) lines' memory back for the next ones.
                rows.clear();
            }
            self.pending = rows;
        }
        result?;

        self.backend.end_sync()?;
        self.backend.flush()?;
        Ok(replaced)
    }

    /// Commit `rows` in chunks of `chunk_cells` cells, the last one with `frame`, already
    /// rendered in `next` (`None`: not rendered, nothing to draw), then place the cursor. `applied` counts the cells of the chunks committed in full,
    /// for the caller to queue the others again after an error.
    fn commit_rows(
        &mut self,
        rows: &[Cell],
        row_width: usize,
        chunk_cells: usize,
        new_height: u16,
        frame: Option<RenderedFrame>,
        applied: &mut usize,
    ) -> io::Result<()> {
        // The last chunk is applied with the frame; the earlier ones, in order, before it.
        let mut chunks = rows.chunks(chunk_cells);
        let last = chunks.next_back().unwrap_or(&[]);

        // Did any chunk move the real cursor? (With several chunks, the last one always
        // scrolls and reports a move anyway: `cargo mutants` sees `|=` → `&=` survive, but
        // the accumulation keeps this correct without relying on that.)
        let mut moved = false;
        for chunk in chunks {
            moved |= self.commit(chunk, row_width, new_height, false)?;
            *applied += chunk.len();
        }
        let last_moved = self.commit(last, row_width, new_height, frame.is_some())?;
        *applied += last.len();
        self.place_cursor(
            frame.and_then(|frame| frame.wanted_cursor),
            moved | last_moved,
        )
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

    /// Where the viewport goes after a screen size change, the cursor being at `cursor`
    /// (see the module header). Writes nothing: [`Terminal::resize_inline`] moves it there.
    fn resized_area(&self, size: Size, cursor: Position) -> Rect {
        // Where the cursor was within the viewport: the shell repositions it elsewhere itself,
        // but this offset remains the best indication of the viewport's starting row.
        let offset = self
            .cursor_row
            .saturating_sub(self.area.y)
            .min(self.area.height.saturating_sub(1));
        // Never a screen with no rows here (see `draw_inline`).
        let height = self.wanted_height.clamp(1, max_viewport_height(size));
        let top = cursor
            .y
            .saturating_sub(offset)
            .min(size.height.saturating_sub(height));
        Rect::new(0, top, size.width, height)
    }

    /// Move the viewport to `area` (from [`Terminal::resized_area`]) on a screen of `size`.
    fn resize_inline(&mut self, size: Size, area: Rect) -> io::Result<()> {
        // `ESC[J` is allowed here: the screen content must be redrawn anyway.
        self.backend.set_cursor_position(0, area.y)?;
        self.backend.clear(ClearType::AfterCursor)?;

        self.screen = size;
        self.wanted_height = area.height;
        self.area = area;
        self.previous.reset_to(area);
        self.cursor_row = area.y;
        Ok(())
    }

    /// Apply a commit: scroll if necessary, write `rows` above the new viewport and, with
    /// `frame`, draw the viewport of `new_height` rows already rendered in `next` (at the
    /// area [`final_area`] gave). Return whether the real cursor moved.
    ///
    /// Everything goes through **a single diff**: a model of the screen content after
    /// scrolling, compared with the target (inserted lines + viewport). Only differing
    /// cells are written, whether in the logs or the viewport.
    fn commit(
        &mut self,
        rows: &[Cell],
        row_width: usize,
        new_height: u16,
        frame: bool,
    ) -> io::Result<bool> {
        let screen = self.screen;
        let width = usize::from(screen.width);
        let lines = saturating_u16(rows.len().div_ceil(row_width));
        let (top, height) = (self.area.y, self.area.height);
        // Outside the placement preconditions (callers do not commit on a screen with no
        // rows, nor more lines than fit), committing nothing keeps the state consistent.
        let Some(plan) = plan_commit(screen.height, top, height, lines, new_height) else {
            return Ok(false);
        };
        let new_area = Rect::new(0, plan.top, screen.width, new_height);
        debug_assert!(
            !frame || self.next.area() == new_area,
            "the frame was rendered at {:?}, the viewport goes to {new_area:?}",
            self.next.area()
        );
        // The common frame, a redraw at every tick (see `commit_in_place`).
        if lines == 0 && plan.scroll == 0 && plan.erase.is_empty() && new_area == self.area {
            return if frame {
                self.commit_in_place()
            } else {
                Ok(false)
            };
        }
        let scrolled = self.scroll_for_commit(&plan, top, height)?;
        // A fallback scroll erased the old viewport, and the stale lines with it.
        let erase = if scrolled == Scrolled::Blank {
            0..0
        } else {
            plan.erase.clone()
        };
        self.build_model(scrolled, top - plan.scroll, lines + new_height, top, height);
        self.build_target(rows, row_width, lines, frame);
        let written = self.write_commit(erase)?;
        let moved = scrolled != Scrolled::None || written;

        self.area = new_area;
        self.previous.reset_to(new_area);
        self.previous
            .content_mut()
            .clone_from_slice(&self.target.content()[usize::from(lines) * width..]);
        Ok(moved)
    }

    /// Step 1 of [`Terminal::commit`]: make room for `plan.scroll` lines above the viewport
    /// (`top`, `height` rows), in the way of the scroll mode, and say how the old viewport
    /// moved on screen.
    fn scroll_for_commit(
        &mut self,
        plan: &CommitPlan,
        top: u16,
        height: u16,
    ) -> io::Result<Scrolled> {
        if plan.scroll == 0 {
            return Ok(Scrolled::None);
        }
        let last_row = self.screen.height.saturating_sub(1);
        if self.scroll_mode == ScrollMode::Region && top >= 2 {
            // Only the area above the viewport scrolls; the viewport stays put.
            self.backend.scroll_region_up(0, top - 1, plan.scroll)?;
            Ok(Scrolled::Region)
        } else if self.scroll_mode == ScrollMode::Screen && plan.scroll <= top {
            // Only rows above the viewport leave the screen, into the scrollback: line
            // feeds on the last row scroll everything, the viewport moves up with the rest
            // and the diff rewrites what differs.
            self.backend.set_cursor_position(0, last_row)?;
            self.backend.append_lines(plan.scroll)?;
            Ok(Scrolled::Shifted)
        } else {
            // Fallback (or region too small: terminals ignore single-line regions).
            // Clear the old viewport, then scroll the whole screen by writing
            // line feeds at the bottom: the result is the same, since everything below
            // the logs is empty. The viewport will be fully rewritten.
            for y in top..top + height {
                self.backend.set_cursor_position(0, y)?;
                self.backend.clear(ClearType::CurrentLine)?;
            }
            self.backend.set_cursor_position(0, last_row)?;
            self.backend.append_lines(plan.scroll)?;
            Ok(Scrolled::Blank)
        }
    }

    /// Step 2 of [`Terminal::commit`]: the model, what the screen holds from row `lo`, over
    /// `rows` rows, once scrolled: the rows of the old viewport (`top`, `height` rows) where
    /// they now are, blank elsewhere.
    fn build_model(&mut self, scrolled: Scrolled, lo: u16, rows: u16, top: u16, height: u16) {
        let width = usize::from(self.screen.width);
        let span = Rect::new(0, lo, self.screen.width, rows);
        self.model.reset_to(span);
        match scrolled {
            Scrolled::Shifted => {
                // The old viewport is now `plan.scroll` rows higher: from `lo`, the top of
                // the model. The rows below it scrolled in blank.
                let rows = usize::from(height).min(usize::from(span.height));
                self.model.content_mut()[..rows * width]
                    .clone_from_slice(&self.previous.content()[..rows * width]);
            }
            Scrolled::None | Scrolled::Region => {
                // The old viewport lines have not moved on screen: reuse them.
                for y in lo.max(top)..(top + height).min(span.bottom()) {
                    let from = usize::from(y - top) * width;
                    let to = usize::from(y - lo) * width;
                    self.model.content_mut()[to..to + width]
                        .clone_from_slice(&self.previous.content()[from..from + width]);
                }
            }
            Scrolled::Blank => {}
        }
    }

    /// Step 3 of [`Terminal::commit`]: the target, the model with the inserted `rows`
    /// (`lines` of them) at its top and, with `frame`, the viewport the application rendered
    /// (in `next`) below them.
    fn build_target(&mut self, rows: &[Cell], row_width: usize, lines: u16, frame: bool) {
        let width = usize::from(self.screen.width);
        // The model, copied into the kept buffer's memory…
        self.target.clone_from(&self.model);
        let target = self.target.content_mut();
        // …with the inserted lines at the top…
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
        if frame {
            self.target.content_mut()[usize::from(lines) * width..]
                .clone_from_slice(self.next.content());
        }
    }

    /// Step 4 of [`Terminal::commit`]: write what differs between the model and the target,
    /// then clear the rows of `erase` (stale lines below a viewport that shrinks more than
    /// the logs fill). Return whether anything was written (the real cursor moved).
    fn write_commit(&mut self, erase: std::ops::Range<u16>) -> io::Result<bool> {
        let mut last_row = None;
        self.backend.draw(
            self.target
                .diff_iter(&self.model)
                .inspect(|&(_, y, _)| last_row = Some(y)),
        )?;
        let mut written = false;
        if let Some(y) = last_row {
            written = true;
            self.cursor_row = y;
        }
        for y in erase {
            self.backend.set_cursor_position(0, y)?;
            self.backend.clear(ClearType::CurrentLine)?;
            written = true;
            self.cursor_row = y;
        }
        Ok(written)
    }

    /// The fast path of [`Terminal::commit`], for the common frame (a redraw at every tick):
    /// nothing inserted nor scrolled, and the viewport keeps its place and height. Write
    /// what differs between the frame rendered in `next` and `previous`, then swap them, as
    /// in fullscreen, instead of building the model and the target (about five copies of
    /// the viewport). Return whether the real cursor moved.
    fn commit_in_place(&mut self) -> io::Result<bool> {
        let mut last_row = None;
        self.backend.draw(
            self.next
                .diff_iter(&self.previous)
                .inspect(|&(_, y, _)| last_row = Some(y)),
        )?;
        if let Some(y) = last_row {
            self.cursor_row = y;
        }
        std::mem::swap(&mut self.previous, &mut self.next);
        Ok(last_row.is_some())
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
    /// Fullscreen: clear the entire screen. Inline: first write the lines still pending (see
    /// [`Terminal::insert_before`]: the viewport filled the screen, for instance) above a
    /// viewport shrunk to one row, so that they are not lost when the application quits;
    /// then clear only the viewport lines and leave the cursor at the viewport start, where
    /// the shell will resume. The requested height is kept for the next `draw`. Lines that
    /// still have no room (a screen of one row, or with no rows or columns) stay pending.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the terminal fails.
    pub fn clear(&mut self) -> io::Result<()> {
        if matches!(self.viewport, Viewport::Inline(_)) && !self.pending.is_empty() {
            let wanted = self.wanted_height;
            self.wanted_height = 1;
            let flushed = self.draw(|_| {});
            self.wanted_height = wanted;
            flushed?;
        }
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

/// A frame rendered in `Terminal::next` before the commit that draws it.
#[derive(Debug, Clone, Copy)]
struct RenderedFrame {
    /// The cursor position the frame asked for (`None`: hidden).
    wanted_cursor: Option<Position>,
}

/// The number of lines of each chunk of `rows` (`chunk_cells` cells each, rows of
/// `row_width` cells), as `Terminal::commit_rows` applies them: none when `rows` is empty,
/// which still commits the frame alone.
fn chunk_lines(rows: &[Cell], chunk_cells: usize, row_width: usize) -> impl Iterator<Item = u16> {
    rows.chunks(chunk_cells)
        .map(move |chunk| saturating_u16(chunk.len().div_ceil(row_width)))
}

/// Where the viewport is (on `screen`, starting at `area`) after the commits of chunks of
/// `chunk_lines` lines each, then of the frame (`new_height` rows): the area the frame is
/// rendered at, before anything is written. `None` when the last commit is outside the
/// placement preconditions (see [`plan_commit`]): the frame is not drawn. Follows
/// `Terminal::commit` step by step.
fn final_area(
    screen: Size,
    area: Rect,
    chunk_lines: impl Iterator<Item = u16>,
    new_height: u16,
) -> Option<Rect> {
    let mut chunk_lines = chunk_lines.peekable();
    let (mut top, mut height) = (area.y, area.height);
    loop {
        // The last commit carries the frame, after the last chunk (or alone, with none).
        let lines = chunk_lines.next().unwrap_or(0);
        let last = chunk_lines.peek().is_none();
        match plan_commit(screen.height, top, height, lines, new_height) {
            Some(plan) => (top, height) = (plan.top, new_height),
            None if last => return None,
            None => {}
        }
        if last {
            return Some(Rect::new(0, top, screen.width, new_height));
        }
    }
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
    fn an_unchanged_frame_writes_nothing() {
        let mut term = terminal(6, 2);
        let render = |f: &mut Frame| f.render_widget("hi", f.area());
        term.draw(render).unwrap();
        take(&mut term);
        term.draw(render).unwrap();
        assert_eq!(take(&mut term), "");
    }

    #[test]
    fn a_wide_symbol_set_in_place_without_its_continuation_is_drawn_as_a_space() {
        let mut term = terminal(4, 1);
        term.draw(|f| f.render_widget("abcd", f.area())).unwrap();
        take(&mut term);
        // `set_symbol` on the head only: printed as is, it would cover the `b` the diff
        // believes on screen.
        term.draw(|f| {
            f.render_widget("abcd", f.area());
            f.buffer_mut().get_mut(0, 0).set_symbol("界");
        })
        .unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[1;1H \x1b[?2026l");
        assert_eq!(term.buffer().get(1, 0).symbol(), "b");
    }

    #[test]
    fn a_continuation_copied_elsewhere_is_drawn_as_a_space() {
        let mut term = terminal(4, 1);
        term.draw(|f| f.render_widget("界cd", f.area())).unwrap();
        take(&mut term);
        // The diff skips a continuation: without the repair, the `d` would stay on screen
        // while the terminal believes it erased.
        term.draw(|f| {
            f.render_widget("界cd", f.area());
            let continuation = f.buffer_mut().get(1, 0).clone();
            *f.buffer_mut().get_mut(3, 0) = continuation;
        })
        .unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[1;4H \x1b[?2026l");
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
        assert_eq!(take(&mut term), "");

        term.draw(|f| f.set_cursor_position((2, 1))).unwrap();
        assert_eq!(take(&mut term), "\x1b[?2026h\x1b[?25h\x1b[2;3H\x1b[?2026l");

        // Same position, nothing to redraw: no cursor sequence, nothing at all.
        term.draw(|f| f.set_cursor_position((2, 1))).unwrap();
        assert_eq!(take(&mut term), "");

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
    /// [`inline_terminal`] in `mode`, its viewport moved to the bottom of the screen by
    /// enough log lines (so that the next one scrolls, with `top >= 2`).
    fn at_bottom(mode: ScrollMode, rows: u16, height: u16) -> Terminal<Vec<u8>> {
        let mut term = inline_terminal(rows, height);
        term.scroll_mode = mode;
        insert_numbered(&mut term, 0, usize::from(rows));
        term.draw(|_| {}).unwrap();
        assert_eq!(term.area().bottom(), rows, "not at the bottom");
        take(&mut term);
        term
    }

    /// Whether `out` sets a scroll region (`CSI top ; bottom r`, not the reset `CSI r`).
    fn sets_a_scroll_region(out: &str) -> bool {
        out.split("\x1b[").any(|seq| {
            seq.split_once('r').is_some_and(|(params, _)| {
                params.contains(';') && params.chars().all(|c| c.is_ascii_digit() || c == ';')
            })
        })
    }

    #[test]
    fn each_scroll_mode_makes_room_its_own_way() {
        for mode in [ScrollMode::Screen, ScrollMode::Region, ScrollMode::Fallback] {
            let mut term = at_bottom(mode, 10, 3);
            assert!(term.area().y >= 2);
            insert_numbered(&mut term, 50, 1);
            term.draw(|_| {}).unwrap();
            let out = take(&mut term);
            assert!(out.contains("#50"), "{mode:?}: {out:?}");
            assert_eq!(
                sets_a_scroll_region(&out),
                mode == ScrollMode::Region,
                "{mode:?}: {out:?}"
            );
            // `Fallback` clears the 3 rows of the old viewport before scrolling; `Screen`
            // and `Region` clear nothing.
            let cleared = out.matches("\x1b[2K").count();
            let expected = if mode == ScrollMode::Fallback { 3 } else { 0 };
            assert_eq!(cleared, expected, "{mode:?}: {out:?}");
        }
    }

    #[test]
    fn the_cursor_is_placed_again_after_a_scroll_that_writes_nothing() {
        // A blank line inserted under a blank viewport: the screen scrolls, nothing differs,
        // but the real cursor moved, so it is placed again where the frame wants it.
        for mode in [ScrollMode::Screen, ScrollMode::Region, ScrollMode::Fallback] {
            let mut term = at_bottom(mode, 10, 3);
            let top = term.area().y;
            let blank = |f: &mut Frame| {
                f.buffer_mut().reset();
                f.set_cursor_position((2, top));
            };
            term.draw(blank).unwrap();
            take(&mut term);
            term.insert_before(1, |_| {});
            term.draw(blank).unwrap();
            let out = take(&mut term);
            let wanted = format!("\x1b[{};3H", top + 1);
            assert!(
                out.ends_with(&format!("{wanted}\x1b[?2026l")),
                "{mode:?}: {out:?}"
            );
        }
    }

    #[test]
    fn a_frame_in_place_swaps_the_buffers() {
        // The fast path of `commit`: the frame is diffed against `previous`, then the two
        // buffers are swapped, so `next` holds the frame before it.
        let mut term = inline_terminal(10, 2);
        term.draw(|f| f.render_widget("one", f.area())).unwrap();
        term.draw(|f| f.render_widget("two", f.area())).unwrap();
        assert_eq!(term.buffer().lines()[0].trim_end(), "two");
        assert_eq!(
            term.next.lines()[0].trim_end(),
            "one",
            "not diffed in place"
        );
        // With a line inserted, the general path: `next` holds the frame drawn.
        insert_numbered(&mut term, 0, 1);
        term.draw(|f| f.render_widget("three", f.area())).unwrap();
        assert_eq!(term.next.lines()[0].trim_end(), "three");
    }

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
    fn inserted_lines_with_a_broken_wide_pair_are_repaired() {
        let mut term = inline_terminal(3, 1);
        term.insert_before(1, |buf| {
            buf.set_string(0, 0, "abc", Style::default());
            buf.get_mut(0, 0).set_symbol("界");
        });
        assert_eq!(term.pending[0].symbol(), " ");
        assert_eq!(term.pending[1].symbol(), "b");
    }

    #[test]
    fn a_tall_insert_renders_no_more_rows_than_are_kept() {
        let mut term = inline_terminal(3, 1);
        let mut rows = 0;
        term.insert_before(u16::MAX, |buf| rows = buf.area().height);
        assert_eq!(usize::from(rows), MAX_PENDING_LINES);
        assert_eq!(term.pending.len(), MAX_PENDING_LINES * 10);
        assert!(term.pending_overflowed, "the dropped rows are not reported");
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
        assert_eq!(take(&mut term), "");
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
            // A viewport is at most `MAX_SCREEN_HEIGHT` rows tall.
            let expected = height.min(size.height).min(MAX_SCREEN_HEIGHT);
            assert_eq!(area.height, expected, "{size:?} {cursor:?}");
        }
    }

    #[test]
    fn a_huge_screen_is_drawn_in_its_capped_part() {
        let huge = Size::new(u16::MAX, u16::MAX);
        // Fullscreen: both dimensions capped, from the start and after a resize.
        let backend = Backend::with_fixed(Vec::new(), huge, Position::new(0, 0));
        let mut term = Terminal::new(backend).unwrap();
        assert_eq!(
            term.area(),
            Rect::new(0, 0, MAX_SCREEN_WIDTH, MAX_SCREEN_HEIGHT)
        );
        term.backend_mut().set_fixed_size(Size::new(80, 24));
        term.draw(|_| {}).unwrap();
        term.backend_mut().set_fixed_size(huge);
        term.draw(|f| {
            assert_eq!(
                f.area(),
                Rect::new(0, 0, MAX_SCREEN_WIDTH, MAX_SCREEN_HEIGHT)
            );
        })
        .unwrap();
        assert_eq!(term.buffer().area(), term.area());

        // Inline: the width capped, the real height kept for the placement, and the viewport
        // at most `MAX_SCREEN_HEIGHT` rows.
        let backend = Backend::with_fixed(Vec::new(), huge, Position::new(0, 0));
        let mut term = Terminal::with_options(
            backend,
            TerminalOptions::default().viewport(Viewport::Inline(u16::MAX)),
        )
        .unwrap();
        assert_eq!(term.area().width, MAX_SCREEN_WIDTH);
        assert_eq!(term.area().height, MAX_SCREEN_HEIGHT);
        term.set_height(u16::MAX);
        term.insert_before(1, |buf| assert_eq!(buf.area().width, MAX_SCREEN_WIDTH));
        term.draw(|f| assert_eq!(f.area().height, MAX_SCREEN_HEIGHT))
            .unwrap();
        assert!(term.area().bottom() <= huge.height);
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
        failing_terminal_in(ScrollMode::default())
    }

    fn failing_terminal_in(mode: ScrollMode) -> Terminal<FailOnce> {
        let writer = FailOnce {
            bytes: Vec::new(),
            fail_at: None,
        };
        let backend = Backend::with_fixed(writer, Size::new(10, 6), Position::new(0, 5));
        let mut term = Terminal::with_options(
            backend,
            TerminalOptions::default()
                .viewport(Viewport::Inline(2))
                .scroll_mode(mode),
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

    /// Whether `needle` appears in `haystack`.
    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[test]
    fn a_frame_after_a_write_error_loses_no_line_and_redraws_everything() {
        let mut term = failing_terminal();
        term.insert_before(1, |b| "log line".render(b.area(), b));
        let before = term.backend().writer().bytes.len();
        term.draw(|f| f.render_widget("hello", f.area())).unwrap();
        let frame_len = term.backend().writer().bytes.len() - before;
        // Every offset of the failure within the frame.
        for offset in 1..frame_len {
            let mut term = failing_terminal();
            term.insert_before(1, |b| "log line".render(b.area(), b));
            let writer = term.backend_mut().writer_mut();
            writer.fail_at = Some(writer.bytes.len() + offset);
            assert!(term.draw(|f| f.render_widget("hello", f.area())).is_err());
            let failed = term.backend().writer().bytes.len();
            term.draw(|f| f.render_widget("again", f.area())).unwrap();
            let bytes = &term.backend().writer().bytes;
            let recovery = &bytes[failed..];
            // Written by the interrupted frame, or else queued again for the next one (the
            // diff skips the space, already blank on screen: `log`, then `line`).
            let shows_line = |part: &[u8]| contains(part, b"log") && contains(part, b"line");
            assert!(
                shows_line(&bytes[before..failed]) || shows_line(recovery),
                "offset {offset}: line lost: {:?}",
                String::from_utf8_lossy(bytes)
            );
            // The next frame starts over: erase from the viewport down, then the whole
            // viewport.
            assert!(
                contains(recovery, b"\x1b[J") && contains(recovery, b"again"),
                "offset {offset}: {:?}",
                String::from_utf8_lossy(recovery)
            );
        }
    }

    #[test]
    fn a_frame_after_a_good_one_does_not_start_over() {
        let mut term = failing_terminal();
        term.draw(|f| f.render_widget("hello", f.area())).unwrap();
        let before = term.backend().writer().bytes.len();
        term.draw(|f| f.render_widget("again", f.area())).unwrap();
        assert!(!contains(
            &term.backend().writer().bytes[before..],
            b"\x1b[J"
        ));
    }

    #[test]
    fn a_panic_in_the_render_function_writes_nothing_and_keeps_the_lines() {
        for mode in [ScrollMode::Screen, ScrollMode::Region, ScrollMode::Fallback] {
            let mut term = failing_terminal_in(mode);
            // A line to insert: making room for it would scroll, but `render` runs first.
            term.insert_before(1, |b| {
                b.set_string(0, 0, "log", Style::default());
            });
            let before = term.backend().writer().bytes.len();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = term.draw(|_| panic!("render failed"));
            }));
            assert!(result.is_err());
            // Nothing that a `BufWriter` would send after the panic report.
            let bytes = &term.backend().writer().bytes[before..];
            assert!(
                bytes.is_empty(),
                "{mode:?}: {:?}",
                String::from_utf8_lossy(bytes)
            );
            // The line is still pending: the next frame writes it.
            term.draw(|f| f.render_widget("ok", f.area())).unwrap();
            let bytes = &term.backend().writer().bytes[before..];
            assert!(
                contains(bytes, b"log") && contains(bytes, b"ok"),
                "{mode:?}: {:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn a_panic_after_a_resize_writes_nothing_and_the_next_frame_starts_over() {
        // Fullscreen: a resize clears the screen, which must not be written before `render`
        // (it would erase the panic report).
        let mut term = terminal(10, 4);
        term.draw(|f| f.render_widget("hi", f.area())).unwrap();
        term.backend_mut().set_fixed_size(Size::new(12, 5));
        let before = term.backend().writer().len();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = term.draw(|_| panic!("render failed"));
        }));
        assert!(result.is_err());
        assert_eq!(term.backend().writer().len(), before, "nothing written");
        term.draw(|f| f.render_widget("again", f.area())).unwrap();
        let out = String::from_utf8_lossy(&term.backend().writer()[before..]).into_owned();
        assert!(out.contains("\x1b[2J") && out.contains("again"), "{out:?}");
        assert_eq!(term.area(), Rect::new(0, 0, 12, 5));

        // Inline: the resize moves and clears the viewport, after `render` too.
        let mut term = failing_terminal();
        term.backend_mut().set_fixed_size(Size::new(12, 6));
        let before = term.backend().writer().bytes.len();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = term.draw(|_| panic!("render failed"));
        }));
        assert!(result.is_err());
        assert_eq!(
            term.backend().writer().bytes.len(),
            before,
            "nothing written"
        );
        term.draw(|f| f.render_widget("again", f.area())).unwrap();
        let bytes = &term.backend().writer().bytes[before..];
        assert!(contains(bytes, b"\x1b[J") && contains(bytes, b"again"));
    }

    #[test]
    fn a_panic_before_any_write_leaves_nothing_to_close() {
        let mut term = failing_terminal();
        let before = term.backend().writer().bytes.len();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = term.draw(|_| panic!("render failed"));
        }));
        assert!(result.is_err());
        let bytes = &term.backend().writer().bytes[before..];
        assert!(bytes.is_empty(), "{:?}", String::from_utf8_lossy(bytes));
    }
}
