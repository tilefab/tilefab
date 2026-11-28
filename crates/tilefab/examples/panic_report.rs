//! A panic in the render function, for the end-to-end check of `tools/check_default_terminal.py`:
//! the panic report must stay on screen, below the log lines, and the terminal must be back
//! in line mode.
//!
//! Run: `cargo run --example panic_report [-- --fullscreen]`, then resize the window.
//!
//! The program inserts a log line, draws the same frame every 50 ms, and panics in the render
//! function of the first frame after the window changes size: the frame that clears the
//! screen (fullscreen) or moves the viewport (inline) before drawing. A log line is queued
//! just before, so that the frame also has a line to insert.

use std::time::Duration;

use tilefab::{is_ctrl_c, poll_key};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = if std::env::args().any(|arg| arg == "--fullscreen") {
        tilefab::init_fullscreen()?
    } else {
        tilefab::init_inline(2)?
    };
    terminal.insert_line("[INFO] started");
    let mut first_width = None;
    loop {
        let width = terminal.backend().size()?.width;
        if first_width.is_some_and(|first| first != width) {
            terminal.insert_line("[INFO] resized");
        }
        terminal.draw(|frame| {
            let area = frame.area();
            assert_eq!(
                *first_width.get_or_insert(area.width),
                area.width,
                "deliberate panic in the render function"
            );
            frame.render_widget("waiting for a resize", area);
        })?;
        if let Some(key) = poll_key(Duration::from_millis(50))?
            && is_ctrl_c(&key)
        {
            return Ok(());
        }
    }
}
