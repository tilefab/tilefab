//! Rendering the inline viewport: one row per task, drawn by its line widget, then the prompt
//! and the completion menu.

use tilefab::{Constraint, Frame, Layout, Line, Rect, Stylize};

use crate::app::App;

/// Prefix displayed before the input.
const PROMPT: &str = "> ";

/// Desired viewport height in a terminal `width` columns wide: one row per task, the prompt
/// line, and the rows of the completion menu when it is open.
pub fn viewport_height(app: &App, width: u16) -> u16 {
    u16::try_from(app.tasks.len())
        .unwrap_or(u16::MAX)
        .saturating_add(1)
        .saturating_add(app.editor.extra_rows(width))
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    // The top of the viewport is reserved for the tasks, the bottom for the prompt and
    // the completion menu below it.
    let prompt_rows = 1 + app.editor.extra_rows(frame.area().width);
    let [tasks_area, prompt_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(prompt_rows)])
            .areas(frame.area());

    // The tasks from the top, one row each, with the whole width: the widget shrinks, then
    // drops components, when the terminal is narrow.
    for (task, row) in app.tasks.iter().zip(tasks_area.rows()) {
        frame.render_widget(&task.widget, row);
    }
    draw_prompt(frame, app, prompt_area);
}

fn draw_prompt(frame: &mut Frame, app: &mut App, area: Rect) {
    // The editor draws the prompt and the line (horizontal scrolling, `…` for hidden
    // text) and returns where the cursor goes.
    let prompt = Line::from(PROMPT.bold());
    if let Some(cursor) = app.editor.render(area, frame.buffer_mut(), &prompt) {
        frame.set_cursor_position(cursor);
    }
}

#[cfg(test)]
mod tests {
    use tilefab::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tilefab::{Backend, Position, Size, Terminal};
    use tokio::sync::mpsc;

    use crate::app::{AppEvent, Task};

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

    fn app_with_tasks(n: usize, composition: &str) -> App {
        let (tx, _rx) = mpsc::channel(16);
        let mut app = App::new(tx);
        for i in 0..n {
            app.tasks
                .push(Task::new(i as u64 + 1, format!("t{i}"), composition, 2).unwrap());
        }
        app
    }

    #[test]
    fn viewport_height_is_one_line_per_task_plus_the_prompt() {
        assert_eq!(viewport_height(&app_with_tasks(0, "N"), 80), 1);
        assert_eq!(viewport_height(&app_with_tasks(3, "N"), 80), 4);
    }

    #[test]
    fn the_open_menu_adds_its_rows_below_the_prompt() {
        let mut app = app_with_tasks(1, "N");
        app.handle_event(AppEvent::Key(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        // Six commands with descriptions: one per row, up to the 5-row limit.
        assert_eq!(viewport_height(&app, 80), 1 + 1 + 5);
        let rows = render(&mut app, 40, 7);
        assert_eq!(rows[0], "t0");
        assert_eq!(rows[1], "> start");
        assert!(rows[2].starts_with("start    start a task"), "{rows:?}");
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
        let mut app = app_with_tasks(0, "N");
        assert_eq!(render(&mut app, 30, 1), [">"]);
    }

    #[test]
    fn each_task_gets_a_row_above_the_prompt_drawn_with_its_composition() {
        let mut app = app_with_tasks(2, "N B2 P");
        app.tasks[1] = Task::new(9, "t1".into(), "P B2 N", 2).unwrap();
        app.handle_event(AppEvent::Progress {
            id: 1,
            ratio: 0.5,
            step: "compiling",
        });
        let rows = render(&mut app, 30, 3);
        assert_eq!(rows, ["t0  50%", "  0% t1", ">"]);
    }

    #[tokio::test]
    async fn the_bars_of_the_default_composition_line_up() {
        let (tx, _rx) = mpsc::channel(64);
        let mut app = App::new(tx);
        app.submit("start a");
        app.submit("start epsilon");
        app.submit("start 日本");
        let rows = render(&mut app, 80, 4);
        // The time follows the name column: same column on every row.
        // In columns, not bytes: `日本` is 4 columns wide.
        let column = |row: &str| {
            row.find('[').map(|at| {
                row[..at]
                    .chars()
                    .map(|c| tilefab::grapheme_width(c.encode_utf8(&mut [0; 4])))
                    .sum::<usize>()
            })
        };
        let columns: Vec<Option<usize>> = rows[..3].iter().map(|row| column(row)).collect();
        assert_eq!(columns, [Some(8), Some(8), Some(8)], "{rows:?}");
        assert!(rows[0].starts_with("a       ["), "{rows:?}");
        // The longest name ends: the column shrinks to the longest one left.
        let id = app.tasks[1].id;
        app.handle_event(AppEvent::TaskDone(id));
        let rows = render(&mut app, 80, 3);
        assert!(rows[0].starts_with("a    ["), "{rows:?}");
        assert!(rows[1].starts_with("日本 ["), "{rows:?}");
    }

    #[test]
    fn a_narrow_terminal_shrinks_then_drops_components() {
        let mut app = app_with_tasks(1, "N B2 P B2 L");
        app.handle_event(AppEvent::Progress {
            id: 1,
            ratio: 0.25,
            step: "downloading crates",
        });
        assert_eq!(render(&mut app, 40, 2)[0], "t0  25% downloading crates");
        assert_eq!(render(&mut app, 16, 2)[0], "t0  25% downloa…");
        assert_eq!(render(&mut app, 9, 2)[0], "t0  25%");
        assert_eq!(render(&mut app, 3, 2)[0], "t0");
    }
}
