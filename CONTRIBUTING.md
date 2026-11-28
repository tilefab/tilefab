# Contributing

How the repository is organized, how the code fits together, and how to check a change. To use
the library, read the [quickstart](crates/tilefab/QUICKSTART.md) and the
[guide](crates/tilefab/GUIDE.md) instead.

Ground rules:

- the crates only depend downward (`tilefab-core` ← `tilefab-term`, `tilefab-widget`,
  `tilefab-input` ← `tilefab` ← the applications): never add an upward or sideways dependency;
- the documentation, the code comments and every message of the code are in English;
- every example of `crates/tilefab/QUICKSTART.md` and `crates/tilefab/GUIDE.md` is compiled by
  `cargo test` (they are included in the `tilefab` crate documentation), and
  `apps/quickstart/src/main.rs` must appear verbatim in the quickstart and both READMEs.

## Architecture

In the demo, a single UI loop owns the `Terminal`; nothing else writes to stdout.

```
 keyboard thread ──(Key, Paste, Resize)┐
 tokio tasks     ──(Progress, Done)────┼──► AppEvent channel ──► UI loop ──► Terminal (inline viewport)
 tracing         ──(Log)───────────────┤
 100 ms tick     ──(Tick)──────────────┘
```

| File | Role |
|---|---|
| `apps/widget-demo/src/main.rs` | terminal init, keyboard thread, UI loop (one frame = lines + height + drawing) |
| `apps/widget-demo/src/app.rs` | `AppEvent`, app state, command parsing and execution |
| `apps/widget-demo/src/ui.rs` | viewport rendering (one task line per row + prompt), wanted height |
| `apps/widget-demo/src/line.rs` | the line widget of a task: lists of values, default composition, updates, local time |
| `apps/widget-demo/src/logging.rs` | tracing `MakeWriter` that splits output into lines and sends them into the channel |
| `apps/widget-demo/src/tasks.rs` | simulated tasks (`tokio::spawn` + `interval`) |
| `apps/widget-demo/src/headless.rs` | degraded mode without a TTY |
| `apps/quickstart/src/main.rs` | the program of `crates/tilefab/QUICKSTART.md`; its test checks that the quickstart and both READMEs show it verbatim |
| `crates/tilefab-*`, `crates/tilefab` | the library: see below |
| `crates/tilefab/examples/` | `fullscreen.rs` (full screen), `inline.rs` (variable-height viewport), `panic_report.rs` (a panic in the render function after a resize, for `check_default_terminal.py`) and `cell_alloc.rs` (allocations and time per frame: `cargo run --release --example cell_alloc`) |
| `tools/` | pty harness (`pty_harness.py`), checks (`check_app.py`, `check_inline.py`, `check_default_terminal.py`), video recording and analysis (`record_check.sh`, `frames_check.py`) |

### The library: one Cargo workspace, one crate per layer

| Crate | Role | Depends on |
|---|---|---|
| `crates/tilefab-core` | `Cell`, `Buffer` and diff between two buffers (wide characters included); `Color`, `Modifier`, `Style`; `Span`, `Line`, `Text`, `Alignment`, `Stylize`; `Rect`, `Position`, `Size`, `Margin`, `Constraint`, `Layout`; the `Widget` and `StatefulWidget` traits (text types are widgets); `wrap`: wrapping of styled lines at a width | — (no crossterm) |
| `crates/tilefab-term` | `backend`: ANSI sequences on a `W: Write` (cells, cursor, erasing, scroll regions); `terminal`: `Terminal` and `Frame` (diff with the previous frame, cursor, viewport); `viewport`: `Viewport` (full screen or inline), `ScrollMode`, commit placement rule; `init`: `TerminalGuard` (raw mode, alternate screen), panic hook; `DefaultTerminal` and `init_inline` / `init_fullscreen` (setup and restoration in one call); `poll_key`, `is_ctrl_c` | `tilefab-core`, crossterm |
| `crates/tilefab-widget` | `LineWidget`: a one-line widget composed from a description string and lists of values, laid out again at every draw; drawn in a `Buffer` without I/O | `tilefab-core` |
| `crates/tilefab-input` | `LineEditor`: prompt line editor (keymap, kill ring, undo, horizontal scrolling), without terminal I/O | crossterm (events), `tilefab-core` |
| `crates/tilefab` | façade: re-exports all of the above under `tilefab::…`; features `widget` and `input` (on by default) | the four crates above |
| `apps/quickstart` | the program of the quickstart | `tilefab` |
| `apps/widget-demo` | the demo application (binary `widget-demo`) | `tilefab` |

