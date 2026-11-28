# Guide

This guide covers everything an application needs from tilefab, one topic per section. Each
example compiles on its own (they are checked by `cargo test`). New to tilefab? Start with the
[quickstart](QUICKSTART.md): a complete program in thirty lines.

- [How tilefab draws](#how-tilefab-draws)
- [Setting the terminal up](#setting-the-terminal-up)
- [Drawing a frame](#drawing-a-frame)
- [Printing above the viewport](#printing-above-the-viewport)
- [Several rows and a variable height](#several-rows-and-a-variable-height)
- [The line widget](#the-line-widget)
- [Keyboard input](#keyboard-input)
- [A real event loop: background work and threads](#a-real-event-loop-background-work-and-threads)
- [A prompt with the line editor](#a-prompt-with-the-line-editor)
- [Logs from `tracing`](#logs-from-tracing)
- [Running without a terminal](#running-without-a-terminal)
- [Fullscreen applications](#fullscreen-applications)
- [Testing](#testing)
- [Features, dependencies and versions](#features-dependencies-and-versions)
- [Known limits](#known-limits)

## How tilefab draws

An **inline viewport** is a band of rows at the bottom of the normal screen, which the program
redraws as often as it likes. Everything above it is ordinary terminal output: lines printed
there scroll up and stay in the terminal history, as if the program had used `println!`. The
classic use is a status area (progress bars, a prompt) below a stream of log lines.

Three rules follow from that design:

- **One owner.** While the viewport is active, only the `Terminal` writes to the screen. A
  `println!` or `eprintln!` from anywhere else would land in the middle of the viewport. Send
  the text to the code that owns the terminal, and print it with `insert_line`.
- **Nothing is written until `draw`.** `insert_line`, `insert_lines`, `insert_before` and
  `set_height` only queue a change. The next `draw` applies all of them, and redraws the
  viewport, in a single synchronized write: the terminal never shows a half-drawn frame.
- **Only what changed is sent.** `draw` compares the new frame with the previous one and
  writes the cells that differ. A frame that changes nothing writes nothing, so a loop can
  call `draw` many times per second.

At the bottom of the screen, the last row of the viewport stays in place while lines are
printed and the height grows: it is the natural place for a prompt. Shrinking keeps it in
place too, when lines printed in the same frame fill the freed rows (see
[Several rows and a variable height](#several-rows-and-a-variable-height)).

## Setting the terminal up

Three functions set everything up in one call, and return a `DefaultTerminal`:

| Function | Viewport |
|---|---|
| `tilefab::init_inline(height)` | `height` rows at the bottom of the normal screen |
| `tilefab::init_fullscreen()` | the alternate screen, all of it (see [Fullscreen applications](#fullscreen-applications)) |
| `tilefab::init_with_options(options)` | either, with the other `TerminalOptions` (the scroll mode) |

Each one:

1. checks that stdin and stdout are terminals, and returns an `io::ErrorKind::Unsupported` error
   otherwise (see [Running without a terminal](#running-without-a-terminal));
2. installs, once, a panic hook that restores the terminal before the panic report is printed;
3. switches the terminal to raw mode: keys arrive one by one, unechoed, and Ctrl+C is a key
   rather than a signal;
4. creates the terminal on a buffered stdout, and reserves the viewport rows.

A `DefaultTerminal` dereferences to `Terminal`: every method below is called on it directly.
When it is dropped, it clears the viewport and restores the terminal; the log lines stay
above, and the shell prompt comes back where the viewport was. To see an error of that last
write, call `finish` instead of letting it drop:

```rust,no_run
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = tilefab::init_inline(1)?;
    terminal.insert_line("hello");
    terminal.draw(|_frame| {})?;
    terminal.finish()?;
    Ok(())
}
```

**Panic hooks.** The hook installed by `init_*` runs before the hook that was there already.
To get a readable report from `color_eyre`, call `color_eyre::install()` first. A panic in a
background thread or a `tokio` task does not stop the program: it keeps running on a terminal
already restored. To make every panic end the process, call `tilefab::install_fatal_panic_hook()`
before `init_*`.

**The building blocks.** `init_*` gathers public pieces, which an application can also put
together itself, for instance to draw on stderr while stdout carries data:

```rust,no_run
use std::io::{self, BufWriter};

use tilefab::{Backend, Terminal, TerminalGuard, TerminalOptions, Viewport};

fn main() -> io::Result<()> {
    tilefab::install_panic_hook();
    // Raw mode, undone when the guard is dropped. Declared before the terminal, so that it
    // is dropped after it.
    let _guard = TerminalGuard::raw()?;
    let mut terminal = Terminal::with_options(
        Backend::new(BufWriter::new(io::stderr())),
        TerminalOptions::default().viewport(Viewport::Inline(1)),
    )?;
    terminal.insert_line("drawn on stderr");
    terminal.draw(|_frame| {})?;
    // Without `DefaultTerminal`, clearing the viewport is the application's job.
    terminal.clear()
}
```

Only one `TerminalGuard` (and so one `DefaultTerminal`) can be alive at a time: a second one
returns an `io::ErrorKind::AlreadyExists` error.

## Drawing a frame

`draw` takes a closure that receives a `Frame`. The frame covers the viewport:

| `Frame` method | Use |
|---|---|
| `area()` | the viewport rectangle, in screen coordinates |
| `render_widget(widget, area)` | draw a widget (a `LineWidget`, a `Line`, a `Span`, a `Text`…) in `area` |
| `buffer_mut()` | the cells themselves: `set_string`, `get_mut`… |
| `set_cursor_position(position)` | show the cursor there after the frame; without it, the cursor is hidden |

Text is built from `Span`s (a string and a `Style`), gathered in a `Line`. The `Stylize` trait
adds shortcuts such as `.bold()`, `.dim()` or `.fg(Color::Cyan)`:

```rust,no_run
use tilefab::{Color, DefaultTerminal, Line, Span, Style, Stylize};

fn draw_status(terminal: &mut DefaultTerminal, done: u32, total: u32) -> std::io::Result<()> {
    terminal.draw(|frame| {
        let status = Line::from(vec![
            Span::styled(format!("{done}/{total}"), Style::new().fg(Color::Green)),
            Span::raw(" files copied"),
        ])
        .bold();
        frame.render_widget(status, frame.area());
    })
}
```

## Printing above the viewport

`insert_line` queues one line, `insert_lines` several. They take anything that converts into
a `Line`: a `&str`, a `String`, a `Span`, or a `Line` of styled spans. Each line is wrapped at
the terminal width, on character boundaries (a wide character is never split), so that
nothing is cut.

```rust
use tilefab::{Color, DefaultTerminal, Line, Span, Style};

fn report(terminal: &mut DefaultTerminal, errors: &[String]) {
    terminal.insert_line(Line::from(vec![
        Span::styled("[ERROR] ", Style::new().fg(Color::Red)),
        Span::raw(format!("{} file(s) failed", errors.len())),
    ]));
    terminal.insert_lines(errors.iter().map(|error| format!("  - {error}")));
}
```

The lines are written by the next `draw`. At most `MAX_PENDING_LINES` (1,000) rows are kept
from one call: beyond that, the oldest are dropped, with a warning. When the viewport fills
the whole screen, there is no room above it: the lines wait, and appear once the viewport
shrinks or the window grows.

For rows drawn by hand, `insert_before(height, render)` gives a buffer of `height` rows, as
wide as the screen, to fill. `tilefab::wrap::wrap_lines` cuts lines into rows the same way as
`insert_lines`, for an application that wants to count or keep them.

```rust
use tilefab::{DefaultTerminal, Style};

fn separator(terminal: &mut DefaultTerminal) {
    let width = usize::from(terminal.area().width);
    terminal.insert_before(1, |buf| {
        buf.set_string(0, 0, &"─".repeat(width), Style::new());
    });
}
```

## Several rows and a variable height

`set_height` changes the number of rows of the viewport; the next `draw` applies it. A
`Layout` splits the viewport into rows:

```rust
use tilefab::convert::saturating_u16;
use tilefab::{Constraint, DefaultTerminal, Layout, Line, LineWidget, Stylize};

/// One row per task, then a status row.
fn draw_tasks(
    terminal: &mut DefaultTerminal,
    tasks: &[LineWidget],
    status: &str,
) -> std::io::Result<()> {
    terminal.set_height(saturating_u16(tasks.len() + 1));
    terminal.draw(|frame| {
        let area = frame.area();
        let rows = Layout::vertical(vec![Constraint::Length(1); usize::from(area.height)])
            .split(area);
        let (status_row, task_rows) = rows.split_last().expect("at least one row");
        for (row, task) in task_rows.iter().zip(tasks) {
            frame.render_widget(task, *row);
        }
        frame.render_widget(Line::from(status).dim(), *status_row);
    })
}
```

The height is kept between 1 and the screen height. The viewport always sits just below the
lines printed last. At the bottom of the screen, growing scrolls the lines above it up, so its
last row stays in place.

**Shrink together with a log line.** A smaller viewport moves up to stay just below the printed
lines, and the rows it frees at the bottom are cleared. Lines printed in the same frame fill
those rows instead: with as many lines as freed rows, the last row of the viewport does not
move, and no blank row appears. The natural way is to print the line that explains the change
in the frame that makes it:

```rust
use tilefab::convert::saturating_u16;
use tilefab::{DefaultTerminal, LineWidget};

fn task_finished(terminal: &mut DefaultTerminal, tasks: &mut Vec<LineWidget>, index: usize) {
    tasks.remove(index);
    // Both changes are applied by the same `draw`: the line takes the freed row.
    terminal.insert_line(format!("[INFO] task {index} finished"));
    terminal.set_height(saturating_u16(tasks.len() + 1));
}
```

## The line widget

`LineWidget` draws one row from a short **composition**: tokens separated by one space, in
display order. The values come from lists given to the builder (`symbols`, `barriers`,
`keywords`, `names`, `units`), and from setters called while the program runs.

| Token | Component | Parameter |
|---|---|---|
| `S` | a symbol of the `symbols` list (one character, 1 or 2 columns) | `S3`: always the fourth; `S`: chosen by `set_symbol` |
| `B` | a barrier of the `barriers` list: any text, typically a separator | index, required: `B0` |
| `D` | date: `D0` `YYYY-MM-DD`, `D1` `YY-MM-DD`, `D2` `MM-DD` | format, required |
| `T` | time: `T0` `HH:MM:SS.SSS`, `T1` `MM:SS.SSS`, `T2` `SS.SSS`, `T3` `HH:MM:SS`, `T4` `HH:MM`, `T5` `MM:SS` | format, required |
| `P` | progress as a number and `%`, rounded down | width 3 to 9 (`P3` by default; decimals from `P5`) |
| `K` | a keyword of the `keywords` list, padded to the longest | `K3`: fixed; `K`: chosen by `set_keyword` |
| `N` | a name of the `names` list | `N14`: fixed; `N`: chosen by `set_name` |
| `L` | free text, set by `set_log` | `L40`: 40 columns; `L`: takes the free columns |
| `V` | a value, with a unit (`U`) and a total (`/`): `12.5MB/40.0MB` | `V5U/`: width 1 to 999 (5 by default) |
| `I` | a bar showing the progress | `I20`: 20 columns; `I`: takes the free columns; then a look |

`D`, `T`, `P`, `K`, `L`, `V` and `I` appear at most once; `S`, `B` and `N` can be repeated.
The **looks** of `I` are one letter after it (`I20L`, `IL`): `B` bar `█░` (the default), `L` line
`━─`, `D` dots `●·`, `S` squares `■□`, `O` circles `●○`, `R` diamonds `◆◇`, `G` segments `▰▱`, `H`
shaded blocks `▓░`, `V` vertical bars `┃│`, `A` ASCII `#-`, `C` chevrons `>·`. Without a look,
`illustration` gives the two characters.

A **paint** changes the style of one component: `.`, then at least one of, in this order, a
text color, `:` and a background color, `*` for bold. Colors: `K` black, `R` red, `G` green, `Y`
yellow, `B` blue, `M` magenta, `C` cyan, `W` white, with `+` for the bright version (`K+` is dark
gray). `I` and `V` take a second paint after `/`, for the rest of the bar and for the units.
For instance `N.W+:B*` is a bright white bold name on blue, `I20L.G/K+` a green line on a dark
gray track. Without paints, the styles come from the entries (`Entry::styled`) and the
`*_style` methods of the builder.

```rust
use tilefab::{BuildError, Color, Entry, LineWidget, Rank, Style, UpdateError};

/// `● build      ████████░░░░░░░░  48% compiling`
fn task_row(name: &str) -> Result<LineWidget, BuildError> {
    LineWidget::builder("S B0 N12 B0 I B0 P B0 L")
        .symbols([
            Entry::styled("●", Style::new().fg(Color::Yellow)),
            Entry::styled("✔", Style::new().fg(Color::Green)),
        ])
        .barriers([Entry::new(" ")])
        .names([Entry::new(name)])
        .build()
}

fn update(row: &mut LineWidget, percent: f64, step: &str) -> Result<(), UpdateError> {
    row.set_progress(percent)?;
    row.set_log(step);
    if percent >= 100.0 {
        // The first dynamic `S` of the row shows the second symbol.
        row.set_symbol(Rank(0), 1)?;
    }
    Ok(())
}
```

**Layout.** The row is laid out again at every draw, at the width it gets. The extra columns
go to an elastic `L`, otherwise to an elastic `I`. When the row is too wide, `L` and `I` shrink
first (down to 2 columns, a cut text ends with `…`), then the names, then the last components
are dropped. A narrow terminal never causes an error.

**Errors.** `build` returns a `BuildError` for a composition or a list that cannot be drawn,
with the position of the faulty token. `warnings()` lists problems that do not prevent drawing
(for now, a row wider than its `target_width`, 80 by default). A setter returns an
`UpdateError` for an invalid call (an index outside its list, a progress that is not a
number…) and keeps the previous value.

**Date and time.** `D` and `T` show a `WallTime`, given by `set_time`: tilefab has no clock and
no time-zone database. Fill its fields from the clock crate of your choice (`jiff`, `chrono`,
`time`).

## Keyboard input

For a loop that draws between key presses, `poll_key(timeout)` waits at most `timeout` for a
key press and returns it, or `None`. It skips key releases, so that each key counts once, and
drops the other events (pastes, resizes, mouse). A resize needs nothing: `draw` checks the
screen size by itself.

Raw mode turns the terminal signals off: Ctrl+C arrives as a key. Recognize it with
`is_ctrl_c`, or the user has no way to stop the program.

```rust,no_run
use std::time::Duration;

use tilefab::crossterm::event::KeyCode;
use tilefab::{DefaultTerminal, is_ctrl_c, poll_key};

/// Wait for a key; true when the user wants to quit.
fn quit_requested(terminal: &mut DefaultTerminal) -> std::io::Result<bool> {
    terminal.draw(|_frame| {})?;
    Ok(poll_key(Duration::from_millis(100))?
        .is_some_and(|key| is_ctrl_c(&key) || key.code == KeyCode::Esc))
}
```

The key events are crossterm's, re-exported as `tilefab::crossterm`: use that path rather
than a separate `crossterm` dependency, so that both versions always match.

**Pastes.** Without bracketed paste, a pasted text arrives as typed keys, and each line break
acts as Enter. To receive it as one `Event::Paste`, turn bracketed paste on after `init_*`; the
terminal restoration turns it off again. Read the events with `crossterm::event::read` (next
section), since `poll_key` drops pastes:

```rust,no_run
use std::io;

use tilefab::crossterm::event::EnableBracketedPaste;
use tilefab::crossterm::execute;

fn enable_paste() -> io::Result<()> {
    execute!(io::stdout(), EnableBracketedPaste)
}
```

## A real event loop: background work and threads

A program that does work while it waits for keys gathers everything in one channel, read by
the loop that owns the terminal: the keys from a keyboard thread, the progress from workers,
the log lines. Each frame then applies all the waiting events at once.

```rust,no_run
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use tilefab::crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use tilefab::{Entry, LineWidget, is_ctrl_c};

enum AppEvent {
    Key(KeyEvent),
    Progress(f64),
    Log(String),
}

fn spawn_keyboard_thread(tx: Sender<AppEvent>) {
    thread::spawn(move || {
        loop {
            // `poll` with a timeout, not a lone blocking `read`: see below.
            match event::poll(Duration::from_millis(50)) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => return,
            }
            let key = match event::read() {
                Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => key,
                Ok(_) => continue,
                Err(_) => return,
            };
            if tx.send(AppEvent::Key(key)).is_err() {
                return; // the UI loop has ended
            }
        }
    });
}

fn spawn_worker(tx: Sender<AppEvent>) {
    thread::spawn(move || {
        for percent in 0..=100 {
            thread::sleep(Duration::from_millis(30));
            if tx.send(AppEvent::Progress(f64::from(percent))).is_err() {
                return;
            }
        }
        let _ = tx.send(AppEvent::Log("[INFO] work done".to_owned()));
    });
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = tilefab::init_inline(1)?;
    let mut bar = LineWidget::builder("I B0 P")
        .barriers([Entry::new(" ")])
        .build()?;
    let (tx, rx) = mpsc::channel();
    spawn_keyboard_thread(tx.clone());
    spawn_worker(tx);

    loop {
        // Wait for one event, then take all the waiting ones: one frame for the batch.
        let first = rx.recv_timeout(Duration::from_millis(100)).ok();
        for app_event in first.into_iter().chain(rx.try_iter()) {
            match app_event {
                AppEvent::Key(key) if is_ctrl_c(&key) => return Ok(()),
                AppEvent::Key(_) => {}
                AppEvent::Progress(percent) => bar.set_progress(percent)?,
                AppEvent::Log(line) => terminal.insert_line(line),
            }
        }
        terminal.draw(|frame| frame.render_widget(&bar, frame.area()))?;
    }
}
```

**Why `poll` before `read`.** crossterm has a single input reader, locked for the whole time a
`read` waits. After a resize, the terminal asks for the cursor position, and that query needs
the same lock: with a thread blocked in `read`, it gives up after 2 seconds and the viewport
position is guessed. A `poll` with a short timeout releases the lock regularly.

**With tokio**, the shape is the same: a `tokio::sync::mpsc` channel, the keyboard reader in
`tokio::task::spawn_blocking` (or a thread), the workers in `tokio::spawn`, and the UI loop
awaiting `recv`. A bounded channel protects memory, but then a log written from the UI loop
itself must not wait for room (`try_send`), or the loop would wait for itself.

## A prompt with the line editor

`LineEditor` (feature `input`, on by default) edits one line: readline-style keys (Emacs
bindings, kill ring, undo), history, hints (the rest of a matching history entry, shown dim)
and completion. It draws into the frame and returns where the cursor goes; it does no input
or output itself.

```rust,no_run
use std::time::Duration;

use tilefab::{EditorConfig, EditorEvent, Line, LineEditor, poll_key};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = tilefab::init_inline(1)?;
    let mut editor = LineEditor::new(EditorConfig::default());
    let prompt = Line::from("> ");

    loop {
        // An open completion menu needs rows below the prompt.
        let width = terminal.area().width;
        terminal.set_height(editor.extra_rows(width).saturating_add(1));
        terminal.draw(|frame| {
            let area = frame.area();
            if let Some(cursor) = editor.render(area, frame.buffer_mut(), &prompt) {
                frame.set_cursor_position(cursor);
            }
        })?;

        let Some(key) = poll_key(Duration::from_millis(100))? else {
            continue;
        };
        match editor.handle_key(key) {
            EditorEvent::Submit(line) if line == "quit" => break,
            EditorEvent::Submit(line) => terminal.insert_line(format!("you typed: {line}")),
            // Ctrl+C, or Ctrl+D on an empty line.
            EditorEvent::Interrupt | EditorEvent::Eof => break,
            _ => {}
        }
    }
    Ok(())
}
```

`EditorEvent::Submit` carries the line and records it in the history. With tasks drawn above
the prompt, give the editor the last rows of the viewport (see
[Several rows and a variable height](#several-rows-and-a-variable-height)). A pasted text goes
in with `editor.paste(&text)`.

**History in a file.** `History::with_file` loads a file and appends each new line to it. It
returns the error of the load, if any; the history then works in memory. Later write errors
come once from `editor.take_history_error()`.

```rust
use std::io;
use std::path::Path;

use tilefab::{EditorConfig, History, LineEditor};

fn editor_with_history(path: &Path) -> (LineEditor, Option<io::Error>) {
    let config = EditorConfig::default();
    let (history, error) = History::with_file(path, config.history_capacity);
    (LineEditor::new(config).with_history(history), error)
}
```

**Completion.** A `Completer` proposes candidates for the text before the cursor. Tab inserts
the common part; a second Tab opens a menu below the prompt.

```rust
use tilefab::{Candidate, Completer, Completions, EditorConfig, LineEditor};

/// Completes the first word with a command name.
struct Commands;

impl Completer for Commands {
    fn complete(&self, line: &str, cursor: usize) -> Completions {
        let typed = &line[..cursor];
        if typed.contains(' ') {
            return Completions::default();
        }
        let candidates = ["help", "quit", "start", "status"]
            .into_iter()
            .filter(|name| name.starts_with(typed))
            .map(|name| Candidate::new(name).append_space())
            .collect();
        Completions { start: 0, candidates }
    }
}

fn editor() -> LineEditor {
    LineEditor::new(EditorConfig::default()).with_completer(Commands)
}
```

**Settings and keys.** `EditorConfig` sets the maximum line length, the history size, the hints,
the menu height and the styles, with chained setters from `EditorConfig::default()`. The keymap
can be changed key by key:

```rust
use tilefab::crossterm::event::{KeyCode, KeyModifiers};
use tilefab::{EditorCommand, KeyBinding, LineEditor};

/// Ctrl+L clears the whole line.
fn bind_ctrl_l(editor: &mut LineEditor) {
    if let Some(key) = KeyBinding::new(KeyCode::Char('l'), KeyModifiers::CONTROL) {
        let _previous = editor.keymap_mut().bind(key, EditorCommand::KillLine);
    }
}
```

The default keys are those of readline: the `widget-demo` README lists them.

## Logs from `tracing`

The logs of a program, and of its dependencies, must go through the terminal too (see
[How tilefab draws](#how-tilefab-draws)). With `tracing`, a writer sends each formatted event to
a channel, and the UI loop prints what it receives:

```rust,no_run
use std::io::{self, Write};
use std::sync::mpsc::{self, Sender};

/// Collects the text of one tracing event, and sends its lines when dropped.
struct LogWriter {
    tx: Sender<String>,
    text: Vec<u8>,
}

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.text.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LogWriter {
    fn drop(&mut self) {
        for line in String::from_utf8_lossy(&self.text).lines() {
            let _ = self.tx.send(line.to_owned());
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (tx, logs) = mpsc::channel();
    tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(move || LogWriter {
            tx: tx.clone(),
            text: Vec::new(),
        })
        .init();

    let mut terminal = tilefab::init_inline(1)?;
    tracing::info!("started");
    // In the UI loop, before each frame:
    terminal.insert_lines(logs.try_iter());
    terminal.draw(|_frame| {})?;
    Ok(())
}
```

`tilefab` itself logs a few warnings through `tracing` (lines dropped, a cursor query that
failed): they arrive the same way.

## Running without a terminal

When stdin or stdout is redirected (`my-app < commands.txt`, `my-app | tee log`), there is no
screen to draw on: `init_*` returns an `io::ErrorKind::Unsupported` error. A program that should
also work in a pipe or a script falls back to plain lines:

```rust,no_run
use std::io::{self, BufRead};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match tilefab::init_inline(1) {
        Ok(terminal) => run_interactive(terminal),
        Err(error) if error.kind() == io::ErrorKind::Unsupported => run_plain(),
        Err(error) => Err(error.into()),
    }
}

fn run_interactive(mut terminal: tilefab::DefaultTerminal) -> Result<(), Box<dyn std::error::Error>> {
    terminal.insert_line("interactive mode");
    terminal.draw(|_frame| {})?;
    Ok(())
}

/// One command per line on stdin, the logs on stderr.
fn run_plain() -> Result<(), Box<dyn std::error::Error>> {
    for line in io::stdin().lock().lines() {
        eprintln!("[INFO] command: {}", line?);
    }
    Ok(())
}
```

A terminal that does not answer the cursor position query (some minimal emulators) gives
another error, which the same fallback can handle.

## Fullscreen applications

`init_fullscreen` switches to the alternate screen: the program draws on the whole window, and
on exit the original screen comes back unchanged, with its history. The viewport is the whole
screen and follows its size; `insert_line` and `set_height` have no effect.

```rust,no_run
use std::time::Duration;

use tilefab::crossterm::event::KeyCode;
use tilefab::{Constraint, Layout, Line, Stylize, is_ctrl_c, poll_key};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = tilefab::init_fullscreen()?;
    loop {
        terminal.draw(|frame| {
            let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                .split(frame.area());
            frame.render_widget(Line::from("tilefab, fullscreen").bold(), rows[0]);
            frame.render_widget(Line::from("press q to quit"), rows[1]);
        })?;
        if let Some(key) = poll_key(Duration::from_millis(100))?
            && (key.code == KeyCode::Char('q') || is_ctrl_c(&key))
        {
            break;
        }
    }
    Ok(())
}
```

## Testing

Widgets draw into a `Buffer` without any terminal, so a test checks the text they produce:

```rust
use tilefab::{Buffer, Entry, LineWidget, Rect, Widget};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut bar = LineWidget::builder("N B0 I10 B0 P")
        .names([Entry::new("build")])
        .barriers([Entry::new(" ")])
        .build()?;
    bar.set_progress(50.0)?;

    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 1));
    (&bar).render(buf.area(), &mut buf);
    assert_eq!(buf.lines()[0].trim_end(), "build █████░░░░░  50%");
    Ok(())
}
```

To test the drawing code of the application itself, the `testing` feature gives a backend with
a fixed size and cursor, which writes into a `Vec<u8>`. Enable it in the dev-dependencies only:
`tilefab = { version = "0.1", features = ["testing"] }` under `[dev-dependencies]`.

```rust
use tilefab::{Backend, Line, Position, Size, Terminal, TerminalOptions, Viewport};

fn main() -> std::io::Result<()> {
    // A 20 × 5 screen, the cursor on its last row.
    let backend = Backend::with_fixed(Vec::new(), Size::new(20, 5), Position::new(0, 4));
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions::default().viewport(Viewport::Inline(1)),
    )?;
    terminal.insert_line("hello");
    terminal.draw(|frame| frame.render_widget(Line::from("> ready"), frame.area()))?;

    // What the viewport shows, and the bytes sent to the terminal.
    assert_eq!(terminal.buffer().lines()[0].trim_end(), "> ready");
    assert!(String::from_utf8_lossy(terminal.backend().writer()).contains("hello"));
    Ok(())
}
```

## Features, dependencies and versions

| Feature | Default | Adds |
|---|---|---|
| `widget` | on | `LineWidget` and its types |
| `input` | on | `LineEditor` and its types |
| `testing` | off | the test backend (`Backend::with_fixed`…): for dev-dependencies only |

For the terminal layer alone: `tilefab = { version = "0.1", default-features = false }`.

- **crossterm** is a public dependency, re-exported as `tilefab::crossterm`. Moving to another
  crossterm minor version is a breaking release of tilefab.
- **Growing types.** Options and configuration types (`TerminalOptions`, `EditorConfig`,
  `EditorStyles`, `Candidate`) and the enums that will grow (`EditorCommand`, `EditorEvent`,
  `BuildError`, `UpdateError`, `ClearType`) are `#[non_exhaustive]`: build them from `default()`
  with the chained setters, and give a `match` on them a `_` arm.
- **Minimum Rust version**: 1.88.
- **The `tilefab-*` crates** are the internal layers of `tilefab`, which re-exports all of their
  API: depend on `tilefab` only.

**Scroll modes.** By default (`ScrollMode::Screen`), room for the printed lines is made by
scrolling the whole screen, and every line reaches the terminal history in every terminal.
`ScrollMode::Region` writes less, using a scroll region, but some terminals (xterm.js in VS
Code, iTerm2 by default) then lose the lines from their history. `ScrollMode::Fallback` clears
the viewport before scrolling, for terminals that handle the rest poorly. Choose one with
`init_with_options(TerminalOptions::default().viewport(Viewport::Inline(1)).scroll_mode(…))`.

## Known limits

- **Shrinking the window.** Terminal.app and iTerm2 rewrap the existing lines when the window
  narrows, which moves the viewport by an unknown amount. tilefab asks the terminal for the
  cursor position, then clears from the top of the viewport to the bottom of the screen and
  redraws: copies of the old viewport may remain above, and lines already printed may leave
  the screen.
- **A viewport as tall as the screen.** There is no room for printed lines: they wait (at most
  `MAX_PENDING_LINES`, the oldest are dropped beyond that) until the viewport shrinks.
- **Synchronized update** (mode 2026). Terminals that do not support it ignore it: a frame is
  then displayed as it arrives, still in a single write.
- **Wrapped lines** keep the width they were printed at: widening the window does not unwrap
  them.
- **Emoji width** is computed with `unicode-width`. Some terminals display complex sequences
  (emoji with modifiers, flags) with another width, which shifts the rest of the row.
- **After a panic**, the viewport stays on screen above the report; the terminal itself is
  restored.
- **Huge screens.** At most `MAX_SCREEN_WIDTH` × `MAX_SCREEN_HEIGHT` cells (2048 × 1024) are drawn;
  the rest of a larger screen is left as it is.
- **Windows** is built and tested in CI, but not tried in a real terminal yet.
