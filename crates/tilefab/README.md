# tilefab

A small terminal UI library for Rust, built on [crossterm](https://crates.io/crates/crossterm),
for programs that show **live status below their output**: progress bars, a status line or a
prompt stay at the bottom of the terminal, while log lines scroll above them and remain in the
terminal history. No alternate screen, no flicker.

```text
[INFO] 0% downloaded
[INFO] 10% downloaded
[INFO] 20% downloaded
download ████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  28%
```

## Example

```sh
cargo add tilefab
```

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

`init_inline` sets the terminal up and restores it on drop, `insert_line` prints above the
viewport, `draw` writes the frame. The [quickstart](https://github.com/tilefab/tilefab/blob/main/crates/tilefab/QUICKSTART.md)
explains each line.

## Features

- **Inline viewport** whose height can change at every frame, anchored at the bottom of the
  screen; printed lines are wrapped, never cut, and reach the terminal history.
- **One write per frame**: the printed lines, the new height and the redraw go out in a single
  synchronized update, with only the cells that changed.
- **`LineWidget`**: a status row described by a short composition (`"N B0 I B0 P"`: name, bar,
  percentage), laid out again at every frame to fit the terminal width.
- **`LineEditor`**: a readline-style prompt with history, hints and completion.
- Fullscreen mode too, a test backend, and no `unsafe` code.

## Documentation

- [Quickstart](https://github.com/tilefab/tilefab/blob/main/crates/tilefab/QUICKSTART.md): a first program, step by step.
- [Guide](https://github.com/tilefab/tilefab/blob/main/crates/tilefab/GUIDE.md): every feature, with examples: several rows,
  keyboard and threads, the prompt, logs from `tracing`, running in a pipe, tests.
- The API reference, on docs.rs.
- The repository's [`widget-demo`](https://github.com/tilefab/tilefab/blob/main/apps/widget-demo/README.md) application, a
  complete program built on tilefab.

## Cargo features

| Feature | Default | Adds |
|---|---|---|
| `widget` | on | `LineWidget` |
| `input` | on | `LineEditor` |
| `testing` | off | a test backend, for dev-dependencies |

`tilefab` re-exports crossterm as `tilefab::crossterm`: use it for key events rather than a
separate dependency, so that both versions match. The `tilefab-*` crates are its internal
layers: depend on `tilefab` only.

Minimum supported Rust version: 1.88.

## License

MIT, see [LICENSE](https://github.com/tilefab/tilefab/blob/main/LICENSE).
