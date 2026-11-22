//! Application state, event handling, and command processing.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tilefab::crossterm::event::KeyEvent;
use tilefab::{
    Candidate, Completer, Completions, EditorConfig, EditorEvent, History, Line, LineEditor,
};
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
    /// Text pasted in the terminal (bracketed paste), read by the keyboard thread.
    Paste(String),
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

/// What a command word does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Start,
    List,
    Help,
    Quit,
}

/// A command of the prompt, as `parse`, `help` and the completer see it.
struct CommandSpec {
    name: &'static str,
    /// How `help` shows it.
    usage: &'static str,
    /// Shown beside the name in the completion menu.
    description: &'static str,
    kind: Kind,
}

/// The commands, defined once. The hidden `panic` command of debug builds is not listed.
const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "start",
        usage: "start <name>",
        description: "start a task",
        kind: Kind::Start,
    },
    CommandSpec {
        name: "list",
        usage: "list",
        description: "list the running tasks",
        kind: Kind::List,
    },
    CommandSpec {
        name: "help",
        usage: "help",
        description: "show the commands",
        kind: Kind::Help,
    },
    CommandSpec {
        name: "quit",
        usage: "quit",
        description: "quit (also Ctrl+C, Ctrl+D)",
        kind: Kind::Quit,
    },
    CommandSpec {
        name: "exit",
        usage: "exit",
        description: "quit",
        kind: Kind::Quit,
    },
];

impl Command {
    /// Parse a line: first word = command, remainder = argument.
    pub fn parse(line: &str) -> Result<Self, CommandError> {
        let line = line.trim();
        let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        if word.is_empty() {
            return Ok(Self::Empty);
        }
        #[cfg(debug_assertions)]
        if word == "panic" {
            return Ok(Self::Panic);
        }
        let Some(spec) = COMMANDS.iter().find(|spec| spec.name == word) else {
            return Err(CommandError::Unknown(word.to_string()));
        };
        match spec.kind {
            Kind::Start if rest.is_empty() => Err(CommandError::MissingName),
            Kind::Start => Ok(Self::Start(rest.to_string())),
            Kind::List => Ok(Self::List),
            Kind::Help => Ok(Self::Help),
            Kind::Quit => Ok(Self::Quit),
        }
    }

    /// The `help` message.
    fn help() -> String {
        let usages: Vec<&str> = COMMANDS.iter().map(|spec| spec.usage).collect();
        format!(
            "commands: {} (or Ctrl+C, Ctrl+D to quit)",
            usages.join(", ")
        )
    }
}

/// Completes the command name (the first word); nothing for the task name.
pub struct CommandCompleter;

impl Completer for CommandCompleter {
    fn complete(&self, line: &str, cursor: usize) -> Completions {
        let before = &line[..cursor];
        let start = before.len() - before.trim_start().len();
        let word = &before[start..];
        if word.contains(char::is_whitespace) {
            return Completions::default();
        }
        let candidates = COMMANDS
            .iter()
            .filter(|spec| spec.name.starts_with(word))
            .map(|spec| {
                let candidate = Candidate::new(spec.name).description(spec.description);
                if spec.kind == Kind::Start {
                    candidate.append_space()
                } else {
                    candidate
                }
            })
            .collect();
        Completions { start, candidates }
    }
}

