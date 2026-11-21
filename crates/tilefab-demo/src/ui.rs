//! Rendering the inline viewport and the lines inserted above it.

use tilefab::{Color, Constraint, Frame, Layout, Line, LineGauge, Paragraph, Rect, Stylize, Wrap};

use tilefab::convert::saturating_u16;

use crate::app::{App, Task};

/// Prefix displayed before the input.
const PROMPT: &str = "> ";
/// Width of the task name column (longer names are truncated).
const NAME_WIDTH: u16 = 10;

/// Desired viewport height: one line per task, plus the prompt line.
pub fn viewport_height(app: &App) -> u16 {
    saturating_u16(app.tasks.len()).saturating_add(1)
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    // The top of the viewport is reserved for task bars, the last line for the prompt.
    let [tasks_area, prompt_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());

    // One line per task, from the top; extra lines remain empty.
    for (task, row) in app.tasks.iter().zip(tasks_area.rows()) {
        draw_task(frame, task, row);
    }
    draw_prompt(frame, app, prompt_area);
}

/// `name       ━━━━━━━━──────────  45%`
fn draw_task(frame: &mut Frame, task: &Task, area: Rect) {
    let [name_area, gauge_area, percent_area] = Layout::horizontal([
        Constraint::Length(NAME_WIDTH),
        Constraint::Fill(1),
        Constraint::Length(5),
    ])
    .areas(area);

    frame.render_widget(Line::from(task.name.as_str()).bold(), name_area);
    // Empty label: LineGauge leaves one column of space before the bar.
    frame.render_widget(
        LineGauge::default()
            .ratio(task.ratio)
            .label("")
            .filled_symbol("█")
            .unfilled_symbol("░")
            .filled_style(Color::Cyan)
            .unfilled_style(Color::DarkGray),
        gauge_area,
    );
    frame.render_widget(
        Line::from(format!("{:3.0}%", task.ratio * 100.0)).right_aligned(),
        percent_area,
    );
}

fn draw_prompt(frame: &mut Frame, app: &mut App, area: Rect) {
    // The editor draws the prompt and the line (horizontal scrolling, `…` for hidden
    // text) and returns where the cursor goes.
    let prompt = Line::from(PROMPT.bold());
    if let Some(cursor) = app.editor.render(area, frame.buffer_mut(), &prompt) {
        frame.set_cursor_position(cursor);
    }
}

/// Prepare a block of lines to insert above the viewport: return the
/// paragraph and its height after wrapping to `width`.
pub fn wrapped_lines(lines: Vec<Line<'static>>, width: u16) -> (Paragraph<'static>, u16) {
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let height = saturating_u16(paragraph.line_count(width));
    (paragraph, height)
}

#[cfg(test)]
mod tests {
    use tilefab::{Backend, Position, Size, Terminal};
    use tokio::sync::mpsc;

    use super::*;

    /// Draw the application in a simulated terminal and return the screen lines.
    fn render(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let backend =
            Backend::with_fixed(Vec::new(), Size::new(width, height), Position::new(0, 0));
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .buffer()
            .lines()
            .into_iter()
            .map(|l| l.trim_end().to_string())
            .collect()
    }

    fn app_with_tasks(n: usize) -> App {
        let (tx, _rx) = mpsc::channel(16);
        let mut app = App::new(tx);
        for i in 0..n {
            app.tasks.push(Task {
                id: i as u64 + 1,
                name: format!("t{i}"),
                ratio: 0.5,
            });
        }
        app
    }

    #[test]
    fn viewport_height_is_one_line_per_task_plus_the_prompt() {
        assert_eq!(viewport_height(&app_with_tasks(0)), 1);
        assert_eq!(viewport_height(&app_with_tasks(3)), 4);
    }

    #[test]
    fn idle_viewport_is_only_the_prompt() {
        let mut app = app_with_tasks(0);
        assert_eq!(render(&mut app, 30, 1), [">"]);
    }

    #[test]
    fn each_task_gets_a_row_above_the_prompt() {
        let mut app = app_with_tasks(2);
        let rows = render(&mut app, 30, 3);
        assert!(rows[0].starts_with("t0"), "{rows:?}");
        assert!(rows[0].ends_with(" 50%"), "{rows:?}");
        assert!(rows[1].starts_with("t1"), "{rows:?}");
        assert_eq!(rows[2], ">");
    }

    #[test]
    fn wrapped_lines_counts_visual_lines() {
        let lines = vec![Line::from("abcdef"), Line::from("gh")];
        let (_, height) = wrapped_lines(lines, 4);
        // “abcdef” wrapped to 4 columns = 2 lines, plus “gh” = 3.
        assert_eq!(height, 3);
    }
}
