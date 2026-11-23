# Changelog

All notable changes to the published crates (`tilefab` and its `tilefab-*` layers, which share
one version) are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/) (in `0.x`, a minor version may
break the API).

## [Unreleased]

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
