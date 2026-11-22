//! Rendering the inline viewport and the lines inserted above it.

use tilefab::{
    Block, BorderType, Color, Constraint, Frame, Layout, Line, Paragraph, ProgressBar,
    ProgressLine, Rect, Span, Style, Stylize, Wrap, grapheme_width,
};

use tilefab::convert::saturating_u16;

use crate::app::{App, Task};
use crate::view::TaskView;

/// Prefix displayed before the input.
const PROMPT: &str = "> ";
/// The name column of `progressline` tasks takes at most this fraction of the width (a
/// longer name ends with `…`), so the bar keeps most of the row.
const NAME_MAX_FRACTION: u16 = 3;

/// Desired viewport height in a terminal `width` columns wide: the rows of each task (one or
/// more, depending on its view), the prompt line, and the rows of the completion menu when
/// it is open.
pub fn viewport_height(app: &App, width: u16) -> u16 {
    app.tasks
        .iter()
        .fold(0u16, |rows, task| rows.saturating_add(task.view.height()))
        .saturating_add(1)
        .saturating_add(app.editor.extra_rows(width))
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    // The top of the viewport is reserved for task bars, the bottom for the prompt and
    // the completion menu below it.
    let prompt_rows = 1 + app.editor.extra_rows(frame.area().width);
    let [tasks_area, prompt_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(prompt_rows)])
            .areas(frame.area());

    // The tasks from the top, each on the rows its view needs; extra rows remain empty.
    let name_width = name_column_width(&app.tasks, tasks_area.width);
    let mut y = tasks_area.y;
    for task in &app.tasks {
        let height = task
            .view
            .height()
            .min(tasks_area.bottom().saturating_sub(y));
        if height == 0 {
            break;
        }
        let area = Rect::new(tasks_area.x, y, tasks_area.width, height);
        draw_task(frame, task, area, name_width);
        y += height;
    }
    draw_prompt(frame, app, prompt_area);
}

/// Width of the name column shared by the `progressline` tasks, so that their bars line up:
/// the longest of their names, capped at a fraction of `width`.
fn name_column_width(tasks: &[Task], width: u16) -> u16 {
    let longest = tasks
        .iter()
        .filter(|task| task.view.is_line())
        .map(|task| saturating_u16(Line::from(task.name.as_str()).width()))
        .max()
        .unwrap_or(0);
    longest.min(width / NAME_MAX_FRACTION)
}

/// `name` cut to `width` columns, ending with `…` when it does not fit.
fn fit_name(name: &str, width: u16) -> String {
    let width = usize::from(width);
    if Line::from(name).width() <= width {
        return name.to_string();
    }
    let mut fitted = String::new();
    let mut used = 0;
    for c in name.chars() {
        let c_width = grapheme_width(c.encode_utf8(&mut [0; 4]));
        // Keep one column for the `…`.
        if used + c_width >= width {
            break;
        }
        fitted.push(c);
        used += c_width;
    }
    if width > 0 {
        fitted.push('…');
    }
    fitted
}

fn draw_task(frame: &mut Frame, task: &Task, area: Rect, name_width: u16) {
    let line = |frame: &mut Frame, symbols, color| {
        draw_line(frame, task, area, name_width, symbols, color);
    };
    match task.view {
        TaskView::LineBlocks => line(frame, ("█", "░"), Color::Cyan),
        TaskView::LineThin => line(frame, ("━", "─"), Color::Yellow),
        TaskView::LineDots => line(frame, ("●", "·"), Color::Green),
        TaskView::BarPlain => draw_bar(frame, task, area, None),
        TaskView::BarBordered => {
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Color::Blue)
                .title(Line::from(format!(" {} ", task.name)).bold());
            draw_bar(frame, task, area, Some(block));
        }
    }
}

/// `name  ██████████░░░░░░░░░░  45%`: the name in a column `name_width` wide, then a
/// `ProgressLine` filling the rest of the row up to the percentage, drawn with the
/// `(filled, unfilled)` symbols.
fn draw_line(
    frame: &mut Frame,
    task: &Task,
    area: Rect,
    name_width: u16,
    (filled, unfilled): (&'static str, &'static str),
    color: Color,
) {
    let [name_area, bar_area, percent_area] = Layout::horizontal([
        Constraint::Length(name_width),
        Constraint::Fill(1),
        Constraint::Length(5),
    ])
    .areas(area);

    frame.render_widget(
        Line::from(fit_name(&task.name, name_width)).bold(),
        name_area,
    );
    // Empty label: ProgressLine leaves one column of space before the bar.
    frame.render_widget(
        ProgressLine::default()
            .ratio(task.ratio)
            .label("")
            .filled_symbol(filled)
            .unfilled_symbol(unfilled)
            .filled_style(color)
            .unfilled_style(Color::DarkGray),
        bar_area,
    );
    frame.render_widget(
        Line::from(format!("{:3.0}%", task.ratio * 100.0)).right_aligned(),
        percent_area,
    );
}

