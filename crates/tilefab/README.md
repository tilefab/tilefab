# tilefab

A small TUI library for Rust, built directly on [crossterm](https://crates.io/crates/crossterm), whose
specialty is the **inline viewport**: a fixed area at the bottom of the normal screen (no alternate
screen) whose height can change from one frame to the next, while log lines scroll above it and stay
in the terminal history.

- **Single frame commit**: inserted lines, the new viewport height and the redraw are written in one
  synchronized update; the terminal never shows an intermediate state, and the last viewport line
  (typically a prompt) does not move when the height changes.
- **Double buffering**: only the cells that changed are written.
- **Widgets** (feature `widget`): `Block`, `Paragraph`, `ProgressBar`, `ProgressLine`, `List`, `Clear`.
- **Line editor** (feature `input`): `LineEditor`, readline-style editing with history, hints,
  completion and horizontal scrolling.
- No `unsafe` code.

Key events are crossterm's: use the re-exported `tilefab::crossterm` rather than a separate
`crossterm` dependency, so that both versions always match.

Options and configuration types (`TerminalOptions`, `EditorConfig`, `EditorStyles`, `Candidate`)
and the enums that will grow (`EditorCommand`, `EditorEvent`, `BorderType`, `ClearType`) are
`#[non_exhaustive]`: build them from `default()` with the chained setters, and give a `match` a
`_` arm, so that new options and variants can come in minor versions.

## Example

```rust,no_run
use std::io::{self, BufWriter};
use std::{thread, time::Duration};

use tilefab::{Backend, ProgressLine, Style, Terminal, TerminalGuard, TerminalOptions, Viewport};

fn main() -> io::Result<()> {
    tilefab::install_panic_hook();
    // Raw mode only: no alternate screen, the log lines stay in the scrollback.
    let _guard = TerminalGuard::raw()?;
    let mut terminal = Terminal::with_options(
        Backend::new(BufWriter::new(io::stdout())),
        TerminalOptions::default().viewport(Viewport::Inline(1)),
    )?;

    for percent in (0..=100).step_by(10) {
        // Queued above the viewport, written by the next `draw` in the same frame.
        terminal.insert_before(1, |buf| {
            buf.set_string(0, 0, &format!("[INFO] step {percent}%"), Style::default());
        });
        terminal.draw(|frame| {
            let bar = ProgressLine::default().percent(percent).label("build");
            frame.render_widget(bar, frame.area());
        })?;
        thread::sleep(Duration::from_millis(200));
    }

    terminal.clear()?;
    Ok(())
}
```

## See it running

- The demo application (tasks with progress bars above a prompt):
  `cargo install widget-demo`, then run `widget-demo`.
- The examples of the repository, after a `git clone`:
  `cargo run --example inline` (variable-height viewport) and `cargo run --example fullscreen`.

## Features

| Feature | Default | Adds |
|---|---|---|
| `widget` | on | the widget library (`tilefab-widget`) |
| `input` | on | the prompt line editor (`tilefab-input`) |

For the terminal layer only: `tilefab = { version = "0.1", default-features = false }`.

## Crates

`tilefab` is the crate to depend on: it re-exports everything under `tilefab::…`. It is a façade over
`tilefab-core`, `tilefab-term`, `tilefab-widget` and `tilefab-input`, which are published only because
it needs them; their versions always move together.

| Layer | Re-exported here |
|---|---|
| `tilefab-core` | cells, buffer, styles, geometry and layout, styled text, the `Widget` traits; module `convert` |
| `tilefab-term` | `Backend`, `Terminal`, `Frame`, `TerminalGuard`, panic hooks; module `viewport` (placement rule of the inline viewport) |
| `tilefab-widget` (feature `widget`) | `Block`, `Paragraph`, `ProgressBar`, `ProgressLine`, `List`, `Clear` |
| `tilefab-input` (feature `input`) | `LineEditor` and its configuration; its `Command` is re-exported as `EditorCommand` |

## Minimum supported Rust version

Rust 1.88.

## License

MIT, see [LICENSE](https://github.com/tilefab/tilefab/blob/main/LICENSE).
