# tilefab

A small terminal UI library for Rust, built on [crossterm](https://crates.io/crates/crossterm),
for programs that show **live status below their output**: progress bars, a status line or a
prompt stay at the bottom of the terminal, while log lines scroll above them and remain in the
terminal history. No alternate screen, no flicker.

```text
> start beta
[INFO] task "alpha" started (duration 6.3 s)
[INFO] task "beta" started (duration 5.5 s)
alpha [05.123] 🟠 ███████████████░░░░░░░░░░░░░░░░░░░░░░░░░░  36% compiling
beta  [05.123] 🟠 ██████████████████░░░░░░░░░░░░░░░░░░░░░░░  42% compiling

> start gam█
```

## Getting started

Add the library to a project:

```sh
cargo add tilefab
```

> tilefab is not on crates.io yet. Until its first release:
> `cargo add tilefab --git https://github.com/tilefab/tilefab`.

A complete program: a progress bar at the bottom of the terminal, log lines above it.

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

From a clone of this repository, it runs with `cargo run -p quickstart`.

## Documentation

| Document | For |
|---|---|
| [QUICKSTART.md](crates/tilefab/QUICKSTART.md) | a first program, explained step by step |
| [GUIDE.md](crates/tilefab/GUIDE.md) | every feature, with examples: several rows, keyboard and threads, the prompt, logs from `tracing`, running in a pipe, fullscreen, tests, known limits |
| [crates/tilefab/README.md](crates/tilefab/README.md) | the crate page: features and Cargo features |
| `cargo doc --open -p tilefab` | the API reference |
| [CONTRIBUTING.md](CONTRIBUTING.md) | the architecture, and how to build and check the project |

## In this repository

| Path | Contents |
|---|---|
| `crates/tilefab` | the library: the crate to depend on |
| `crates/tilefab-core`, `-term`, `-widget`, `-input` | its internal layers, re-exported by `tilefab` |
| `crates/tilefab/examples` | `inline` (a viewport that grows and shrinks) and `fullscreen`: `cargo run --example inline` |
| `apps/quickstart` | the program above |
| `apps/widget-demo` | a complete demo: tasks drawn with line widgets above a prompt, see its [README](apps/widget-demo/README.md) |
| `tools`, `fuzz` | end-to-end checks in a pseudo-terminal, video analysis, fuzzing: see [CONTRIBUTING.md](CONTRIBUTING.md) |

## The demo

```sh
cargo run
```

`cargo run` starts `widget-demo`. Type `start build` to run a simulated task, `help` for the
other commands, `quit` or Ctrl+C to leave. Its [README](apps/widget-demo/README.md) describes
the commands, the line compositions and the editing keys.

## License

MIT: see [LICENSE](LICENSE).