`tilefab-widget` and `tilefab-input` do no input/output, so they are tested without a terminal, and neither depends on `tilefab-term`. `cargo test`, `cargo clippy`… at the root apply to the whole workspace.

Rendering flow: `Frame` (current buffer) → diff with the previous buffer → `Backend` (`MoveTo` only on jumps, minimal style changes) → stdout, all inside a **single** synchronized-update block (`ESC[?2026h` … `ESC[?2026l`). A frame that changes nothing writes nothing, not even an empty block: an idle application sends no bytes.

Notable choices:

- **A single crossterm version** (0.29) in the dependency tree, without its default `derive-more` feature (it would pull `syn` 2 next to the `syn` 3 of tokio's macros).
- **Single frame commit**: `Terminal::insert_before` and `Terminal::set_height` write nothing, they accumulate. `draw` applies scrolling, inserted lines, the new height and the redraw in one synchronized block: the terminal never shows an intermediate state. The viewport is anchored at the bottom: its last line (the prompt) does not move on screen when the height changes. The placement rule is detailed in `crates/tilefab-term/src/viewport.rs`.
- **Task start and end lines**: produced directly in `app.output` (same `[INFO] …` format as tracing), not through tracing, so that they arrive in the **same** frame as the height change. Otherwise the viewport would shrink one frame before the line replacing it arrived.
- **Making room for log lines** (`ScrollMode`): by default (`Screen`), line feeds on the last row scroll the whole screen, the viewport with it, and a single diff then rewrites what changed, in the same synchronized update: a line leaving the top of the screen this way reaches the scrollback in every terminal. `Region` scrolls only the area above the viewport with a scroll region (crossterm has no DECSTBM command, `backend.rs` implements it: `SetScrollRegion`), and writes less, but many terminals (xterm.js in VS Code, iTerm2 by default…) delete the lines that leave a region instead of keeping them in the scrollback. `Fallback` clears the viewport before scrolling. The demo reads the mode from `TILEFAB_SCROLL_MODE` (`screen`, `region` or `fallback`).
- **Cursor position**: a single `ESC[6n` query at startup, then internal tracking; a new query only after a resize. If the terminal does not answer at startup, the demo falls back to the degraded mode (commands read line by line), with a warning.
- **Keyboard thread using `poll` + `read`** rather than a lone blocking `read()`: crossterm has only one global input reader, locked for the whole duration of a `read()` wait. But the cursor position query needs the same lock and gives up after 2 s. With `poll(50 ms)`, the lock is released regularly.
- **Panic hook**: `install_panic_hook` restores the terminal (cursor visible, main screen, line mode) **before** calling the previous hook, color-eyre's, installed earlier. The panic then unwinds as usual (destructors run, `catch_unwind` can stop it). The demo uses `install_fatal_panic_hook` instead: every panic is fatal, after the report the hook ends the process with code 101, even for a panic in a tokio task or a secondary thread.

### Scope and simplifications

- **Layout**: no constraint solver, a simple four-step algorithm (see the header of `layout.rs`). No `Flex` other than `Start`; when space runs out, the last constraints are trimmed.
- **Widgets**: a single one, `LineWidget`, always one line. No blocks, paragraphs, lists, tables, charts, or mouse support. Text is not wrapped inside widgets; the lines printed above the viewport are wrapped by `Terminal::insert_lines` (`tilefab_core::wrap`).
- **Viewport**: the height of an inline viewport can change from one frame to the next (`set_height`).

## Tests and checks

`tools/ci.sh` runs all the checks below, in the same way as the CI workflow (`.github/workflows/ci.yml`), with `--locked` so that a `Cargo.lock` out of sync with `Cargo.toml` fails; it requires `cargo-deny` 0.21.0, the version the CI installs, and, once a release is tagged, `cargo-semver-checks` 0.51.0:

```sh
cargo fmt --check
cargo test                      # library (buffer, text, layout, line widget, backend, viewport) and application
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --release --all-targets --all-features -- -D warnings
cargo test --profile ci       # release build with overflow checks: the debug-only `panic` command is absent
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo deny check                # advisories, bans, licenses (see deny.toml), sources
cargo deny --manifest-path fuzz/Cargo.toml check   # the same for the fuzz crate (own Cargo.lock)
cargo package -p tilefab-core … -p tilefab          # each library crate built from its archive, as `cargo publish` will
cargo semver-checks -p tilefab-core … --baseline-rev <last v* tag>   # no breaking change without a new version
```

The packaging check runs on a copy of the workspace where the root `publish = false` becomes `publish = true`: with `publish = false`, cargo looks for the unpublished workspace crates on crates.io. It needs the network (the crates.io index). `cargo semver-checks` compares each library crate with the newest `v*` tag, and is skipped as long as there is none: before the first release, breaking changes go to the CHANGELOG (“Changed (breaking)”) only. A new toolchain may need a newer `cargo-semver-checks` (rustdoc JSON format): update its version in `tools/ci.sh` and `.github/workflows/ci.yml` with `rust-toolchain.toml`.

The workflow `.github/workflows/advisories.yml` checks the RustSec advisories again every Monday, for both lock files, so that an advisory published on a dependency already locked does not wait for the next push; it can also be started by hand from the Actions tab.

The Rust toolchain is pinned in `rust-toolchain.toml` (rustc, rustfmt and clippy): local runs and CI use the same version, so a new stable release cannot break CI with new clippy lints on its own. To move to a new version, change `channel` there, run `rustup install` then `tools/ci.sh`, and fix what the new lints report.

The GitHub Actions of the CI are pinned by commit SHA (version in a comment). Dependabot (`.github/dependabot.yml`) checks them every week and opens a single pull request that updates the SHA and the comment; it does not touch the Cargo dependencies.

The minimum supported Rust version is **1.88** (`rust-version` in the root `Cargo.toml`). The CI job `msrv` builds and tests with it; locally: `rustup toolchain install 1.88 --profile minimal && cargo +1.88 test --all-targets --all-features`.

The CI job `platforms` runs clippy and the tests on Windows and macOS: the code under `cfg(windows)` is compiled nowhere else. Locally, the Windows code can at least be linted: `rustup target add x86_64-pc-windows-msvc && cargo clippy --target x86_64-pc-windows-msvc --all-targets --all-features -- -D warnings`.

End to end, in a pty with a terminal emulator. The CI job `pty` (Linux) runs these three scripts on every push; locally, once: `python3 -m venv tools/.venv && tools/.venv/bin/pip install --require-hashes --only-binary=:all: -r tools/requirements-pty.txt` (pyte and wcwidth, pinned with the hash of their wheel), then `tools/.venv/bin/pip install -r tools/requirements.txt` for the video analysis (`frames_check.py`):

```sh
cargo build --examples && cargo build
tools/.venv/bin/python tools/check_inline.py   # height cycle 1 → 5 → 1 with logs: a single synchronized block per commit,
                                               # no ESC[J / ESC[2J, no log lost or duplicated, prompt not moving
tools/.venv/bin/python tools/check_app.py      # Unicode input, burst of 7 tasks (`MAX_TASKS`), resize, zero width, zero height, line editor (history, completion menu, search, paste), signals (and a second signal on a stuck UI), panic, degraded mode
tools/.venv/bin/python tools/check_default_terminal.py  # init_inline / init_fullscreen: quickstart (quit with q, to the end), and a panic in the render
                                                        # function after a resize: report kept on screen, line mode restored
```

The same scenarios on the release build (no overflow checks, no `debug_assert!`; `panic` must then be an unknown command): `cargo build --release && tools/.venv/bin/python tools/check_app.py --release`.

Coverage, unit tests and pty harness combined (`main.rs` and `headless.rs` only run in the real binary, so unit tests alone under-report them); requires `cargo-llvm-cov` (once: `rustup component add llvm-tools-preview && cargo install --locked cargo-llvm-cov`):

```sh
tools/coverage.sh            # summary per file; --unit: unit tests only; --html: HTML report in target/llvm-cov/html
```

Mutation testing of the inline placement code (`cargo install --locked cargo-mutants` once): each mutant is a small change to the code that the tests must catch.

```sh
cargo mutants -p tilefab-term -f crates/tilefab-term/src/terminal.rs -f crates/tilefab-term/src/viewport.rs -j 3   # report in mutants.out/
cargo mutants -p tilefab-widget -j 4   # the line widget
cargo mutants -p tilefab-input -j 4    # the line editor and its history
```

On the line widget and the line editor, every mutant is caught by the tests (checked on 27 November 2026: 448 mutants for the widget, 639 for the editor). One more is excluded in `.cargo/mutants.toml`, with its reason: it computes the same value as the code, so no test can catch it. The timeouts are mutants that loop forever: the tests catch them too.

Fuzzing with libFuzzer (`fuzz/`, a crate outside the workspace; once: `rustup toolchain install nightly --profile minimal && cargo install --locked cargo-fuzz`). Four targets, each checking invariants rather than only the absence of a panic:

- `buffer_set_string`: successive `Buffer::set_stringn` writes with tricky text (wide characters, combining marks, emoji sequences, control characters), at positions inside and outside the buffer; the buffer invariants hold after each write (one printable grapheme of width 1 or 2 per cell, a wide character always followed by its continuation);
- `line_widget`: `LineWidget` with an arbitrary composition (grammar tokens and raw text), lists, setter calls and widths; `build`, the setters and the drawing never panic, the buffer invariants hold, and nothing is written outside the first row of the area;
- `layout_split`: `Layout::split` with arbitrary constraints, margins and spacing; one rectangle per constraint, within the area, none overlapping.
- `line_editor`: `LineEditor` with arbitrary commands, pastes, `set_value` calls and renders at any width; the line never exceeds `max_len` nor holds a control character, the cursor stays in the line, and the rendered buffer keeps its invariants.

```sh
cd fuzz
cargo +nightly fuzz run line_widget -- -max_total_time=60   # or buffer_set_string, layout_split, line_editor
cargo +nightly fuzz run line_widget artifacts/line_widget/crash-…   # replay a crash
```

The corpus and the crashes stay local (`fuzz/corpus/`, `fuzz/artifacts/`). `tools/ci.sh` only checks that the targets build and pass clippy; the workflow `.github/workflows/fuzz.yml` runs each target for 60 seconds every Monday (or by hand from the Actions tab), and keeps the input of a crash as an artifact of the run.

The frame commit tests (`crates/tilefab-term/src/inline_tests.rs`) replay the emitted bytes in a small terminal simulator (`crates/tilefab-term/src/sim.rs`): they cover every case of the placement rule, both scroll modes, lines inserted in batches and resizing.

`tools/record_check.sh` films the Terminal.app or iTerm2 window and `tools/frames_check.py` analyzes the video frame by frame (transient frames, blank lines, needless rewrites). It requires the macOS "Screen Recording" and "Automation" permissions.

## License

By contributing, you agree that your contributions are licensed under the MIT license of the
project: see [LICENSE](LICENSE).
