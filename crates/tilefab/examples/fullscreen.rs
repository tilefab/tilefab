//! Fullscreen demo of the small `tilefab` library: border, wrapped paragraph, animated progress bar.
//!
//! Run: `cargo run --example fullscreen`. `q`, Escape, or Ctrl+C quits.
//! Resize the window: everything is recalculated and redrawn.

use std::io::{self, BufWriter};
use std::time::{Duration, Instant};

use tilefab::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use tilefab::{
    Backend, Block, Color, Constraint, Layout, Paragraph, ProgressBar, Stylize, Terminal,
    TerminalGuard, Wrap, install_panic_hook,
};

const TEXT: &str = "This is a long paragraph, automatically wrapped to the width of the \
window. It contains accents (été, où, à), wide characters (日本語, 😀) and enough words \
to span several lines. Resize the window to see the wrapping change.";

fn main() -> io::Result<()> {
    // The panic hook restores the terminal before displaying the report.
    install_panic_hook();
    // The guard enables raw mode and the alternate screen; its `Drop` undoes both.
    let _guard = TerminalGuard::fullscreen()?;
    // BufWriter: the entire frame is sent in a single `write` on `flush`.
    let mut terminal = Terminal::new(Backend::new(BufWriter::new(io::stdout())))?;

    let start = Instant::now();
    loop {
        // Progress: 0 → 100% in 10 seconds, then repeat.
        let elapsed = start.elapsed().as_secs_f64();
        let ratio = (elapsed % 10.0) / 10.0;

        terminal.draw(|frame| {
            let [title, body, bar, help] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Fill(1),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .areas(frame.area());

            frame.render_widget("Full-screen demo".bold().cyan(), title);
            frame.render_widget(
                Paragraph::new(TEXT)
                    .wrap(Wrap { trim: true })
                    .block(Block::bordered().title(" Text ")),
                body,
            );
            frame.render_widget(
                ProgressBar::default()
                    .block(Block::bordered().title(" Progress "))
                    .filled_style(Color::Green)
                    .ratio(ratio),
                bar,
            );
            frame.render_widget("q: quit".fg(Color::DarkGray), help);
        })?;

        // Wait at most 50 ms for a key: this also sets the animation rate.
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    _ => {}
                },
                // A `Resize` needs no handling: the next `draw` reads the size again.
                _ => {}
            }
        }
    }
    Ok(())
}
