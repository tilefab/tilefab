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

### Demo

- `compose "<composition>"` replaces `select`: it sets the composition of the tasks started
  next; `start <name> "<composition>"` gives a task its own. Each task is one row drawn by a
  `LineWidget` (name, local time, state symbol, progress bar and percentage, current step).

## [0.1.0]

First release on crates.io.

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
- Demo application `widget-demo` (`cargo install widget-demo`).

[Unreleased]: https://github.com/tilefab/tilefab/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/tilefab/tilefab/releases/tag/v0.1.0
