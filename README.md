# tilefab

Small demo CLI tool in Rust, built on a home-made mini TUI library (a Cargo workspace of small crates, written directly on [crossterm](https://crates.io/crates/crossterm)):

- **log lines** scroll above a fixed area and stay in the terminal history;
- the fixed area (an *inline viewport*, no alternate screen) shows one **line widget** per running task (name, time, state, progress bar, current step…, composed by a short description string); its height is **dynamic**: one line per task, plus the prompt (1 line when idle);
- the last line is an **input prompt** (`> command`) that stays intact while logs and bars are updated.

```
> start beta
[INFO] task "alpha" started (duration 6.3 s)
[INFO] task "beta" started (duration 5.5 s)
alpha [05.123] 🟠 ███████████████░░░░░░░░░░░░░░░░░░░░░░░░░░  36% compiling
beta  [05.123] 🟠 ██████████████████░░░░░░░░░░░░░░░░░░░░░░░  42% compiling

> start gam█
```

## Running

```sh
cargo run
```

Installed from a clone (not on crates.io yet): `cargo install --path apps/widget-demo`, then `widget-demo`. To use the library in a project, depend on the [`tilefab`](crates/tilefab/README.md) crate only; the `tilefab-*` crates are its internal layers.

Outside a terminal (stdin or stdout redirected), the tool switches to **degraded mode**: no UI, commands are read line by line from stdin and logs are written to stderr. When stdin ends, it waits for the started tasks to finish.

```sh
echo "start a" | cargo run
printf 'start a\nstart b\nlist\n' | cargo run
```

In this mode there is no prompt, and `list` is the only way to see progress. All the piped lines are read at once, so in the second example `list` runs right after the two `start`, before any progress report (sent every 100 ms): both tasks show `0 %`. To see progress, wait before `list`:

```sh
{ printf 'start a\nstart b\n'; sleep 3; echo list; } | cargo run
```

Keys typed while it waits are echoed by the terminal but not read: stdin is the pipe.

## Commands

| Command | Effect |
|---|---|
| `start <name>` | starts a simulated task (3 to 10 s), drawn with the current composition; the name is the rest of the line |
| `start <name> "<composition>"` | the same, with a composition for this task only (the current one does not change) |
| `list` | logs the running tasks and their progress |
| `compose "<composition>"` | sets the composition of the tasks started next, for instance `compose "N B0 P B0 L"`; the composition goes between double quotes. An invalid composition is an ERROR and the current one is kept; a composition wider than 80 columns gives a WARN. Without an argument, shows the current composition. Each task keeps the composition it started with |
| `help` | logs the list of commands |
| `quit`, `exit`, Ctrl+C, Ctrl+D (on an empty line) | clean exit |

At most 5 simultaneous tasks (`MAX_TASKS`); beyond that, the command is refused with a WARN. An unknown command gives a WARN, `start` without a name an ERROR. A name that ends with a quoted part is read as a name and a composition: `start say "hi"` starts `say` with the composition `hi`, which is refused.

## Task lines

Each task is drawn on one row by a `LineWidget` of `tilefab-widget`, composed from a short string: one letter per component, separated by one space. The default composition is `N B0 B4 T2 B5 B0 S B0 I B0 P B0 L`:

```
alpha [05.123] 🟠 ████████████████░░░░░░░░░░░░░░  52% compiling
```

| Token | Component in the demo |
|---|---|
| `S`, `S0`–`S3` | state symbol: 🔴 waiting, 🟠 running, 🟡 almost done, 🟢 done (`S` follows the task, `S2` is always 🟡) |
| `B0`–`B9` | barriers: space, `-`, `:`, `\|`, `[`, `]`, `(`, `)`, `{`, `}` |
| `D0`–`D2` | date: `YYYY-MM-DD`, `YY-MM-DD`, `MM-DD` (local time, from the `jiff` crate) |
| `T0`–`T5` | time: `HH:MM:SS.SSS`, `MM:SS.SSS`, `SS.SSS`, `HH:MM:SS`, `HH:MM`, `MM:SS` |
| `P`, `P3`–`P9` | progress, rounded down, with `n − 4` decimals from `P5` (`P5`: ` 99.9%`) |
| `K`, `K0`–`K3` | state keyword: `waiting`, `running`, `finishing`, `done` |
| `N` | the task name |
| `L`, `L40` | the current step of the task (`compiling`…): elastic, or a fixed size |
| `V`, `V5U/` | simulated download: size of each number, `U` for the unit (MB), `/` for the total (`12.5MB/40.0MB`) |
| `I`, `I20`, `IL`, `I20D` | progress bar: elastic (10 columns at least when there is room), or a fixed size; then its look: `B` bar `█░` (default), `L` line `━─`, `D` dots `●·`, `S` squares `■□`, `O` circles `●○`, `R` diamonds `◆◇`, `G` segments `▰▱`, `H` shaded blocks `▓░`, `V` vertical bars `┃│`, `A` ASCII `#-`, `C` chevrons `>·` |

Any token can end with a **paint**, which changes the style of that component only: `.`, then at least one of, in this order: a text color, `:` and a background color, `*` for bold. Colors: `K` black, `R` red, `G` green, `Y` yellow, `B` blue, `M` magenta, `C` cyan, `W` white (the ANSI white, a light gray), followed by `+` for the bright version (`K+` dark gray, `W+` bright white). `I` and `V` take a second part after `/`, for the rest of the bar and for the units. The paint is laid over the demo's own styles (gray barriers and time, cyan bar, bold name and percentage): what it does not set is kept. For instance:

```
start build "N.W+:B* B0 B4.Y T2.Y B5.Y B0 IL.G/K+ B0 P.G* B0 L.C"
```

shows a bright white bold name on blue, a yellow time between yellow brackets, a green line on a dark gray track, a bold green percentage and a cyan step.

`D`, `T`, `P`, `K`, `L`, `V` and `I` appear at most once; `S`, `B` and `N` can be repeated. The line is laid out again at every frame, with the whole terminal width: the extra columns go to the elastic `L`, otherwise to the elastic `I`. When the window is too narrow, `L` and `I` shrink first (down to 2 columns, a cut text ends with `…`), then the names, then the last components are dropped; a barrier stays, which shows the line was cut. `help` lists the tokens.
The hidden `panic` command triggers a deliberate panic, to check that the terminal is properly restored. It exists in debug builds only (`cargo build`, used by `tools/check_app.py`); in a release build, `panic` is an unknown command. The hidden `count <n>` command (debug builds too) writes `n` numbered log lines at once, to check that every line can be found again by scrolling up in the terminal window (scenario `tools/scenarios/scrollback.scn`).

## Line editing

The prompt uses the line editor of `tilefab-input`: readline-style Emacs keys, Unicode-aware (the cursor moves by grapheme: an accented letter or an emoji is one step). A long line scrolls horizontally, keeping a few columns of context around the cursor; a dim `…` shows hidden text.

| Keys | Action |
|---|---|
| ←/→, Ctrl+B/Ctrl+F | one character left / right |
| Ctrl+←/→, Alt+←/→, Alt+B/Alt+F | previous word start / next word end |
| Home/End, Ctrl+A/Ctrl+E | start / end of the line |
| Backspace (Ctrl+H), Delete, Ctrl+D | delete the character before / under the cursor (Ctrl+D on an empty line quits) |
| Alt+Backspace, Alt+D (Ctrl+Delete) | delete the previous word / to the end of the next word |
| Ctrl+W | delete the previous whitespace-delimited word |
| Ctrl+U, Ctrl+K | delete to the start / to the end of the line |
| Ctrl+Y, then Alt+Y | paste back the last deleted text, then older ones (consecutive deletions are pasted back together) |
| Ctrl+T, Alt+T | swap two characters / two words |
| Alt+U, Alt+L, Alt+C | upper-case / lower-case / capitalize to the end of the word |
| Ctrl+_ (Ctrl+/, Ctrl+7, Ctrl+Z), Alt+_ | undo (a typed word at once) / redo |
| ↑/↓ | previous / next history entry that starts with what is typed |
| Ctrl+P/Ctrl+N, Alt+</Alt+> | previous / next history entry, oldest / newest |
| Ctrl+R | search the history (see below) |
| Tab, Shift+Tab | complete the command name (see below); with nothing to complete, Tab accepts the hint |
| Esc | hide the hint; back to the typed line while browsing the history |
| Enter (Ctrl+J, Ctrl+M) | run the line |

**History.** Lines run at the prompt are saved, each one once (the latest wins), up to 1000. A line that starts with a space is not saved. The file is `$TILEFAB_HISTORY` if set, else `$XDG_STATE_HOME/tilefab/history`, else `~/.local/state/tilefab/history`; on Windows, `%LOCALAPPDATA%\tilefab\history` replaces the last two. On Unix, it is private to the user (mode `0600`). `TILEFAB_HISTORY= cargo run` runs without a history file. The headless mode has no history.

**Search.** Ctrl+R replaces the prompt with `(search) query: ` and shows the newest entry that contains what is typed (case-insensitive unless the query has an upper-case letter), the match underlined. Ctrl+R / Ctrl+S go to older / newer matches; `(failing search)` means there is no other match. Enter runs the match; Esc or an arrow key keeps it in the line to edit it; Ctrl+G or Ctrl+C cancels.

**Hints.** While typing, the rest of the newest matching history entry is shown dim after the line: → / End / Ctrl+F accepts it, Alt+F accepts one word, Esc hides it.

**Completion.** Tab completes the command name (`st` → `start `). When several commands match, Tab inserts their common part, then a second Tab opens a menu below the prompt (the viewport grows to show it): Tab/↓ and Shift+Tab/↑ select, ←/→ change column, Enter keeps the selection without running the line, typing goes on after it, Esc restores the line. When there is nothing left to complete (for instance after `start `), Tab accepts the whole hint, like →.

**Paste.** The application turns bracketed paste on: a pasted text is inserted on the prompt line in one go (line breaks and tabs become spaces) and nothing runs before Enter. In a terminal without bracketed paste, pasted text arrives as typed keys and each line break runs a line.

Terminal notes:

- on macOS, the Alt shortcuts need "Use Option as Meta key" (Terminal.app, Profiles › Keyboard) or "Option key: Esc+" (iTerm2); otherwise Option+B types `∫`, which is inserted;
- many terminals send Ctrl+Backspace as Ctrl+H, which deletes one character;
- Ctrl+Z is undo, not suspend, and Ctrl+S is free (forward search): raw mode turns off the terminal's `ISIG` and `IXON`.

## Architecture

A single UI loop owns the `Terminal`; nothing else writes to stdout.

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
| `apps/widget-demo/src/wrap.rs` | wrapping of the long log lines inserted above the viewport |
| `apps/widget-demo/src/logging.rs` | tracing `MakeWriter` that splits output into lines and sends them into the channel |
| `apps/widget-demo/src/tasks.rs` | simulated tasks (`tokio::spawn` + `interval`) |
| `apps/widget-demo/src/headless.rs` | degraded mode without a TTY |
| `crates/tilefab-*`, `crates/tilefab` | the library: see below |
| `crates/tilefab/examples/` | `fullscreen.rs` (full screen), `inline.rs` (variable-height viewport) and `cell_alloc.rs` (allocations and time per frame: `cargo run --release --example cell_alloc`) |
| `tools/` | pty harness (`pty_harness.py`), checks (`check_app.py`, `check_inline.py`), video recording and analysis (`record_check.sh`, `frames_check.py`) |

### The library: one Cargo workspace, one crate per layer

| Crate | Role | Depends on |
|---|---|---|
| `crates/tilefab-core` | `Cell`, `Buffer` and diff between two buffers (wide characters included); `Color`, `Modifier`, `Style`; `Span`, `Line`, `Text`, `Alignment`, `Stylize`; `Rect`, `Position`, `Size`, `Margin`, `Constraint`, `Layout`; the `Widget` and `StatefulWidget` traits (text types are widgets) | — (no crossterm) |
| `crates/tilefab-term` | `backend`: ANSI sequences on a `W: Write` (cells, cursor, erasing, scroll regions); `terminal`: `Terminal` and `Frame` (diff with the previous frame, cursor, viewport); `viewport`: `Viewport` (full screen or inline), `ScrollMode`, commit placement rule; `init`: `TerminalGuard` (raw mode, alternate screen), panic hook | `tilefab-core`, crossterm |
| `crates/tilefab-widget` | `LineWidget`: a one-line widget composed from a description string and lists of values, laid out again at every draw; drawn in a `Buffer` without I/O | `tilefab-core` |
| `crates/tilefab-input` | `LineEditor`: prompt line editor (keymap, kill ring, undo, horizontal scrolling), without terminal I/O | crossterm (events), `tilefab-core` |
| `crates/tilefab` | façade: re-exports all of the above under `tilefab::…`; features `widget` and `input` (on by default) | the four crates above |
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
- **Widgets**: a single one, `LineWidget`, always one line. No blocks, paragraphs, lists, tables, charts, or mouse support. The library does not wrap text: the demo cuts its long log lines itself (`wrap.rs`).
- **Viewport**: the height of an inline viewport can change from one frame to the next (`set_height`).

## Tests and checks

`tools/ci.sh` runs all the checks below, in the same way as the CI workflow (`.github/workflows/ci.yml`), with `--locked` so that a `Cargo.lock` out of sync with `Cargo.toml` fails; it requires `cargo-deny` 0.21.0, the version the CI installs:

```sh
cargo fmt --check
cargo test                      # library (buffer, text, layout, line widget, backend, viewport) and application
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --release --all-targets --all-features -- -D warnings
cargo test --profile ci       # release build with overflow checks: the debug-only `panic` command is absent
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo deny check                # advisories, bans, licenses (see deny.toml), sources
cargo deny --manifest-path fuzz/Cargo.toml check   # the same for the fuzz crate (own Cargo.lock)
```

The workflow `.github/workflows/advisories.yml` checks the RustSec advisories again every Monday, for both lock files, so that an advisory published on a dependency already locked does not wait for the next push; it can also be started by hand from the Actions tab.

The Rust toolchain is pinned in `rust-toolchain.toml` (rustc, rustfmt and clippy): local runs and CI use the same version, so a new stable release cannot break CI with new clippy lints on its own. To move to a new version, change `channel` there, run `rustup install` then `tools/ci.sh`, and fix what the new lints report.

The GitHub Actions of the CI are pinned by commit SHA (version in a comment). Dependabot (`.github/dependabot.yml`) checks them every week and opens a single pull request that updates the SHA and the comment; it does not touch the Cargo dependencies.

The minimum supported Rust version is **1.88** (`rust-version` in the root `Cargo.toml`). The CI job `msrv` builds and tests with it; locally: `rustup toolchain install 1.88 --profile minimal && cargo +1.88 test --all-targets --all-features`.

The CI job `platforms` runs clippy and the tests on Windows and macOS: the code under `cfg(windows)` is compiled nowhere else. Locally, the Windows code can at least be linted: `rustup target add x86_64-pc-windows-msvc && cargo clippy --target x86_64-pc-windows-msvc --all-targets --all-features -- -D warnings`.

End to end, in a pty with a terminal emulator (local only, not part of the CI; once: `python3 -m venv tools/.venv && tools/.venv/bin/pip install -r tools/requirements.txt`):

```sh
cargo build --examples && cargo build
tools/.venv/bin/python tools/check_inline.py   # height cycle 1 → 5 → 1 with logs: a single synchronized block per commit,
                                               # no ESC[J / ESC[2J, no log lost or duplicated, prompt not moving
tools/.venv/bin/python tools/check_app.py      # Unicode input, burst of 5 tasks, resize, zero width, zero height, line editor (history, completion menu, search, paste), signals (and a second signal on a stuck UI), panic, degraded mode
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

On the line widget and the line editor, every mutant is caught by the tests (none is marked as equivalent). The timeouts are mutants that loop forever: the tests catch them too.

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

The corpus and the crashes stay local (`fuzz/corpus/`, `fuzz/artifacts/`). `tools/ci.sh` only checks that the targets build and pass clippy.

The frame commit tests (`crates/tilefab-term/src/inline_tests.rs`) replay the emitted bytes in a small terminal simulator (`crates/tilefab-term/src/sim.rs`): they cover every case of the placement rule, both scroll modes, lines inserted in batches and resizing.

`tools/record_check.sh` films the Terminal.app or iTerm2 window and `tools/frames_check.py` analyzes the video frame by frame (transient frames, blank lines, needless rewrites). It requires the macOS "Screen Recording" and "Automation" permissions.

## Known limits

- **Window shrinking**: Terminal.app and iTerm2 rewrap the existing lines when the window shrinks, which makes the viewport position uncertain. The cursor position is queried, everything from the top of the viewport to the bottom of the screen is erased (`ESC[J`, the only case besides exit) and everything is redrawn: copies of the old viewport may remain above, and log lines already displayed may disappear from the screen (depending on the terminal, they may or may not stay in the scrollback).
- **Terminal as short as the viewport**: with no room for log lines, they wait as long as the viewport fills the whole screen, and are shown once tasks end and the viewport shrinks. At most `MAX_PENDING_LINES` (1,000) lines wait: beyond that, the oldest are dropped, with a warning.
- **Maximum height**: `MAX_TASKS + 1` lines (6); below 6 terminal lines, the viewport is capped to the screen height.
- **A task ending with fewer log lines than freed lines**: a blank line may appear below the viewport (case `d > n`). In `tilefab` every task end produces a line, so this does not happen.
- **Synchronized update** (mode 2026): terminals that do not know it ignore it; the frame is then displayed as it is written (still in a single `write`).
- **Long log lines**: they are wrapped to the width at that moment; widening the window afterwards does not "unwrap" them.
- **Emoji width**: computed with `unicode-width`; some terminals display complex sequences differently (emoji with modifiers, flags), which can shift the cursor.
- **After a panic**, the viewport content (bars, prompt) stays on screen above the error report; the terminal itself is restored.
- **Degraded mode**: no task lines; Ctrl+C there is the standard SIGINT signal (immediate stop).
- **Signals** (`kill` from another process: SIGTERM, SIGINT, SIGHUP): the first one quits through the normal clean-up, with exit code 128 + the signal number (143 for SIGTERM). If the UI is stuck (a terminal that reads nothing), a second signal ends the application at once: raw mode is disabled, the rest of the restoration is tried for half a second.
- **Windows**: untested (`Release` events and rendering in Windows Terminal to be checked).

## License

MIT: see [LICENSE](LICENSE).
