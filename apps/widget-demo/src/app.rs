//! Application state, event handling, and command processing.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tilefab::crossterm::event::KeyEvent;
use tilefab::{
    Candidate, Completer, Completions, EditorConfig, EditorEvent, History, Line, LineEditor,
    LineWidget,
};
use tokio::sync::mpsc::Sender;
use tracing::{Level, error, info, warn};

use crate::MAX_TASKS;
use crate::line;
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
    /// Task progress, between 0.0 and 1.0, and the step it has reached.
    Progress {
        id: TaskId,
        ratio: f64,
        step: &'static str,
    },
    /// A task has finished.
    TaskDone(TaskId),
    /// Regular tick: updates the time of the task lines and forces a periodic redraw.
    Tick,
    /// Headless mode (no TTY): line read from stdin.
    Line(String),
    /// Headless mode: end of stdin.
    InputClosed,
    /// Termination signal received (`SIGTERM`, `SIGINT`, `SIGHUP`): quit through
    /// the normal path so the terminal is restored.
    Quit,
    /// The keyboard could not be read any more (the error message): quit through the normal
    /// path, instead of running on without input in raw mode.
    InputFailed(String),
}

/// A command entered at the prompt.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Empty line: nothing to do.
    Empty,
    /// Start a task by its name, with its own composition or the current one.
    Start {
        name: String,
        composition: Option<String>,
    },
    List,
    /// Set the composition of the tasks started next; `None`: show the current one.
    Compose(Option<String>),
    Help,
    Quit,
    /// Hidden command for testing terminal restoration. Debug builds only
    /// in a release build, `panic` is an unknown command.
    #[cfg(debug_assertions)]
    Panic,
    /// Hidden command for checking the scrollback (`count <n>`): write `n` numbered log
    /// lines at once, to find again by scrolling up. Debug builds only, like `panic`.
    #[cfg(debug_assertions)]
    Count(u32),
}

/// Most lines `count` writes at once.
#[cfg(debug_assertions)]
pub const MAX_COUNT: u32 = 10_000;

/// Command parsing error.
#[derive(Debug, PartialEq, Eq)]
pub enum CommandError {
    /// `start` without a task name.
    MissingName,
    /// `compose` with a composition that is not between double quotes.
    Quotes(QuoteError),
    /// Unrecognized first word.
    Unknown(String),
}

/// Why a quoted composition could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteError {
    /// It does not start with `"`.
    MissingOpening,
    /// No `"` after the opening one.
    MissingClosing,
    /// Text after the closing `"`.
    TextAfter,
}

impl std::fmt::Display for QuoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingOpening => "the composition goes between double quotes",
            Self::MissingClosing => "missing closing double quote",
            Self::TextAfter => "text after the closing double quote",
        })
    }
}

/// The text between the double quotes of `text` (`"B1 T2 L"` → `B1 T2 L`).
fn quoted(text: &str) -> Result<&str, QuoteError> {
    let inner = text.strip_prefix('"').ok_or(QuoteError::MissingOpening)?;
    let (composition, after) = inner.split_once('"').ok_or(QuoteError::MissingClosing)?;
    if after.trim().is_empty() {
        Ok(composition)
    } else {
        Err(QuoteError::TextAfter)
    }
}

/// The argument of `start`: a name, optionally followed by a composition between double
/// quotes (`alpha "N B0 P"`), for this task only.
fn start_arguments(rest: &str) -> (&str, Option<&str>) {
    if let Some(without_last) = rest.strip_suffix('"')
        && let Some(open) = without_last.rfind('"')
    {
        return (rest[..open].trim_end(), Some(&without_last[open + 1..]));
    }
    (rest, None)
}

