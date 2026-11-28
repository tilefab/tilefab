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
