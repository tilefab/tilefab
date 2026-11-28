# Quickstart

From an empty project to a running inline interface in a few minutes: a progress bar at the
bottom of the terminal, log lines scrolling above it, and a clean exit.

## 1. Create a project

```sh
cargo new hello-tilefab
cd hello-tilefab
cargo add tilefab
```

> tilefab is not on crates.io yet. Until its first release, add it from the repository instead:
> `cargo add tilefab --git https://github.com/tilefab/tilefab`.

## 2. Write the program

Replace `src/main.rs` with:

```rust,no_run
use std::time::Duration;

use tilefab::crossterm::event::KeyCode;
use tilefab::{Entry, LineWidget, is_ctrl_c, poll_key};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Set the terminal up: a viewport of one row at the bottom of the screen.
    let mut terminal = tilefab::init_inline(1)?;

    // 2. The row: a name, a space, a progress bar using the free columns, a space, a percentage.
    let mut bar = LineWidget::builder("N B0 I B0 P")
        .names([Entry::new("download")])
        .barriers([Entry::new(" ")])
        .build()?;

    for percent in 0..=100 {
        // 3. A log line every 10 %: printed above the viewport, it stays in the history.
        if percent % 10 == 0 {
            terminal.insert_line(format!("[INFO] {percent}% downloaded"));
        }

        // 4. Draw a frame: the queued log lines, then the viewport.
        bar.set_progress(f64::from(percent))?;
        terminal.draw(|frame| frame.render_widget(&bar, frame.area()))?;

        // 5. Wait a little, and quit early on `q` or Ctrl+C.
        if let Some(key) = poll_key(Duration::from_millis(50))?
            && (key.code == KeyCode::Char('q') || is_ctrl_c(&key))
        {
            break;
        }
    }

    // 6. When `terminal` is dropped, the viewport is cleared and the terminal restored.
    Ok(())
}
```

## 3. Run it

```sh
cargo run
```

```text
[INFO] 0% downloaded
[INFO] 10% downloaded
[INFO] 20% downloaded
download ████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  28%
```

The log lines pile up above the bar and stay in the terminal history: scroll up after the
program ends, they are still there. Press `q` or Ctrl+C to stop early.

## What each step does

1. **`tilefab::init_inline(1)`** prepares the terminal and returns a `DefaultTerminal`. The
   *viewport*, the area the program redraws, is one row at the bottom of the normal screen:
   no alternate screen, so the shell history stays visible above it. The call also switches
   the terminal to raw mode and installs a panic hook that restores it. It fails with a clear
   error when the output is not a terminal (a pipe or a file).
2. **`LineWidget::builder("N B0 I B0 P")`** describes a row with a short *composition*, one
   token per component: `N` a name taken from `names`, `B0` the first barrier (a separator)
   taken from `barriers`, `I` a progress bar that takes the free columns, `P` the percentage.
   `build` checks the composition and the lists.
3. **`terminal.insert_line(…)`** queues a line to print above the viewport. Nothing is written
   yet. A line wider than the terminal is wrapped, never cut.
4. **`terminal.draw(…)`** writes one frame: the queued lines, then the viewport, in a single
   write. Only the cells that changed since the last frame are sent, so drawing often is
   cheap.
5. **`poll_key(…)`** waits at most 50 ms for a key press. In raw mode, Ctrl+C does not stop the
   program: it arrives as a key, which `is_ctrl_c` recognizes.
6. **Dropping the terminal** clears the viewport and restores the terminal: the shell prompt
   comes back where the bar was, below the log lines.

## Try a few changes

Another look for the row: a bold name, a 30-cell bar drawn as a green line on a dark gray
track, a bold green percentage. Paints such as `.G/K+` and looks such as `L` are described
in the [guide](GUIDE.md#the-line-widget).

```rust
use tilefab::{BuildError, Entry, LineWidget};

fn download_bar() -> Result<LineWidget, BuildError> {
    LineWidget::builder("N.* B0 I30L.G/K+ B0 P.G*")
        .names([Entry::new("download")])
        .barriers([Entry::new(" ")])
        .build()
}
```

A colored log line: `insert_line` takes a `&str`, a `String`, a `Span` or a `Line` made of
styled spans.

```rust
use tilefab::{Color, DefaultTerminal, Line, Span, Style};

fn warn(terminal: &mut DefaultTerminal, text: &str) {
    terminal.insert_line(Line::from(vec![
        Span::styled("[WARN] ", Style::new().fg(Color::Yellow)),
        Span::raw(text),
    ]));
}
```

## Where to go next

- The [guide](GUIDE.md) covers everything else: several rows and a variable height, a prompt
  with history and completion, logs from `tracing`, background threads, fullscreen mode,
  running without a terminal, and tests.
- This program is also in the repository, as `apps/quickstart`: `cargo run -p quickstart`.
- `cargo run --example inline` shows a viewport that grows and shrinks, and the `widget-demo`
  application (`cargo run`) a complete program with tasks and a prompt.
