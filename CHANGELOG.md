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
  `TokenProblem`, `EntryProblem`, `TimeField`.
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

### Added

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
  `V5U/.W/K+`). It is resolved at construction: drawing stays allocation-free. New `TokenProblem::BadPaint` and
  `TokenProblem::SecondPart`.
- `tilefab-widget`: `LineWidget::value_shape` returns the shape of `V` (new type
  `ValueShape`: `size`, `unit`, `total`), the arguments `set_value` accepts, without parsing
  the composition again.
- `tilefab-input`: `History::file` returns the file the history appends to, `None` when it is
  in memory only.

### Changed

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
- `tilefab-input`: when the rewrite of a large history file failed (read-only directory, full
  disk), the history switched to memory only, although the file could still be appended to;
  it now keeps appending, and `with_file` still returns the error.
- `tilefab-core`: `Buffer::filled` with a wide cell broke the invariant of wide characters
  (now head and continuation pairs); a `Rect` written literally past `u16::MAX` made
  `Buffer` overflow (the area is clipped like `Rect::new`).
- `tilefab-term`: the initial placement of an inline viewport overflowed at extreme sizes.
- `tilefab-term`: a frame interrupted by a write error or a panic left the synchronized update
  open (frozen display) and maybe a scroll region set; the terminal restoration also closes
  both now.
- `tilefab-term`: `insert_before` could queue lines without bound until the next `draw`.
- `tilefab-term`: the lines still pending when the application quit (a viewport as tall as
  the screen has no room for them) were lost: `Terminal::clear` now writes them first, above
  a viewport of one row.
- `tilefab-term`: after a frame interrupted by a write error or a panic, the inserted lines
  of that frame were lost, and the next frames were drawn against a screen they no longer
  matched. The lines not written in full are queued again (one may show twice), and the next
  `draw` clears and redraws the viewport, as after a resize.
- `tilefab-term`: a frame that changed a text modifier (bold, dim…) allocated.
- `tilefab-input`: `LineEditor::render` allocated at every frame.
- `tilefab-widget`: the layout of a `LineWidget` was quadratic in its number of components.
- Demo: a task name with control characters (read on stdin) reached the terminal raw in the
  task started / finished lines; it is now escaped.
- Demo: a failure of the keyboard reader left the application running in raw mode without
  input; it now quits through the normal clean-up.
- Demo: a stdin line had no length limit in headless mode; a line longer than the prompt
  (4096 bytes) is refused whole.
- Demo: the lines waiting while the terminal had no columns were kept without bound.
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
- On Windows, closing the console, logging off or shutting down quit through the clean-up.

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
