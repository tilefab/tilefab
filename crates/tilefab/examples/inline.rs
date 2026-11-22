//! Demo of an inline viewport with variable height.
//!
//! Run: `cargo run --example inline`.
//!
//! - every 300 ms, a log line is inserted above the viewport;
//! - every 2 seconds, the viewport grows or shrinks (from 1 to 5 lines): it contains task
//!   lines followed by a prompt, always last. As in `tilefab`, each height change
//!   comes with a log line in the **same** frame (task started or finished);
//!   without it, shrinking would leave a blank line below the viewport (see the placement
//!   rule in `tilefab::viewport`);
//! - text can be typed at the prompt; Enter clears it, `q` alone or Ctrl+C quits.
//!
//! The last viewport line (the prompt) stays at the same screen position when the height changes.

use std::io::{self, BufWriter};
use std::time::{Duration, Instant};

use tilefab::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use tilefab::{
    Backend, Color, Constraint, Layout, Line, Paragraph, Span, Style, Stylize, Terminal,
    TerminalGuard, TerminalOptions, Viewport, install_panic_hook,
};

use tilefab::convert::saturating_u16;

const LOG_EVERY: Duration = Duration::from_millis(300);
const RESIZE_EVERY: Duration = Duration::from_secs(2);

fn main() -> io::Result<()> {
    install_panic_hook();
    // Raw mode only: stay on the normal screen; logs go into scrollback.
    let _guard = TerminalGuard::raw()?;
    let mut terminal = Terminal::with_options(
        Backend::new(BufWriter::new(io::stdout())),
        TerminalOptions::default().viewport(Viewport::Inline(1)),
    )?;

    let mut input = String::new();
    let mut height: u16 = 1;
    let mut growing = true;
    let mut log_number = 0u32;
    let mut last_log = Instant::now();
    let mut last_resize = Instant::now();

    loop {
        // A log every 300 ms: inserted above the viewport on the next `draw`.
        if last_log.elapsed() >= LOG_EVERY {
            last_log = Instant::now();
            log_number += 1;
            insert_log(&mut terminal, &format!("[INFO] log number {log_number:04}"));
        }
        // Every 2 seconds, the height changes by one line: 1 → 5 then 5 → 1. The “task
        // started / finished” line is inserted in the same frame as the height change.
        if last_resize.elapsed() >= RESIZE_EVERY {
            last_resize = Instant::now();
            if growing {
                height += 1;
                growing = height < 5;
                insert_log(
                    &mut terminal,
                    &format!("[INFO] task {} started", height - 1),
                );
            } else {
                insert_log(
                    &mut terminal,
                    &format!("[INFO] task {} finished", height - 1),
                );
                height -= 1;
                growing = height <= 1;
            }
            terminal.set_height(height);
        }

        // The height change and logs are applied here, in a single synchronized block.
        terminal.draw(|frame| {
            let rows = Layout::vertical(vec![Constraint::Length(1); frame.area().height as usize])
                .split(frame.area());
            let (prompt, tasks) = rows.split_last().expect("at least one row");
            for (i, row) in tasks.iter().enumerate() {
                let line = Line::from(vec![
                    Span::raw(format!("task {} ", i + 1)).fg(Color::Cyan),
                    Span::raw("██████░░░░ 60%"),
                ]);
                frame.render_widget(Paragraph::new(line), *row);
            }
            // The cursor is in the prompt, after the typed text. Its column is the
            // display width of the line (wide characters take 2 columns), measured
            // with the same rule as rendering, and kept inside the prompt row.
            let prompt_line = Line::from(format!("> {input}"));
            let x = saturating_u16(prompt_line.width()).min(prompt.width.saturating_sub(1));
            frame.render_widget(Paragraph::new(prompt_line), *prompt);
            frame.set_cursor_position((prompt.x.saturating_add(x), prompt.y));
        })?;

        if event::poll(Duration::from_millis(30))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Char('q') if input.is_empty() => break,
                KeyCode::Char(c) => input.push(c),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Enter => input.clear(),
                _ => {}
            }
        }
    }

    // Exit: clear the viewport and leave the cursor at its start; the shell resumes there.
    terminal.clear()?;
    Ok(())
}

/// Queue a log line for insertion above the viewport (applied on the next `draw`).
fn insert_log(terminal: &mut Terminal<BufWriter<io::Stdout>>, text: &str) {
    terminal.insert_before(1, |buf| {
        buf.set_string(0, 0, text, Style::default());
    });
}