/// What a command word does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Start,
    List,
    Compose,
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
        usage: "start <name> [\"<composition>\"]",
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
        name: "compose",
        usage: "compose [\"<composition>\"]",
        description: "choose how new tasks are drawn",
        kind: Kind::Compose,
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
        #[cfg(debug_assertions)]
        if word == "count" {
            return match rest.parse() {
                Ok(n @ 1..=MAX_COUNT) => Ok(Self::Count(n)),
                _ => Err(CommandError::Unknown(format!("count {rest}"))),
            };
        }
        let Some(spec) = COMMANDS.iter().find(|spec| spec.name == word) else {
            return Err(CommandError::Unknown(word.to_string()));
        };
        match spec.kind {
            Kind::Start => match start_arguments(rest) {
                ("", _) => Err(CommandError::MissingName),
                (name, composition) => Ok(Self::Start {
                    name: name.to_string(),
                    composition: composition.map(str::to_string),
                }),
            },
            Kind::List => Ok(Self::List),
            Kind::Compose if rest.is_empty() => Ok(Self::Compose(None)),
            Kind::Compose => quoted(rest)
                .map(|composition| Self::Compose(Some(composition.to_string())))
                .map_err(CommandError::Quotes),
            Kind::Help => Ok(Self::Help),
            Kind::Quit => Ok(Self::Quit),
        }
    }

    /// The `help` message, on several lines: the command names, the usage of the commands
    /// that take arguments, the ways to quit, then the letters of a composition.
    fn help() -> String {
        let names: Vec<&str> = COMMANDS.iter().map(|spec| spec.name).collect();
        let mut lines = vec![format!("commands: {}", names.join(", "))];
        lines.extend(
            COMMANDS
                .iter()
                .filter(|spec| spec.usage != spec.name)
                .map(|spec| format!("  {}", spec.usage)),
        );
        let quits: Vec<&str> = COMMANDS
            .iter()
            .filter(|spec| spec.kind == Kind::Quit)
            .map(|spec| spec.name)
            .collect();
        lines.push(format!(
            "  {} (or Ctrl+C, Ctrl+D to quit)",
            quits.join(", ")
        ));
        lines.extend(COMPOSITION_HELP.iter().map(|line| format!("  {line}")));
        lines.join("\n")
    }
}

/// The letters of a composition, for `help`.
const COMPOSITION_HELP: &[&str] = &[
    "composition: tokens separated by one space, for instance \"N B0 B1 T2 B2 B0 S B0 I B0 P B0 L\"",
    "  S symbol of the state, B0-B9 space [ ] - : | ( ) { },",
    "  D0-D2 date, T0-T5 time, P[3-9] progress, K state keyword, N task name,",
    "  L[size] log, V[size][U][/] downloaded size, I[size][B|L|D] progress bar,",
    "  B bar █░ (default), L line ━─, D dots ●·",
];

