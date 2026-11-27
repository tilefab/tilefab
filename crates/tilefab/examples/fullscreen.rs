//! Fullscreen demo of the small `tilefab` library: wrapped text and animated line widgets.
//!
//! Run: `cargo run --example fullscreen`. `q`, Escape, or Ctrl+C quits.
//! Resize the window: everything is recalculated and redrawn; when the window gets narrow,
//! the line widgets shrink their log and bar, then drop their last components.

use std::io::{self, BufWriter};
use std::time::{Duration, Instant};

use tilefab::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use tilefab::{
    Alignment, Backend, BuildError, Color, Constraint, Entry, Layout, Line, LineWidget, Modifier,
    Rank, Rect, Style, Stylize, Terminal, TerminalGuard, WallTime, grapheme_width,
    install_panic_hook,
};

const TEXT: &str = "This is a long paragraph, wrapped to the width of the window by a small \
function of this example. It contains accents (été, où, à), wide characters (日本語, 😀) and \
enough words to span several lines. Below it, three line widgets are animated over 10 seconds: \
resize the window to see them shrink, then drop their last components.";

/// The steps shown by the log of the first widget, one after the other.
const STEPS: &[&str] = &[
    "resolving dependencies",
    "downloading crates",
    "compiling tilefab-core",
    "compiling tilefab-widget",
    "linking the demo",
];

/// The three widgets of the demo.
struct Widgets {
    /// `[00:00:01.234] 🟠 running  compiling tilefab-core`.
    build: LineWidget,
    /// `download ██████░░░░  60.0%  30.0MB/50.0MB`.
    download: LineWidget,
    /// `2026-11-25 00:00:01 | step 3 of 5`.
    clock: LineWidget,
}

fn widgets() -> Result<Widgets, BuildError> {
    let gray = Style::new().fg(Color::DarkGray);
    let build = LineWidget::builder("B0 T0 B1 B2 S B2 K B2 L")
        .barriers([
            Entry::styled("[", gray),
            Entry::styled("]", gray),
            Entry::new(" "),
        ])
        .symbols([Entry::new("🔴"), Entry::new("🟠"), Entry::new("🟢")])
        .keywords([
            Entry::styled("waiting", gray),
            Entry::styled("running", Style::new().fg(Color::Yellow)),
            Entry::styled("done", Style::new().fg(Color::Green)),
        ])
        .keyword_alignment(Alignment::Center)
        .time_style(Style::new().fg(Color::Cyan))
        .build()?;
    let download = LineWidget::builder("N B0 I B0 P5 B0 V5U/")
        .names([Entry::styled(
            "download",
            Style::new().add_modifier(Modifier::BOLD),
        )])
        .barriers([Entry::new(" ")])
        .illustration(
            Entry::styled("━", Style::new().fg(Color::Green)),
            Entry::styled("─", gray),
        )
        .units([Entry::styled("MB", gray)])
        .build()?;
    let clock = LineWidget::builder("D0 B0 T3 B1 L")
        .barriers([Entry::new(" "), Entry::styled(" | ", gray)])
        .build()?;
    Ok(Widgets {
        build,
        download,
        clock,
    })
}

/// A time of day `elapsed` after midnight, on a fixed date.
fn wall_time(elapsed: Duration) -> WallTime {
    let millis = elapsed.as_millis();
    let part = |value: u128, modulo: u128| u8::try_from(value % modulo).unwrap_or(0);
    WallTime {
        year: 2026,
        month: 11,
        day: 25,
        hour: part(millis / 3_600_000, 24),
        minute: part(millis / 60_000, 60),
        second: part(millis / 1000, 60),
        millis: u16::try_from(millis % 1000).unwrap_or(0),
    }
}

/// Update the widgets for `elapsed`: a 10-second cycle.
fn animate(widgets: &mut Widgets, elapsed: Duration) -> Result<(), tilefab::UpdateError> {
    let ratio = (elapsed.as_secs_f64() % 10.0) / 10.0;
    let now = wall_time(elapsed);
    let step_count = STEPS.len();
    // `ratio` is in 0..1: the index is below `step_count`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "a small index in 0..STEPS.len()"
    )]
    let step = ((ratio * step_count as f64) as usize).min(step_count - 1);
    let state = match ratio {
        r if r < 0.05 => 0,
        r if r < 0.95 => 1,
        _ => 2,
    };
    widgets.build.set_time(now)?;
    widgets.build.set_symbol(Rank(0), state)?;
    widgets.build.set_keyword(state)?;
    widgets.build.set_log(STEPS[step]);
    widgets.download.set_progress(ratio * 100.0)?;
    widgets
        .download
        .set_value(ratio * 50.0, Some(0), Some((50.0, Some(0))))?;
    widgets.clock.set_time(now)?;
    widgets
        .clock
        .set_log(&format!("step {} of {step_count}", step + 1));
    Ok(())
}

/// Cut `text` into rows of at most `width` columns, at spaces when possible.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    for word in text.split(' ') {
        let word_width: usize = word
            .chars()
            .map(|c| grapheme_width(c.encode_utf8(&mut [0; 4])))
            .sum();
        if used > 0 && used + 1 + word_width > width {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        if used > 0 {
            row.push(' ');
            used += 1;
        }
        // A word wider than the row is cut wherever it reaches the edge.
        for c in word.chars() {
            let c_width = grapheme_width(c.encode_utf8(&mut [0; 4]));
            if used + c_width > width && used > 0 {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push(c);
            used += c_width;
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The panic hook restores the terminal before displaying the report.
    install_panic_hook();
    let mut widgets = widgets()?;
    // The guard enables raw mode and the alternate screen; its `Drop` undoes both.
    let _guard = TerminalGuard::fullscreen()?;
    // BufWriter: the entire frame is sent in a single `write` on `flush`.
    let mut terminal = Terminal::new(Backend::new(BufWriter::new(io::stdout())))?;

    let start = Instant::now();
    loop {
        animate(&mut widgets, start.elapsed())?;
        terminal.draw(|frame| {
            let [title, body, lines, help] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Fill(1),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .areas(frame.area());

            frame.render_widget("Full-screen demo".bold().cyan(), title);
            for (y, row) in (body.y..body.bottom()).zip(wrap(TEXT, usize::from(body.width))) {
                frame.render_widget(Line::from(row), Rect::new(body.x, y, body.width, 1));
            }
            let rows = [&widgets.build, &widgets.download, &widgets.clock];
            for (y, widget) in (lines.y..lines.bottom()).zip(rows) {
                frame.render_widget(widget, Rect::new(lines.x, y, lines.width, 1));
            }
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
