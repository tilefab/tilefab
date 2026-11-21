# tilefab

Small demo CLI tool in Rust, built on a home-made mini TUI library (a Cargo workspace of small crates, written directly on [crossterm](https://crates.io/crates/crossterm)):

- **log lines** scroll above a fixed area and stay in the terminal history;
- the fixed area (an *inline viewport*, no alternate screen) shows the **progress bars** of running tasks; its height is **dynamic**: one line per task, plus the prompt (1 line when idle);
- the last line is an **input prompt** (`> command`) that stays intact while logs and bars are updated.

```
> start beta
[INFO] task "alpha" started (duration 6.3 s)
[INFO] task "beta" started (duration 5.5 s)
alpha      ████████████░░░░░░░░░░░░░░░░░░░░░  36%
beta       ██████████████░░░░░░░░░░░░░░░░░░░  42%

> start gam█
```

## Running

```sh
cargo run
```

Outside a terminal (stdin or stdout redirected), the tool switches to **degraded mode**: no UI, commands are read line by line from stdin and logs are written to stderr. When stdin ends, it waits for the started tasks to finish.

```sh
echo "start a" | cargo run
printf 'start a\nstart b\nlist\n' | cargo run
```

## Commands

| Command | Effect |
|---|---|
| `start <name>` | starts a simulated task (3 to 10 s); the name is the rest of the line |
| `list` | logs the running tasks and their progress |
| `help` | logs the list of commands |
| `quit`, `exit`, Ctrl+C, Ctrl+D (on an empty line) | clean exit |

At most 5 simultaneous tasks (`MAX_TASKS`); beyond that, the command is refused with a WARN. An unknown command gives a WARN, `start` without a name an ERROR.
The hidden `panic` command triggers a deliberate panic, to check that the terminal is properly restored. It exists in debug builds only (`cargo build`, used by `tools/check_app.py`); in a release build, `panic` is an unknown command.

Line editing (provided by [tui-input](https://crates.io/crates/tui-input)): ←/→, Home/End, Ctrl+A/E, Backspace, Delete, Ctrl+←/→ (word by word), Ctrl+W or Alt+Backspace (deletes the previous word), Ctrl+U (clears the line), Ctrl+K (deletes to the end), Ctrl+Y (pastes back the last deleted text).

## Architecture

A single UI loop owns the `Terminal`; nothing else writes to stdout.

```
 keyboard thread ──(Key, Resize)───┐
 tokio tasks     ──(Progress, Done)┼──► AppEvent channel ──► UI loop ──► Terminal (inline viewport)
 tracing         ──(Log)───────────┤
 100 ms tick     ──(Tick)──────────┘
```

| File | Role |
|---|---|
| `crates/tilefab-demo/src/main.rs` | terminal init, keyboard thread, UI loop (one frame = lines + height + drawing) |
| `crates/tilefab-demo/src/app.rs` | `AppEvent`, app state, command parsing and execution |
| `crates/tilefab-demo/src/ui.rs` | viewport rendering (`LineGauge` bars + prompt), wanted height |
| `crates/tilefab-demo/src/logging.rs` | tracing `MakeWriter` that splits output into lines and sends them into the channel |
| `crates/tilefab-demo/src/tasks.rs` | simulated tasks (`tokio::spawn` + `interval`) |
| `crates/tilefab-demo/src/headless.rs` | degraded mode without a TTY |
| `crates/tilefab-*`, `crates/tilefab` | the library: see below |
| `crates/tilefab/examples/` | `fullscreen.rs` (full screen), `inline.rs` (variable-height viewport) and `cell_alloc.rs` (allocations and time per frame: `cargo run --release --example cell_alloc`) |
| `tools/` | pty harness (`pty_harness.py`), checks (`check_app.py`, `check_inline.py`), video recording and analysis (`record_check.sh`, `frames_check.py`) |

### The library: one Cargo workspace, one crate per layer

| Crate | Role | Depends on |
|---|---|---|
| `crates/tilefab-core` | `Cell`, `Buffer` and diff between two buffers (wide characters included); `Color`, `Modifier`, `Style`; `Span`, `Line`, `Text`, `Alignment`, `Stylize`; `Rect`, `Position`, `Size`, `Margin`, `Constraint`, `Layout`; the `Widget` and `StatefulWidget` traits (text types are widgets) | — (no crossterm) |
| `crates/tilefab-cli` | `backend`: ANSI sequences on a `W: Write` (cells, cursor, erasing, scroll regions); `terminal`: `Terminal` and `Frame` (diff with the previous frame, cursor, viewport); `viewport`: `Viewport` (full screen or inline), `ScrollMode`, commit placement rule; `init`: `TerminalGuard` (raw mode, alternate screen), panic hook | `tilefab-core`, crossterm |
| `crates/tilefab-widget` | `Block`, `Paragraph`, `Gauge`, `LineGauge`, `List`, `Clear`, drawn in a `Buffer` without I/O | `tilefab-core` |
| `crates/tilefab-input` | `InputLine`: prompt line editing, without I/O (a layer over tui-input for now) | crossterm (events), tui-input |
| `crates/tilefab` | façade: re-exports all of the above under `tilefab::…`; features `widget` and `input` (on by default) | the four crates above |
| `crates/tilefab-demo` | the demo application (binary `tilefab`), not published | `tilefab` |

`tilefab-widget` and `tilefab-input` do no input/output, so they are tested without a terminal, and neither depends on `tilefab-cli`. `cargo test`, `cargo clippy`… at the root apply to the whole workspace.

Rendering flow: `Frame` (current buffer) → diff with the previous buffer → `Backend` (`MoveTo` only on jumps, minimal style changes) → stdout, all inside a **single** synchronized-update block (`ESC[?2026h` … `ESC[?2026l`).

Notable choices:

- **A single crossterm version** (0.29) in the dependency tree; `tui-input` is used without its ratatui features (`default-features = false, features = ["crossterm"]`).
- **Single frame commit**: `Terminal::insert_before` and `Terminal::set_height` write nothing, they accumulate. `draw` applies scrolling, inserted lines, the new height and the redraw in one synchronized block: the terminal never shows an intermediate state. The viewport is anchored at the bottom: its last line (the prompt) does not move on screen when the height changes. The placement rule is detailed in `crates/tilefab-cli/src/viewport.rs`.
- **Task start and end lines**: produced directly in `app.output` (same `[INFO] …` format as tracing), not through tracing, so that they arrive in the **same** frame as the height change. Otherwise the viewport would shrink one frame before the line replacing it arrived.
- **Scroll regions**: crossterm has no DECSTBM command, `backend.rs` implements it (`SetScrollRegion`). Only the area above the viewport scrolls. `ScrollMode::Fallback` does the same without a region, for terminals that handle them poorly.
- **Cursor position**: a single `ESC[6n` query at startup, then internal tracking; a new query only after a resize. If the terminal does not answer at startup, the demo falls back to the degraded mode (commands read line by line), with a warning.
- **Keyboard thread using `poll` + `read`** rather than a lone blocking `read()`: crossterm has only one global input reader, locked for the whole duration of a `read()` wait. But the cursor position query needs the same lock and gives up after 2 s. With `poll(50 ms)`, the lock is released regularly.
- **Panic hook**: `install_panic_hook` restores the terminal (cursor visible, main screen, line mode) **before** calling the previous hook, color-eyre's, installed earlier. The panic then unwinds as usual (destructors run, `catch_unwind` can stop it). The demo uses `install_fatal_panic_hook` instead: every panic is fatal, after the report the hook ends the process with code 101, even for a panic in a tokio task or a secondary thread.

### Differences from ratatui

- **Layout**: no constraint solver, a simple four-step algorithm (see the header of `layout.rs`). No `Flex` other than `Start`; when space runs out, the last constraints are trimmed.
- **Text wrapping**: greedy algorithm, simpler than ratatui's (see the header of `paragraph.rs`).
- **Widgets**: only `Block`, `Paragraph`, `Gauge`, `LineGauge`, `List` and `Clear`. No tables, tabs, charts, or mouse support.
- **Viewport**: the height of an inline viewport can change (`set_height`), which ratatui does not allow.

## Tests and checks

`tools/ci.sh` runs all the checks below, in the same way as the CI workflow (`.github/workflows/ci.yml`), with `--locked` so that a `Cargo.lock` out of sync with `Cargo.toml` fails; it requires `cargo-deny` 0.21.0, the version the CI installs:

```sh
cargo fmt --check
cargo test                      # library (buffer, text, layout, widgets, backend, viewport) and application
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --release --all-targets --all-features -- -D warnings
cargo test --profile ci       # release build with overflow checks: the debug-only `panic` command is absent
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo deny check                # advisories, bans, licenses (see deny.toml), sources
```

The Rust toolchain is pinned in `rust-toolchain.toml` (rustc, rustfmt and clippy): local runs and CI use the same version, so a new stable release cannot break CI with new clippy lints on its own. To move to a new version, change `channel` there, run `rustup install` then `tools/ci.sh`, and fix what the new lints report.

The GitHub Actions of the CI are pinned by commit SHA (version in a comment). Dependabot (`.github/dependabot.yml`) checks them every week and opens a single pull request that updates the SHA and the comment; it does not touch the Cargo dependencies.

The minimum supported Rust version is **1.88** (`rust-version` in the root `Cargo.toml`). The CI job `msrv` builds and tests with it; locally: `rustup toolchain install 1.88 --profile minimal && cargo +1.88 test --all-targets --all-features`.

End to end, in a pty with a terminal emulator (local only, not part of the CI; once: `python3 -m venv tools/.venv && tools/.venv/bin/pip install -r tools/requirements.txt`):

```sh
cargo build --examples && cargo build
tools/.venv/bin/python tools/check_inline.py   # height cycle 1 → 5 → 1 with logs: a single synchronized block per commit,
                                               # no ESC[J / ESC[2J, no log lost or duplicated, prompt not moving
tools/.venv/bin/python tools/check_app.py      # Unicode input, burst of 5 tasks, resize, zero width, zero height, signals, panic, degraded mode
```

The same scenarios on the release build (no overflow checks, no `debug_assert!`; `panic` must then be an unknown command): `cargo build --release && tools/.venv/bin/python tools/check_app.py --release`.

Coverage, unit tests and pty harness combined (`main.rs` and `headless.rs` only run in the real binary, so unit tests alone under-report them); requires `cargo-llvm-cov` (once: `rustup component add llvm-tools-preview && cargo install --locked cargo-llvm-cov`):

```sh
tools/coverage.sh            # summary per file; --unit: unit tests only; --html: HTML report in target/llvm-cov/html
```

Mutation testing of the inline placement code (`cargo install --locked cargo-mutants` once): each mutant is a small change to the code that the tests must catch.

```sh
cargo mutants -p tilefab-cli -f crates/tilefab-cli/src/terminal.rs -f crates/tilefab-cli/src/viewport.rs -j 3   # report in mutants.out/
cargo mutants -p tilefab-widget -j 4   # widgets: about 3 minutes
```

On the widgets, three mutants survive because they do not change the behavior (equivalent mutants); a comment marks each one in the code (`block.rs`, `gauge.rs`, `paragraph.rs`). The timeouts are mutants that loop forever: the tests catch them too.

The frame commit tests (`crates/tilefab-cli/src/inline_tests.rs`) replay the emitted bytes in a small terminal simulator (`crates/tilefab-cli/src/sim.rs`): they cover every case of the placement rule, both scroll modes, lines inserted in batches and resizing.

`tools/record_check.sh` films the Terminal.app or iTerm2 window and `tools/frames_check.py` analyzes the video frame by frame (transient frames, blank lines, needless rewrites). It requires the macOS "Screen Recording" and "Automation" permissions.

## Known limits

- **Window shrinking**: Terminal.app and iTerm2 rewrap the existing lines when the window shrinks, which makes the viewport position uncertain. The cursor position is queried, everything from the top of the viewport to the bottom of the screen is erased (`ESC[J`, the only case besides exit) and everything is redrawn: copies of the old viewport may remain above, and log lines already displayed may disappear from the screen (depending on the terminal, they may or may not stay in the scrollback).
- **Terminal as short as the viewport**: with no room for log lines, they are ignored as long as the viewport fills the whole screen.
- **Maximum height**: `MAX_TASKS + 1` lines (6); below 6 terminal lines, the viewport is capped to the screen height.
- **A task ending with fewer log lines than freed lines**: a blank line may appear below the viewport (case `d > n`). In `tilefab` every task end produces a line, so this does not happen.
- **Synchronized update** (mode 2026): terminals that do not know it ignore it; the frame is then displayed as it is written (still in a single `write`).
- **Long log lines**: they are wrapped to the width at that moment; widening the window afterwards does not "unwrap" them.
- **Emoji width**: computed with `unicode-width`; some terminals display complex sequences differently (emoji with modifiers, flags), which can shift the cursor.
- **After a panic**, the viewport content (bars, prompt) stays on screen above the error report; the terminal itself is restored.
- **Degraded mode**: no progress bars; Ctrl+C there is the standard SIGINT signal (immediate stop).
- **Windows**: untested (`Release` events and rendering in Windows Terminal to be checked).

## License

MIT: see [LICENSE](LICENSE).
