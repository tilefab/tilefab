# Changelog

All notable changes to the published crates (`tilefab` and its `tilefab-*` layers, which share
one version) are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/) (in `0.x`, a minor version may
break the API).

## [Unreleased]

### Changed (breaking)

- `tilefab-widget` is rewritten around a single widget, `LineWidget`: one line composed from a
  short description string (`"B0 T2 B1 B2 S B2 L"`: barriers, date and time, symbols, keyword,
  names, log, value, progress and its illustration) and lists of values, laid out again at
  every draw (flexible components shrink, then the last components are dropped). Built by
  `LineWidget::builder(…)…build()`, which returns a `BuildError` or warnings; updated by
  setters that return an `UpdateError` and keep the previous value. New types: `Entry`,
  `WallTime`, `LineWidgetBuilder`, `BuildError`, `UpdateError`, `Warning`, `ListName`,
  `TokenProblem`, `EntryProblem`, `TimeField`, `Rank`.
- `tilefab-widget`: `LineWidget::set_symbol` and `set_name` take the rank of the component as
  a `Rank` (`set_name(Rank(0), 1)`), no longer as a second `usize` that could be swapped with
  the index of the entry without an error.
- `Block`, `Borders`, `BorderType`, `Padding`, `Clear`, `List`, `ListItem`, `ListState`,
  `Paragraph`, `Wrap`, `ProgressBar` and `ProgressLine` are removed, from `tilefab-widget` and
  from the re-exports of `tilefab` (feature `widget`), which now re-exports the new types.
- `tilefab-term`: new variant `ScrollMode::Screen`, the new default of the inline terminal
  (it was `Region`): it makes room for the inserted lines by scrolling the whole screen with
  line feeds, so that every log line reaches the scrollback in every terminal.
  `ScrollMode::Region` stays, documented as able to lose the scrollback (xterm.js in VS Code,
  iTerm2 by default…).
- `tilefab-input`: `Hinter` and `Completer` require `Send`, so that a `LineEditor` holding
  one is `Send` and can move to another thread or task (`tokio::spawn`). A hinter or a
  completer holding an `Rc` or a `RefCell` must switch to `Arc` or `Mutex`.
- `ScrollMode`, `Viewport` (`tilefab-term`) and `Constraint` (`tilefab-core`) are
  `#[non_exhaustive]`, like the other enums that may grow: a `match` on them needs a `_` arm.
- `tilefab-term`: the hooks for tests need the new `testing` feature (also on `tilefab`):
  `Backend::with_fixed`, `set_fixed_size`, `set_fixed_cursor`, `fail_cursor_queries`,
  `writer_mut`, and `Terminal::backend_mut`. Writing through them bypassed what `Terminal`
  knows of the screen (a cursor moved with `backend_mut` was not moved back by the next
  `draw`). Enable the feature in `[dev-dependencies]`; `Backend::writer` and
  `Terminal::backend` (read-only) stay.

### Added

- `tilefab::init_inline(height)`, `init_fullscreen()` and `init_with_options(options)` set the
  terminal up in one call: they check that stdin and stdout are terminals (an
  `io::ErrorKind::Unsupported` error otherwise), install the restoring panic hook once, enable
  raw mode (and the alternate screen) and create the terminal on a buffered stdout. They return
  a `DefaultTerminal`, which dereferences to `Terminal` and, when dropped, clears the viewport
  and restores the terminal (not during a panic); `finish` does the same and reports errors.
- `Terminal::insert_line` and `insert_lines` print text lines above the inline viewport,
  wrapped at the screen width (at most `MAX_PENDING_LINES` rows per call, the newest kept).
- `tilefab-core`: module `wrap` (`wrap_lines`, `wrap_lines_into`, `render_rows`, `TOO_WIDE`),
  the wrapping of styled lines that the demo had on its own, re-exported as `tilefab::wrap`.
- `tilefab::poll_key(timeout)` waits for a key press (releases skipped, other events dropped),
  and `is_ctrl_c(&key)` recognizes Ctrl+C, which raw mode delivers as a key.
- Documentation: `crates/tilefab/QUICKSTART.md` (a first program, step by step) and
  `crates/tilefab/GUIDE.md` (every feature, with examples), shown on docs.rs as the
  `tilefab::quickstart` and `tilefab::guide` modules and compiled as doctests. The `tilefab`
  README shows the quickstart program; the demo manual moves to `apps/widget-demo/README.md`,
  the architecture and the checks to `CONTRIBUTING.md`.
- `apps/quickstart`: the quickstart program, never published; its test checks that the
  quickstart and both READMEs show its code verbatim.
