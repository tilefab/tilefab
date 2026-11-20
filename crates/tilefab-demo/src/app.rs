//! Application state, event handling, and command processing.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tilefab::{InputLine, Line};
use tokio::sync::mpsc::Sender;
use tracing::{Level, error, info, warn};

use crate::MAX_TASKS;
use crate::logging;
use crate::tasks::{self, TaskId};

/// Everything that can wake the UI loop goes through this type over a single
/// channel: the UI loop is the only code that writes to the terminal.
#[derive(Debug)]
pub enum AppEvent {
    /// Key read by the keyboard thread.
    Key(KeyEvent),
    /// The terminal was resized (redrawing is enough: `draw` handles resizing).
    Resize,
    /// Complete log line produced by tracing (see `logging.rs`).
    Log(String),
    /// Task progress, between 0.0 and 1.0.
    Progress { id: TaskId, ratio: f64 },
    /// A task has finished.
    TaskDone(TaskId),
    /// Regular tick: forces a periodic redraw.
    Tick,
    /// Headless mode (no TTY): line read from stdin.
    Line(String),
    /// Headless mode: end of stdin.
    InputClosed,
    /// Termination signal received (`SIGTERM`, `SIGINT`, `SIGHUP`): quit through
    /// the normal path so the terminal is restored.
    Quit,
}

/// A command entered at the prompt.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Empty line: nothing to do.
    Empty,
    Start(String),
    List,
    Help,
    Quit,
    /// Hidden command for testing terminal restoration. Debug builds only
    /// in a release build, `panic` is an unknown command.
    #[cfg(debug_assertions)]
    Panic,
}

/// Command parsing error.
#[derive(Debug, PartialEq, Eq)]
pub enum CommandError {
    /// `start` without a task name.
    MissingName,
    /// Unrecognized first word.
    Unknown(String),
}

impl Command {
    /// Parse a line: first word = command, remainder = argument.
    pub fn parse(line: &str) -> Result<Self, CommandError> {
        let line = line.trim();
        let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        match word {
            "" => Ok(Self::Empty),
            "start" if rest.is_empty() => Err(CommandError::MissingName),
            "start" => Ok(Self::Start(rest.to_string())),
            "list" => Ok(Self::List),
            "help" => Ok(Self::Help),
            "quit" | "exit" => Ok(Self::Quit),
            #[cfg(debug_assertions)]
            "panic" => Ok(Self::Panic),
            other => Err(CommandError::Unknown(other.to_string())),
        }
    }
}

/// A running task, as seen by the UI.
#[derive(Debug)]
pub struct Task {
    pub id: TaskId,
    pub name: String,
    pub ratio: f64,
}

pub struct App {
    /// Prompt input field.
    pub input: InputLine,
    /// Running tasks, in launch order.
    pub tasks: Vec<Task>,
    /// Lines to insert above the viewport on the next loop iteration.
    pub output: Vec<Line<'static>>,
    pub should_quit: bool,
    /// Sender given to tasks so they can report progress.
    tx: Sender<AppEvent>,
    next_id: TaskId,
}