/// Where the history is kept, given a way to read environment variables: `$TILEFAB_HISTORY`
/// if set (empty: no history file), else `$XDG_STATE_HOME/tilefab/history` (an absolute
/// path only, as the XDG specification requires), else `$HOME/.local/state/tilefab/history`;
/// `None` when none of them is available.
pub fn history_path(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(path) = var("TILEFAB_HISTORY") {
        return (!path.is_empty()).then(|| PathBuf::from(path));
    }
    if let Some(state) = var("XDG_STATE_HOME").map(PathBuf::from)
        && state.is_absolute()
    {
        return Some(state.join("tilefab").join("history"));
    }
    var("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".local/state/tilefab/history"))
}

/// A running task, as seen by the UI.
#[derive(Debug)]
pub struct Task {
    pub id: TaskId,
    pub name: String,
    pub ratio: f64,
}

pub struct App {
    /// Prompt line editor.
    pub editor: LineEditor,
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
            editor: LineEditor::new(EditorConfig::default().tab_accepts_hint(true))
                .with_completer(CommandCompleter),
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
            AppEvent::Paste(text) => {
                let _ = self.editor.paste(&text);
            }
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

    /// Every key goes to the line editor (key releases, Enter, Ctrl+J, Ctrl+C and Ctrl+D
    /// included); the application only acts on what it reports.
    fn handle_key(&mut self, key: KeyEvent) {
        match self.editor.handle_key(key) {
            EditorEvent::Submit(line) => self.accept_line(&line),
            // Ctrl+C, or Ctrl+D on an empty line.
            EditorEvent::Interrupt | EditorEvent::Eof => self.should_quit = true,
            // `Changed`, `Ignored`, and any event a later tilefab version adds: nothing to do.
            _ => {}
        }
    }

    /// Submitted line: echo it above (as in a shell), then execute it.
    fn accept_line(&mut self, line: &str) {
        self.output.push(Line::from(format!("> {line}")));
        if let Some(error) = self.editor.take_history_error() {
            warn!("cannot write the history file ({error}): history kept in memory only");
        }
        self.submit(line);
    }

    /// Use the history file at `path` (loaded now, each submitted line appended). An error
    /// is logged once; the history then works in memory only.
    pub fn load_history(&mut self, path: &Path) {
        let capacity = self.editor.config().history_capacity;
        let (history, error) = History::with_file(path, capacity);
        if let Some(error) = error {
            warn!(
                "cannot use the history file {} ({error}): history kept in memory only",
                path.display()
            );
        }
        *self.editor.history_mut() = history;
    }

    /// Add an `[INFO] …` line directly to `output`, bypassing tracing.
    ///
    /// For events that change viewport height (task started or finished), the
    /// line must arrive in the same frame as the change; otherwise the viewport would shrink
    /// before its replacement line arrives (see the placement rule in [`tilefab::viewport`]).
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
            Ok(Command::Help) => info!("{}", Command::help()),
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
    use tilefab::crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
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
        assert_eq!(app.editor.value(), "");
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
        assert_eq!(app.editor.value(), "");
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
    fn the_completer_completes_the_command_name_only() {
        let names = |line: &str| -> (usize, Vec<String>) {
            let completions = CommandCompleter.complete(line, line.len());
            let names = completions
                .candidates
                .iter()
                .map(|c| c.replacement.clone())
                .collect();
            (completions.start, names)
        };
        assert_eq!(
            names(""),
            (
                0,
                vec!["start", "list", "help", "quit", "exit"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            )
        );
        assert_eq!(names("  l"), (2, vec!["list".to_string()]));
        assert_eq!(names("e"), (0, vec!["exit".to_string()]));
        // Nothing for the task name.
        assert_eq!(names("start a"), (0, vec![]));
        let start = &CommandCompleter.complete("st", 2).candidates[0];
        assert!(start.append_space);
        assert_eq!(start.description.as_deref(), Some("start a task"));
    }

    #[test]
    fn tab_completes_the_command_at_the_prompt() {
        let mut app = new_app();
        type_text(&mut app, "st");
        app.handle_event(key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.editor.value(), "start ");
        let mut app = new_app();
        app.handle_event(key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.editor.value(),
            "start",
            "the menu opens on the first command"
        );
        assert!(app.editor.extra_rows(100) > 0);
    }

    #[test]
    fn the_history_path_follows_tilefab_history_then_xdg_then_home() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                vars.iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        let path = |vars| history_path(env(vars));
        assert_eq!(
            path(&[("TILEFAB_HISTORY", "/tmp/h"), ("HOME", "/home/u")]),
            Some(PathBuf::from("/tmp/h"))
        );
        // An empty value disables the file.
        assert_eq!(path(&[("TILEFAB_HISTORY", ""), ("HOME", "/home/u")]), None);
        assert_eq!(
            path(&[("XDG_STATE_HOME", "/state"), ("HOME", "/home/u")]),
            Some(PathBuf::from("/state/tilefab/history"))
        );
        // A relative XDG_STATE_HOME is ignored.
        assert_eq!(
            path(&[("XDG_STATE_HOME", "state"), ("HOME", "/home/u")]),
            Some(PathBuf::from("/home/u/.local/state/tilefab/history"))
        );
        assert_eq!(path(&[]), None);
    }

    #[test]
    fn submitted_lines_go_to_the_history_file() {
        let dir = std::env::temp_dir().join(format!("tilefab-demo-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("history");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "list\n").unwrap();
        let mut app = new_app();
        app.load_history(&path);
        assert_eq!(app.editor.history().get(0), Some("list"));
        type_text(&mut app, "help");
        app.handle_event(key(KeyCode::Enter, KeyModifiers::NONE));
        // A line starting with a space is not recorded.
        type_text(&mut app, " list");
        app.handle_event(key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "list\nhelp\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unusable_history_file_is_reported_once() {
        let dir =
            std::env::temp_dir().join(format!("tilefab-demo-bad-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = new_app();
        // A directory is not a history file.
        let logs = logging::capture(|| app.load_history(&dir));
        assert_eq!(logs.len(), 1, "{logs:?}");
        assert!(
            logs[0].starts_with("[WARN] cannot use the history file"),
            "{logs:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_paste_is_inserted_on_the_prompt_line_without_running() {
        let mut app = new_app();
        app.handle_event(AppEvent::Paste("list\nhelp\n".into()));
        assert_eq!(app.editor.value(), "list help ");
        assert_eq!(app.output, []);
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
        assert_eq!(app.output, []);
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
                "[INFO] commands: start <name>, list, help, quit, exit (or Ctrl+C, Ctrl+D to quit)",
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
