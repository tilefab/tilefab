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

pub fn draw(frame: &mut Frame, app: &App) {
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

fn draw_prompt(frame: &mut Frame, app: &App, area: Rect) {
    let [prefix_area, input_area] = Layout::horizontal([
        Constraint::Length(saturating_u16(PROMPT.len())),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(Line::from(PROMPT.bold()), prefix_area);

    // Horizontal scrolling: the input line computes how many columns to shift
    // the text to keep the cursor visible. Reserve one free column
    // for the cursor at the end of the line.
    let width = input_area.width.saturating_sub(1) as usize;
    let scroll = app.input.visual_scroll(width);
    frame.render_widget(
        Paragraph::new(app.input.value()).scroll((0, saturating_u16(scroll))),
        input_area,
    );

    // Cursor position in display columns (handles accents and wide emoji).
    let cursor_x = saturating_u16(app.input.visual_cursor().saturating_sub(scroll));
    frame.set_cursor_position((input_area.x + cursor_x, input_area.y));
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
    fn render(app: &App, width: u16, height: u16) -> Vec<String> {
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
        let app = app_with_tasks(0);
        assert_eq!(render(&app, 30, 1), [">"]);
    }

    #[test]
    fn each_task_gets_a_row_above_the_prompt() {
        let app = app_with_tasks(2);
        let rows = render(&app, 30, 3);
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