/// A `ProgressBar` drawn with background colors, its label centered over it: `name 45%`
/// without a block, `45%` inside one (the block's title then gives the name).
fn draw_bar(frame: &mut Frame, task: &Task, area: Rect, block: Option<Block<'static>>) {
    let percent = format!("{:.0}%", task.ratio * 100.0);
    let label = if block.is_some() {
        percent
    } else {
        format!("{} {percent}", task.name)
    };
    let mut bar = ProgressBar::default()
        .ratio(task.ratio)
        .label(Span::raw(label).fg(Color::White).bold())
        .filled_symbol(" ")
        .unfilled_symbol(" ")
        .filled_style(Style::new().bg(Color::Blue))
        .unfilled_style(Style::new().bg(Color::DarkGray));
    if let Some(block) = block {
        bar = bar.block(block);
    }
    frame.render_widget(bar, area);
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
    use tilefab::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tilefab::{Backend, Position, Size, Terminal};
    use tokio::sync::mpsc;

    use crate::app::AppEvent;

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
                view: TaskView::default(),
            });
        }
        app
    }

    #[test]
    fn viewport_height_is_one_line_per_task_plus_the_prompt() {
        assert_eq!(viewport_height(&app_with_tasks(0), 80), 1);
        assert_eq!(viewport_height(&app_with_tasks(3), 80), 4);
    }

    #[test]
    fn the_open_menu_adds_its_rows_below_the_prompt() {
        let mut app = app_with_tasks(1);
        app.handle_event(AppEvent::Key(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        // Six commands with descriptions: one per row, up to the 5-row limit.
        assert_eq!(viewport_height(&app, 80), 1 + 1 + 5);
        let rows = render(&mut app, 40, 7);
        assert!(rows[0].starts_with("t0"), "{rows:?}");
        assert_eq!(rows[1], "> start");
        assert!(rows[2].starts_with("start   start a task"), "{rows:?}");
        // The last visible row says how many commands are hidden.
        assert!(rows[6].starts_with("quit"), "{rows:?}");
        assert!(rows[6].ends_with("(+1 more)"), "{rows:?}");
        // Esc closes it: the viewport shrinks back.
        app.handle_event(AppEvent::Key(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert_eq!(viewport_height(&app, 80), 2);
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
    fn each_task_is_drawn_with_its_own_view() {
        let mut app = app_with_tasks(3);
        app.tasks[1].view = TaskView::LineDots;
        app.tasks[2].view = TaskView::BarBordered;
        // 1 + 1 + 3 task rows, then the prompt.
        assert_eq!(viewport_height(&app, 30), 6);
        let rows = render(&mut app, 30, 6);
        assert!(rows[0].contains("█░"), "{rows:?}");
        assert!(rows[1].contains("●·"), "{rows:?}");
        assert!(rows[2].starts_with("╭ t2 ─"), "{rows:?}");
        assert!(rows[3].contains("50%"), "{rows:?}");
        assert!(rows[4].starts_with("╰─"), "{rows:?}");
        assert_eq!(rows[5], ">");
    }

    #[test]
    fn the_plain_bar_puts_the_name_and_percentage_in_its_label() {
        let mut app = app_with_tasks(1);
        app.tasks[0].view = TaskView::BarPlain;
        let rows = render(&mut app, 30, 2);
        assert_eq!(rows[0].trim(), "t0 50%", "{rows:?}");
    }

    #[test]
    fn the_name_column_fits_the_longest_line_name_up_to_a_third_of_the_width() {
        let mut app = app_with_tasks(3);
        app.tasks[0].name = "cinqsixsepthuitneuf".into();
        app.tasks[2].name = "a much longer name, drawn in a bordered bar".into();
        app.tasks[2].view = TaskView::BarBordered;
        // The bordered task does not count: the column is as wide as the first name.
        assert_eq!(name_column_width(&app.tasks, 90), 19);
        let rows = render(&mut app, 90, 6);
        assert!(rows[0].starts_with("cinqsixsepthuitneuf █"), "{rows:?}");
        // The bars line up after the shared column.
        assert!(rows[1].starts_with("t1                  █"), "{rows:?}");
        // Narrower: capped at a third of 30 columns, the name ends with `…`.
        assert_eq!(name_column_width(&app.tasks, 30), 10);
        let rows = render(&mut app, 30, 6);
        assert!(rows[0].starts_with("cinqsixse… █"), "{rows:?}");
    }

    #[test]
    fn fit_name_cuts_by_columns_and_marks_the_cut() {
        assert_eq!(fit_name("alpha", 5), "alpha");
        assert_eq!(fit_name("alphabet", 5), "alph…");
        // A wide character that would overflow is dropped whole.
        assert_eq!(fit_name("日本語", 4), "日…");
        assert_eq!(fit_name("alpha", 0), "");
    }

    #[test]
    fn wrapped_lines_counts_visual_lines() {
        let lines = vec![Line::from("abcdef"), Line::from("gh")];
        let (_, height) = wrapped_lines(lines, 4);
        // “abcdef” wrapped to 4 columns = 2 lines, plus “gh” = 3.
        assert_eq!(height, 3);
    }
}