- `tilefab-widget`: the look of the illustration is chosen in the composition, with a letter
  after its size: `B` bar `█░`, `L` line `━─`, `D` dots `●·`, `S` squares `■□`, `O` circles
  `●○`, `R` diamonds `◆◇`, `G` segments `▰▱`, `H` shaded blocks `▓░`, `V` vertical bars `┃│`,
  `A` ASCII `#-`, `C` chevrons `>·` (`IL`, `I20D`). A look keeps the styles given to
  `.illustration(done, todo)` and replaces its characters; without a look, `I` uses those
  characters, or else the bar (`.illustration` is no longer required).
- `tilefab-widget`: any token of a composition may end with a *paint*, laid over the styles
  given to the builder for that component only: `.`, then a foreground color, `:` and a
  background color, `*` for bold, at least one of them and in this order (`B1.R`, `N.W+:B*`,
  `P.*`). Colors `K` `R` `G` `Y` `B` `M` `C` `W`, followed by `+` for the bright version; `I`
  and `V` take a second part after `/`, for their todo cells and their units (`I20L.G/K+`,
  `V5U/.W/K+`). It is resolved at construction: drawing stays allocation-free. New
  `TokenProblem::BadPaint` and `TokenProblem::SecondPart`.
- `tilefab-widget`: `LineWidget::value_shape` returns the shape of `V` (new type
  `ValueShape`: `size`, `unit`, `total`), the arguments `set_value` accepts, without parsing
  the composition again.
- `tilefab` (feature `widget`) re-exports the constants of `tilefab-widget` too: `MAX_ENTRIES`,
  `MAX_NAMES`, `MAX_VALUE_DECIMALS` and `DEFAULT_TARGET_WIDTH`, to check lists and values
  against the limits of `LineWidget` without depending on `tilefab-widget`.
- `tilefab-core` (and `tilefab`): `str_width`, the display width of a string, the width
  `Buffer::set_string` gives it. The layers and the demo measured text with copies of that
  rule; they all use this one now.
- `tilefab-input`: `History::file` returns the file the history appends to, `None` when it is
  in memory only.
- `tilefab-core`: `Buffer::normalize_wide` repairs, in place and without allocating, the
  pairing of wide characters that cells changed in place may break (a head without its
  continuation becomes a space, as an orphan continuation does). `Terminal` calls it on every
  frame.
- `tilefab-term` (and `tilefab`): `MAX_SCREEN_WIDTH` and `MAX_SCREEN_HEIGHT` (2048 × 1024),
  the most cells a `Terminal` draws (see "Fixed").