/// Completes the command name (the first word); nothing for the arguments.
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
                if matches!(spec.kind, Kind::Start | Kind::Compose) {
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

/// Most lines kept in `App::output` while they cannot be shown.
pub const MAX_KEPT_LINES: usize = 1000;
/// Most bytes of text kept in `App::output` while it cannot be shown.
pub const MAX_KEPT_BYTES: usize = 1024 * 1024;

/// A running task, as seen by the UI.
#[derive(Debug)]
pub struct Task {
    pub id: TaskId,
    pub name: String,
    pub ratio: f64,
    /// The composition of its line: the current one when it started, or its own.
    pub composition: String,
    /// Its line in the viewport.
    pub widget: LineWidget,
    /// Simulated download size shown by `V`, in MB.
    pub total_mb: f64,
    /// Width of the name column the widget was built with (see `line::name_column`).
    pub column: usize,
    /// The last step reported, shown again when the widget is built again.
    step: &'static str,
}

impl Task {
    /// A task drawn with `composition`, its name in a column `column` wide.
    ///
    /// # Errors
    ///
    /// The error of an invalid composition.
    pub fn new(
        id: TaskId,
        name: String,
        composition: &str,
        column: usize,
    ) -> Result<Self, tilefab::BuildError> {
        let mut widget = line::build(composition, &name, column)?;
        // Without `D` and `T`, there is no time to show.
        widget.set_time(line::now()).ok();
        Ok(Self {
            id,
            name,
            ratio: 0.0,
            composition: composition.to_string(),
            widget,
            total_mb: f64::from(fastrand::u16(5..=500)),
            column,
            step: "",
        })
    }

    /// Show the progress `ratio` and the current `step`.
    fn progress(&mut self, ratio: f64, step: &'static str) {
        self.ratio = ratio;
        self.step = step;
        line::update(
            &mut self.widget,
            &self.composition,
            ratio,
            step,
            self.total_mb,
        );
    }

    /// Build the widget again with a name column `column` wide, showing the same values.
    fn set_column(&mut self, column: usize) {
        // The composition already built a widget: it cannot fail now.
        if let Ok(widget) = line::build(&self.composition, &self.name, column) {
            self.widget = widget;
            self.column = column;
            self.widget.set_time(line::now()).ok();
            if self.ratio > 0.0 {
                self.progress(self.ratio, self.step);
            }
        }
    }
}

pub struct App {
    /// Prompt line editor.
    pub editor: LineEditor,
    /// Running tasks, in launch order.
    pub tasks: Vec<Task>,
    /// Lines to insert above the viewport on the next loop iteration.
    pub output: Vec<Line<'static>>,
    /// Lines dropped from `output` while they could not be shown (see [`App::limit_output`]).
    pub dropped_output: usize,
    /// Composition given to the tasks started next (`compose`).
    pub composition: String,
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
            dropped_output: 0,
            composition: line::DEFAULT_COMPOSITION.to_string(),
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
            AppEvent::Progress { id, ratio, step } => {
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
                    task.progress(ratio, step);
                }
            }
            AppEvent::Tick => {
                let now = line::now();
                for task in &mut self.tasks {
                    // Without `D` and `T`, there is no time to show.
                    task.widget.set_time(now).ok();
                }
            }
            AppEvent::TaskDone(id) => {
                if let Some(pos) = self.tasks.iter().position(|t| t.id == id) {
                    let task = self.tasks.remove(pos);
                    self.log_info(&format!("task \"{}\" finished", task.name));
                    self.align_names();
                }
            }
            AppEvent::Line(line) => self.submit(&line),
            AppEvent::Quit => self.should_quit = true,
            AppEvent::InputFailed(error) => {
                error!("keyboard input failed ({error}): quitting");
                self.should_quit = true;
            }
            // The headless loop handles the end of stdin.
            AppEvent::Resize | AppEvent::InputClosed => {}
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

    /// Keep only the newest lines of `output`, at most [`MAX_KEPT_LINES`] and
    /// [`MAX_KEPT_BYTES`] (the newest line is always kept), while they cannot be shown (a
    /// terminal with no columns); the others are counted in `dropped_output`.
    pub fn limit_output(&mut self) {
        let size = |line: &Line<'_>| line.spans.iter().map(|s| s.content.len()).sum::<usize>();
        let mut bytes: usize = self.output.iter().map(size).sum();
        let mut drop = 0;
        while self.output.len() - drop > 1
            && (self.output.len() - drop > MAX_KEPT_LINES || bytes > MAX_KEPT_BYTES)
        {
            bytes -= size(&self.output[drop]);
            drop += 1;
        }
        self.output.drain(..drop);
        self.dropped_output += drop;
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
            Ok(Command::Start { name, composition }) => self.start_task(name, composition),
            Ok(Command::List) => self.list_tasks(),
            Ok(Command::Compose(None)) => info!("composition: \"{}\"", self.composition),
            Ok(Command::Compose(Some(composition))) => self.compose(composition),
            Ok(Command::Help) => info!("{}", Command::help()),
            Ok(Command::Quit) => self.should_quit = true,
            #[cfg(debug_assertions)]
            Ok(Command::Panic) => panic!("deliberate panic (`panic` command)"),
            #[cfg(debug_assertions)]
            Ok(Command::Count(n)) => {
                // Straight into the output, not through tracing: its channel could drop some.
                for i in 1..=n {
                    self.log_info(&format!("count {i:05}/{n:05}"));
                }
            }
            Err(CommandError::MissingName) => error!("usage: start <name> [\"<composition>\"]"),
            Err(CommandError::Quotes(error)) => {
                error!("{error}: compose \"<composition>\"");
            }
            Err(CommandError::Unknown(word)) => {
                warn!("unknown command: \"{word}\" (type help)");
            }
        }
    }

    /// Make `composition` the current one if it builds a widget; otherwise keep the current
    /// one. Each warning is logged.
    fn compose(&mut self, composition: String) {
        match line::check(&composition) {
            Ok(warnings) => {
                for warning in warnings {
                    warn!("{warning}");
                }
                info!("new tasks will be drawn with \"{composition}\"");
                self.composition = composition;
            }
            Err(error) => error!("invalid composition: {error}"),
        }
    }

    /// Start task `name`, drawn with `composition` (for this task only) or the current one.
    fn start_task(&mut self, name: String, composition: Option<String>) {
        if self.tasks.len() >= MAX_TASKS {
            warn!("already {MAX_TASKS} tasks running: \"{name}\" refused");
            return;
        }
        let own = composition.is_some();
        let composition = composition.unwrap_or_else(|| self.composition.clone());
        let column = line::name_column(
            self.tasks
                .iter()
                .map(|task| task.name.as_str())
                .chain([name.as_str()]),
        );
        let task = match Task::new(self.next_id, name, &composition, column) {
            Ok(task) => task,
            Err(error) => {
                error!("invalid composition: {error}");
                return;
            }
        };
        if own {
            for warning in task.widget.warnings() {
                warn!("{warning}");
            }
        }
        self.next_id += 1;
        let duration = tasks::spawn(task.id, self.tx.clone());
        self.log_info(&format!(
            "task \"{}\" started (duration {:.1} s)",
            task.name,
            duration.as_secs_f64()
        ));
        self.tasks.push(task);
        self.align_names();
    }

    /// Give every task the name column of the longest running name, so that what follows
    /// the names lines up (the bars of the default composition).
    fn align_names(&mut self) {
        let column = line::name_column(self.tasks.iter().map(|task| task.name.as_str()));
        for task in &mut self.tasks {
            if task.column != column {
                task.set_column(column);
            }
        }
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

    #[cfg(debug_assertions)]
    #[test]
    fn count_writes_numbered_lines_in_debug_builds() {
        assert_eq!(Command::parse("count 3"), Ok(Command::Count(3)));
        assert!(Command::parse("count 0").is_err());
        assert!(Command::parse("count 10001").is_err());
        assert!(Command::parse("count x").is_err());
        let mut app = new_app();
        app.submit("count 3");
        let lines: Vec<String> = app.output.iter().map(|l| text(l)).collect();
        assert_eq!(
            lines,
            [
                "[INFO] count 00001/00003",
                "[INFO] count 00002/00003",
                "[INFO] count 00003/00003"
            ]
        );
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
            Ok(start("alpha", None))
        );
    }

    #[test]
    fn parse_start_takes_rest_of_line_as_name() {
        assert_eq!(
            Command::parse("start mon été 🚀"),
            Ok(start("mon été 🚀", None))
        );
        assert_eq!(Command::parse("start\talpha"), Ok(start("alpha", None)));
    }

    fn start(name: &str, composition: Option<&str>) -> Command {
        Command::Start {
            name: name.into(),
            composition: composition.map(String::from),
        }
    }

    #[test]
    fn parse_start_with_its_own_composition() {
        assert_eq!(
            Command::parse("start my task \"N B0 P\""),
            Ok(start("my task", Some("N B0 P")))
        );
        assert_eq!(Command::parse("start a \"\""), Ok(start("a", Some(""))));
        // Only a trailing quoted part is a composition.
        assert_eq!(
            Command::parse("start say \"hi\" twice"),
            Ok(start("say \"hi\" twice", None))
        );
        assert_eq!(Command::parse("start x\""), Ok(start("x\"", None)));
        assert_eq!(
            Command::parse("start \"N P\""),
            Err(CommandError::MissingName)
        );
    }

    #[test]
    fn parse_compose() {
        assert_eq!(Command::parse("compose"), Ok(Command::Compose(None)));
        assert_eq!(
            Command::parse("compose \"B1 T2 B2 B0 S B0 L\""),
            Ok(Command::Compose(Some("B1 T2 B2 B0 S B0 L".into())))
        );
        // The spacing inside the quotes is kept: the widget checks it.
        assert_eq!(
            Command::parse("compose \" L \"  "),
            Ok(Command::Compose(Some(" L ".into())))
        );
        let errors = [
            ("compose L", QuoteError::MissingOpening),
            ("compose \"L", QuoteError::MissingClosing),
            ("compose \"L\" P", QuoteError::TextAfter),
        ];
        for (line, error) in errors {
            assert_eq!(
                Command::parse(line),
                Err(CommandError::Quotes(error)),
                "{line}"
            );
        }
    }

    #[tokio::test]
    async fn each_task_keeps_the_composition_current_when_it_started() {
        let mut app = new_app();
        app.submit("start one");
        app.submit("compose \"N B0 P\"");
        app.submit("start two");
        app.submit("start three \"P N\"");
        app.submit("compose \"L\"");
        let compositions: Vec<&str> = app.tasks.iter().map(|t| t.composition.as_str()).collect();
        assert_eq!(compositions, [line::DEFAULT_COMPOSITION, "N B0 P", "P N"]);
        assert_eq!(app.composition, "L");
        // A rejected composition keeps the current one.
        let logs = logging::capture(|| app.submit("compose \"L X\""));
        assert_eq!(app.composition, "L");
        assert_eq!(
            logs,
            ["[ERROR] invalid composition: token 2 (\"X\"): unknown component"]
        );
        // A task with an invalid composition is not started.
        let logs = logging::capture(|| app.submit("start four \"Q\""));
        assert_eq!(app.tasks.len(), 3);
        assert_eq!(logs.len(), 1, "{logs:?}");
    }

    #[test]
    fn compose_shows_the_current_composition_and_the_warnings() {
        let mut app = new_app();
        let logs = logging::capture(|| {
            app.submit("compose");
            app.submit("compose \"L100\"");
            app.submit("compose \"L");
        });
        assert_eq!(
            logs,
            [
                format!("[INFO] composition: \"{}\"", line::DEFAULT_COMPOSITION),
                "[WARN] the widget is 100 columns wide, more than its target width of 80"
                    .to_string(),
                "[INFO] new tasks will be drawn with \"L100\"".to_string(),
                "[ERROR] missing closing double quote: compose \"<composition>\"".to_string(),
            ]
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
        app.handle_event(AppEvent::Progress {
            id,
            ratio: 0.5,
            step: "step",
        });
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
    fn the_completer_completes_the_command_names() {
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
                vec!["start", "list", "compose", "help", "quit", "exit"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            )
        );
        assert_eq!(names("  l"), (2, vec!["list".to_string()]));
        assert_eq!(names("e"), (0, vec!["exit".to_string()]));
        // Nothing for the task name.
        assert_eq!(names("start a"), (0, vec![]));
        // Nothing for the arguments.
        assert_eq!(names("compose \"B"), (0, vec![]));
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
        let dir = std::env::temp_dir().join(format!("widget-demo-history-{}", std::process::id()));
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
            std::env::temp_dir().join(format!("widget-demo-bad-history-{}", std::process::id()));
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
        app.handle_event(AppEvent::Progress {
            id: 42,
            ratio: 0.5,
            step: "step",
        });
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
                "[INFO] commands: start, list, compose, help, quit, exit",
                "  start <name> [\"<composition>\"]",
                "  compose [\"<composition>\"]",
                "  quit, exit (or Ctrl+C, Ctrl+D to quit)",
                "  composition: tokens separated by one space, for instance \"N B0 B1 T2 B2 B0 S B0 I B0 P B0 L\"",
                "    S symbol of the state, B0-B9 space [ ] - : | ( ) { },",
                "    D0-D2 date, T0-T5 time, P[3-9] progress, K state keyword, N task name,",
                "    L[size] log, V[size][U][/] downloaded size, I[size][B|L|D] progress bar,",
                "    B bar █░ (default), L line ━─, D dots ●·",
                "[INFO] no task running",
                "[ERROR] usage: start <name> [\"<composition>\"]",
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
            app.handle_event(AppEvent::Progress {
                id,
                ratio: 0.25,
                step: "step",
            });
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