impl App {
    pub fn new(tx: Sender<AppEvent>) -> Self {
        Self {
            input: InputLine::default(),
            tasks: Vec::new(),
            output: Vec::new(),
            should_quit: false,
            tx,
            next_id: 1,
        }
    }

    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Log(line) => self.output.push(Line::from(line)),
            AppEvent::Progress { id, ratio } => {
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
                    task.ratio = ratio;
                }
            }
            AppEvent::TaskDone(id) => {
                if let Some(pos) = self.tasks.iter().position(|t| t.id == id) {
                    let task = self.tasks.remove(pos);
                    self.log_info(&format!("task \"{}\" finished", task.name));
                }
            }
            AppEvent::Line(line) => self.submit(&line),
            AppEvent::Quit => self.should_quit = true,
            // The headless loop handles the end of stdin.
            AppEvent::Resize | AppEvent::Tick | AppEvent::InputClosed => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // On Windows, crossterm also emits key release events.
        if key.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => self.should_quit = true,
            KeyCode::Char('d') if ctrl && self.input.value().is_empty() => self.should_quit = true,
            // In raw mode, `\n` arrives as Ctrl+J (this happens with input
            // typed before startup that the tty has already converted): like
            // readline, treat it as Enter.
            KeyCode::Char('j') if ctrl => self.accept_line(),
            KeyCode::Enter => self.accept_line(),
            // Everything else (characters, arrows, Ctrl+A/E/W/U…) is handled by the input line.
            _ => {
                self.input.handle_event(&Event::Key(key));
            }
        }
    }

    /// Submit input: echo it above (as in a shell), then execute it.
    fn accept_line(&mut self) {
        let line = self.input.value_and_reset();
        self.output.push(Line::from(format!("> {line}")));
        self.submit(&line);
    }

    /// Add an `[INFO] …` line directly to `output`, bypassing tracing.
    ///
    /// For events that change viewport height (task started or finished), the
    /// line must arrive in the same frame as the change; otherwise the viewport would shrink
    /// before its replacement line arrives (see the placement rule in `tui::viewport`).
    fn log_info(&mut self, message: &str) {
        self.output
            .push(Line::from(logging::format_line(Level::INFO, message)));
    }

    /// Execute a command line. Feedback goes through tracing, like
    /// any other log.
    pub fn submit(&mut self, line: &str) {
        match Command::parse(line) {
            Ok(Command::Empty) => {}
            Ok(Command::Start(name)) => self.start_task(name),
            Ok(Command::List) => self.list_tasks(),
            Ok(Command::Help) => {
                info!("commands: start <name>, list, help, quit (or exit, Ctrl+C, Ctrl+D)");
            }
            Ok(Command::Quit) => self.should_quit = true,
            #[cfg(debug_assertions)]
            Ok(Command::Panic) => panic!("deliberate panic (`panic` command)"),
            Err(CommandError::MissingName) => error!("usage: start <name>"),
            Err(CommandError::Unknown(word)) => {
                warn!("unknown command: \"{word}\" (type help)");
            }
        }
    }

    fn start_task(&mut self, name: String) {
        if self.tasks.len() >= MAX_TASKS {
            warn!("already {MAX_TASKS} tasks running: \"{name}\" refused");
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        let duration = tasks::spawn(id, self.tx.clone());
        self.log_info(&format!(
            "task \"{name}\" started (duration {:.1} s)",
            duration.as_secs_f64()
        ));
        self.tasks.push(Task {
            id,
            name,
            ratio: 0.0,
        });
    }

    fn list_tasks(&self) {
        if self.tasks.is_empty() {
            info!("no task running");
        }
        for task in &self.tasks {
            info!("  {} : {:3.0} %", task.name, task.ratio * 100.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn quit_event_requests_shutdown() {
        let (tx, _rx) = mpsc::channel(16);
        let mut app = App::new(tx);
        app.handle_event(AppEvent::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn parse_simple_commands() {
        assert_eq!(Command::parse("list"), Ok(Command::List));
        assert_eq!(Command::parse("help"), Ok(Command::Help));
        assert_eq!(Command::parse("quit"), Ok(Command::Quit));
        assert_eq!(Command::parse("exit"), Ok(Command::Quit));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn panic_command_exists_in_debug_builds() {
        assert_eq!(Command::parse("panic"), Ok(Command::Panic));
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn panic_command_is_unknown_in_release_builds() {
        assert_eq!(
            Command::parse("panic"),
            Err(CommandError::Unknown("panic".to_string()))
        );
    }

    #[test]
    fn parse_ignores_surrounding_whitespace() {
        assert_eq!(Command::parse(""), Ok(Command::Empty));
        assert_eq!(Command::parse("   "), Ok(Command::Empty));
        assert_eq!(Command::parse("  list  "), Ok(Command::List));
        assert_eq!(
            Command::parse("  start   alpha  "),
            Ok(Command::Start("alpha".into()))
        );
    }

    #[test]
    fn parse_start_takes_rest_of_line_as_name() {
        assert_eq!(
            Command::parse("start mon été 🚀"),
            Ok(Command::Start("mon été 🚀".into()))
        );
        assert_eq!(
            Command::parse("start\talpha"),
            Ok(Command::Start("alpha".into()))
        );
    }

    #[test]
    fn parse_errors() {
        assert_eq!(Command::parse("start"), Err(CommandError::MissingName));
        assert_eq!(Command::parse("start   "), Err(CommandError::MissingName));
        assert_eq!(
            Command::parse("foo bar"),
            Err(CommandError::Unknown("foo".into()))
        );
        // Commands are case-sensitive.
        assert_eq!(
            Command::parse("LIST"),
            Err(CommandError::Unknown("LIST".into()))
        );
    }

    #[tokio::test]
    async fn start_refuses_beyond_max_tasks_and_done_removes() {
        let (tx, _rx) = mpsc::channel(16);
        let mut app = App::new(tx);
        for i in 0..MAX_TASKS + 2 {
            app.submit(&format!("start t{i}"));
        }
        assert_eq!(app.tasks.len(), MAX_TASKS);

        let id = app.tasks[0].id;
        app.handle_event(AppEvent::Progress { id, ratio: 0.5 });
        assert!((app.tasks[0].ratio - 0.5).abs() < f64::EPSILON);

        app.handle_event(AppEvent::TaskDone(id));
        assert_eq!(app.tasks.len(), MAX_TASKS - 1);
        assert!(app.tasks.iter().all(|t| t.id != id));
    }

    /// The plain text of an output line.
    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> AppEvent {
        AppEvent::Key(KeyEvent::new(code, modifiers))
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            app.handle_event(key(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    fn new_app() -> App {
        let (tx, _rx) = mpsc::channel(16);
        App::new(tx)
    }

    #[test]
    fn enter_and_ctrl_j_echo_the_line_and_clear_the_input() {
        let mut app = new_app();
        type_text(&mut app, "foo");
        app.handle_event(key(KeyCode::Enter, KeyModifiers::NONE));
        // In raw mode, a `\n` typed before startup arrives as Ctrl+J: same as Enter.
        type_text(&mut app, "bar");
        app.handle_event(key(KeyCode::Char('j'), KeyModifiers::CONTROL));
        let lines: Vec<String> = app.output.iter().map(|l| text(l)).collect();
        assert_eq!(lines, ["> foo", "> bar"]);
        assert_eq!(app.input.value(), "");
    }

    #[test]
    fn key_releases_are_ignored() {
        let mut app = new_app();
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        app.handle_event(AppEvent::Key(release));
        assert_eq!(app.input.value(), "");
    }

    #[test]
    fn ctrl_c_quits_even_with_input() {
        let mut app = new_app();
        type_text(&mut app, "start a");
        app.handle_event(key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_d_quits_only_on_an_empty_line() {
        let mut app = new_app();
        type_text(&mut app, "x");
        app.handle_event(key(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(!app.should_quit, "Ctrl+D with input must not quit");
        app.handle_event(key(KeyCode::Backspace, KeyModifiers::NONE));
        app.handle_event(key(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn log_events_are_appended_to_output() {
        let mut app = new_app();
        app.handle_event(AppEvent::Log("[INFO] hello".into()));
        assert_eq!(text(&app.output[0]), "[INFO] hello");
    }

    #[test]
    fn events_for_unknown_tasks_are_ignored() {
        let mut app = new_app();
        app.handle_event(AppEvent::Progress { id: 42, ratio: 0.5 });
        app.handle_event(AppEvent::TaskDone(42));
        assert!(app.tasks.is_empty());
        assert!(app.output.is_empty());
    }

    #[test]
    fn commands_report_through_tracing() {
        let mut app = new_app();
        let logs = logging::capture(|| {
            app.submit("");
            app.submit("help");
            app.submit("list");
            app.submit("start");
            app.submit("foo");
        });
        assert_eq!(
            logs,
            [
                "[INFO] commands: start <name>, list, help, quit (or exit, Ctrl+C, Ctrl+D)",
                "[INFO] no task running",
                "[ERROR] usage: start <name>",
                "[WARN] unknown command: \"foo\" (type help)",
            ]
        );
        assert!(!app.should_quit);
    }

    #[tokio::test]
    async fn list_shows_running_tasks_and_start_reports_a_refusal() {
        let mut app = new_app();
        let logs = logging::capture(|| {
            for i in 0..=MAX_TASKS {
                app.submit(&format!("start t{i}"));
            }
            let id = app.tasks[0].id;
            app.handle_event(AppEvent::Progress { id, ratio: 0.25 });
            app.submit("list");
        });
        assert_eq!(
            logs[0],
            format!("[WARN] already {MAX_TASKS} tasks running: \"t{MAX_TASKS}\" refused")
        );
        assert_eq!(logs[1], "[INFO]   t0 :  25 %");
        assert_eq!(
            logs.len(),
            1 + MAX_TASKS,
            "one line per running task: {logs:?}"
        );
    }

    #[tokio::test]
    async fn start_and_done_put_their_line_directly_in_output() {
        let (tx, _rx) = mpsc::channel(16);
        let mut app = App::new(tx);
        app.submit("start alpha");
        assert_eq!(app.output.len(), 1);
        let started = text(&app.output[0]);
        assert!(
            started.starts_with("[INFO] task \"alpha\" started"),
            "{started}"
        );

        let id = app.tasks[0].id;
        app.handle_event(AppEvent::TaskDone(id));
        assert_eq!(app.output.len(), 2);
        assert_eq!(text(&app.output[1]), "[INFO] task \"alpha\" finished");
    }
}