- `tilefab-term`: the `testing` feature (also on `tilefab`), for the hooks of tests: a backend
  with a fixed size and cursor, and write access to the stream and the backend (see "Changed
  (breaking)").

### Changed

- `tilefab-term`: a `draw` that changes nothing on screen writes nothing; it wrote an empty
  synchronized update (`ESC[?2026h ESC[?2026l`), so that an idle application redrawing
  on a timer, like the demo, sent ten of them a second. `Backend::begin_sync` now writes the
  opening with the first write that follows, and `end_sync` closes only an update opened.
- Workspace lints `allow_attributes` and `allow_attributes_without_reason`: every lint
  exception is an `#[expect(…, reason = …)]`.

### Fixed

- `tilefab-term`: in inline mode, the log lines that scrolled above the viewport were deleted
  instead of entering the scrollback in terminals that do not save the lines leaving a scroll
  region (VS Code's terminal among them).
- `tilefab-input`: `LineEditor::set_value` right after a yank made the next `YankPop` panic;
  with the search or the completion menu open, the next key threw the new value away.
- `tilefab-input`: the history rewrite used a predictable temporary name, followed through a
  symbolic link; it now creates a new file that nobody else uses. An existing history file
  readable by others is made private (`0600`), as documented.
- `tilefab-input`: loading the history read the whole file, and blocked or exhausted memory on
  a FIFO or `/dev/zero`; it now refuses anything but a regular file and reads at most its last
  16 MiB.
- `tilefab-input`: the rewrite of a history file lost the lines that another instance
  appended since the load.
- `tilefab-input`: the history file was checked to be a regular file before being opened,
  and then opened again at each line without a check: a file replaced by a FIFO after the
  load blocked the next Enter. Every open is now checked on the open file itself, and, on
  Unix, does not block nor follow a symbolic link put in its place.
- `tilefab-input`: a history path that is a symbolic link was replaced by a regular file at
  the first rewrite; the link is now resolved once, and the rewrite replaces the file it
  points to. A link to a missing file is an error.
- `tilefab-input`: when the rewrite of a large history file failed (read-only directory, full
  disk), the history switched to memory only, although the file could still be appended to;
  it now keeps appending, and `with_file` still returns the error.
- `tilefab-core`: `Buffer::filled` with a wide cell broke the invariant of wide characters
  (now head and continuation pairs); a `Rect` written literally past `u16::MAX` made
  `Buffer` overflow (the area is clipped like `Rect::new`).
- `tilefab-term`: the initial placement of an inline viewport overflowed at extreme sizes.
- `tilefab-term`: a screen reported as huge (a pty set to 65535 × 65535, which anyone can do)
  made the buffers allocate width × height cells of 40 bytes, and the process abort. A
  `Terminal` now draws at most `MAX_SCREEN_WIDTH` × `MAX_SCREEN_HEIGHT` cells (2048 × 1024,
  new constants, also in `tilefab`), and an inline viewport is at most `MAX_SCREEN_HEIGHT`
  rows tall.
- `tilefab-term`: a wide character set in place without its continuation
  (`buffer.get_mut(x, y).set_symbol("界")`), or a continuation copied elsewhere, left the
  screen different from what the next frames believed: the head covered a cell the diff
  thought visible, or the diff skipped a cell to erase. `Terminal` now repairs such pairs on
  every frame and on inserted lines, as spaces, with the new `Buffer::normalize_wide`
  (`tilefab-core`).
- `tilefab-term`: a frame interrupted by a write error or a panic left the synchronized update
  open (frozen display) and maybe a scroll region set; the terminal restoration also closes
  both now.
- `tilefab-term`: the panic hooks restored the terminal while another thread could still
  write to it: with `install_fatal_panic_hook`, a UI thread drawing while a task panicked
  could reopen a synchronized update before the exit, and the shell inherited a frozen
  display. The hooks now hold the lock of stdout from the restoration to the end of the
  report, and until the exit when fatal.
- `tilefab-term`: `insert_before` could queue lines without bound until the next `draw`.
- `tilefab-term`: `insert_before` with a large `height` allocated a buffer of that many rows,
  and kept it, to keep only the last `MAX_PENDING_LINES`; `height` is now capped at
  `MAX_PENDING_LINES` (a warning tells that the rows below are lost).
- `tilefab-term`: the lines still pending when the application quit (a viewport as tall as
  the screen has no room for them) were lost: `Terminal::clear` now writes them first, above
  a viewport of one row.
- `tilefab-term`: after a frame interrupted by a write error or a panic, the inserted lines
  of that frame were lost, and the next frames were drawn against a screen they no longer
  matched. The lines not written in full are queued again (one may show twice), and the next
  `draw` clears and redraws the viewport, as after a resize.
- `tilefab-term`: a frame that changed a text modifier (bold, dim…) allocated.
- `tilefab-term`: an inline frame copied the viewport about five times (a model of the screen,
  a target, then back into the previous buffer), even when nothing was inserted and the
  viewport stayed in place, as in a redraw at every tick. That frame is now diffed against
  the previous one directly, and the two buffers swapped, as in fullscreen (30 % less time
  per frame in `idle`, a new scenario of `cell_alloc`).
- `tilefab-core`: each grapheme written was segmented and measured up to three times, and
  `Cell::width` measured its symbol again at every call (the diff and the backend call it
  for every cell): a cell now keeps its width, measured once. In release, a screen of CJK
  and emoji is written about twice as fast, and the `cell_alloc` frames take 30 to 40 %
  less time.
- `tilefab-core`: a symbol longer than a cell holds inline (22 bytes, such as a family emoji
  joined by ZWJ) was copied with an allocation each time its cell was, about five times per
  frame; its copies now share it (`Arc<str>`), and only writing it allocates.
- `tilefab-input`: `LineEditor::render` allocated at every frame; during a history search
  (Ctrl+R) or with the completion menu open, it still did (the search prompt, each visible
  candidate, the `(+N more)` counter). The selected candidate was also padded in
  characters, not columns: a label with a combining mark was highlighted one column short.
- `tilefab-input`: a completion menu drawn down to the last row a `Rect` can reach
  (`u16::MAX - 1`) panicked with an overflow in debug builds.
- `tilefab-input`: the case-insensitive history search (Ctrl+R) compared the query from every
  position of each entry: a pasted query of 4 KiB over 1000 long entries took 13 s (in
  release). Each entry is now lowered once and searched in linear time (54 ms there), with
  the same matches.
- `tilefab-widget`: the layout of a `LineWidget` was quadratic in its number of components.
- `tilefab-core`: building and solving a `Layout` allocated about ten times per frame (a
  layout built at every frame, as the demo does). Up to 8 constraints are now kept inline,
  the solver uses no intermediate `Vec`, and `Layout::areas` allocates nothing; `split`
  allocates only the returned `Vec`. The sizes are the same.
- Demo: a task name with control characters (read on stdin) reached the terminal raw in the
  task started / finished lines; it is now escaped.
- Demo: a failure of the keyboard reader left the application running in raw mode without
  input; it now quits through the normal clean-up.
- Demo: only the first termination signal was handled: with the UI loop stuck (a terminal
  that reads nothing), the next ones were ignored, and only `kill -9` stopped the demo,
  leaving the terminal in raw mode. A second signal now ends it at once, raw mode disabled.
  A demo that quits on a signal exits with 128 + its number (it exited with 0); on
  Windows, with `STATUS_CONTROL_C_EXIT`.
- Demo: a stdin line had no length limit in headless mode; a line longer than the prompt
  (4096 bytes) is refused whole.
- Demo: the lines waiting while the terminal had no columns were kept without bound.
- Demo: a line logged while the demo quit (a task finishing at that moment) was dropped
  silently; it is now written to stderr once the terminal is restored.
- Demo: the list of log lines to insert, and the list of their wrapped rows, started from
  nothing at every burst; both keep their capacity now.
- Demo: in a terminal one column wide, the wide characters of the log lines (emoji,
  ideographs) disappeared without a trace; each one is now shown as the replacement
  character, U+FFFD.
- Demo: log lines wrapped into more than 65,535 rows (a narrow terminal) lost their newest
  rows; the demo now inserts the newest `MAX_PENDING_LINES` rows, and warns about the others.
- Demo: the value of a `V` whose paint has a second part (`V5U.W/K+`) stayed at 0: the demo
  took the `/` of the paint for a total. It now reads the shape from the widget.
- Demo: a carriage return, a tab or another C0 control character of a task name (read on
  stdin) reached stderr raw through `list`, since `tracing-subscriber` does not escape them;
  every control character of a log line but the line feed is now escaped as `\xNN`.

### Demo

- `compose "<composition>"` replaces `select`: it sets the composition of the tasks started
  next; `start <name> "<composition>"` gives a task its own. Each task is one row drawn by a
  `LineWidget` (name, local time, state symbol, progress bar and percentage, current step).
- The tasks share a name column, as wide as the longest running name (at most 20 columns, a
  longer name ends with `…`), so that their bars line up.
- The compositions of `compose` and `start` take the look of the bar and the paints; `help`
  lists them. The barriers `B0`–`B9` are space `-` `:` `|` `[` `]` `(` `)` `{` `}`, and the
  default composition is `N B0 B4 T2 B5 B0 S B0 I B0 P B0 L`.
- `TILEFAB_SCROLL_MODE=screen|region|fallback` chooses the scroll mode of the inline terminal.
- At most 7 tasks run at once (`MAX_TASKS`, 5 before): the viewport grows to 8 rows.
- On Windows, the history file is `%LOCALAPPDATA%\tilefab\history` (after `TILEFAB_HISTORY`):
  `HOME`, often unset there, left the demo without a history file, and `XDG_STATE_HOME` and
  `HOME` changed with the shell.
- On Windows, closing the console, logging off or shutting down quit through the clean-up.
- `tools/dist.sh` builds a demo binary to hand to someone else: a release build whose paths
  (the panic locations of the dependencies, under the Cargo home) no longer hold the home
  directory, and so the user name, of the machine that built it; the script checks it.

## 0.1.0 (not released yet)

Planned first release on crates.io: nothing is published yet (`publish = false` in
`Cargo.toml`), and there is no tag.

- `tilefab`: façade crate, features `widget` and `input` (on by default); re-exports
  `crossterm`, a public dependency (key events).
- Configuration types and growing enums are `#[non_exhaustive]`; configuration types have
  chained setters (`TerminalOptions::default().viewport(…)`).
- Terminal layer (`tilefab-term`): crossterm backend, double-buffered `Terminal`, full-screen and
  variable-height inline viewport, single synchronized frame commit, panic hooks.
- Common base (`tilefab-core`): cells, buffer, styles, geometry and layout, styled text, widget traits.
- Widgets (`tilefab-widget`): `Block`, `Paragraph`, `ProgressBar`, `ProgressLine`, `List`, `Clear`.
- Line editor (`tilefab-input`): `LineEditor` with keymap, kill ring, undo / redo, history,
  hints, completion and bracketed paste.
- Demo application `widget-demo` (`cargo install --path apps/widget-demo` until it is published).

[Unreleased]: https://github.com/tilefab/tilefab/commits/main
