//! The line widget of each task: the lists of values the compositions can use, the default
//! composition, and how a task drives its widget (state, progress, value, step, time).

use tilefab::{BuildError, Color, Entry, LineWidget, Modifier, Style, WallTime, Warning};

/// The composition current at startup: `alpha [05.123] 🟠 ██████████░░░░░░  45% compiling`.
/// The name comes first: the pty checks of `tools/` find the task rows by their name.
pub const DEFAULT_COMPOSITION: &str = "N B2 B0 T2 B1 B2 S B2 I B2 P B2 L";

/// Unit of the simulated download size shown by `V` (index in [`units`]).
const MEGABYTES: usize = 2;
/// Progress from which a task is "almost done".
const ALMOST_DONE: f64 = 0.8;

/// The state of a task, the index of its entry in the `symbols` and `keywords` lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Started, no progress yet.
    Waiting = 0,
    Running = 1,
    AlmostDone = 2,
    Done = 3,
}

impl State {
    /// The state of a task at `ratio` (0.0–1.0).
    pub fn at(ratio: f64) -> Self {
        match ratio {
            r if r >= 1.0 => Self::Done,
            r if r >= ALMOST_DONE => Self::AlmostDone,
            r if r > 0.0 => Self::Running,
            _ => Self::Waiting,
        }
    }
}

fn gray() -> Style {
    Style::new().fg(Color::DarkGray)
}

/// `S`: one symbol per [`State`].
fn symbols() -> [Entry; 4] {
    ["🔴", "🟠", "🟡", "🟢"].map(Entry::new)
}

/// `K`: one keyword per [`State`].
fn keywords() -> [Entry; 4] {
    [
        Entry::styled("waiting", gray()),
        Entry::styled("running", Style::new().fg(Color::Yellow)),
        Entry::styled("finishing", Style::new().fg(Color::Cyan)),
        Entry::styled("done", Style::new().fg(Color::Green)),
    ]
}

/// `B0` to `B9`.
fn barriers() -> [Entry; 10] {
    ["[", "]", " ", " - ", ": ", " | ", "(", ")", "/", "  "].map(|text| Entry::styled(text, gray()))
}

/// `U`: the units of `V`.
fn units() -> [Entry; 4] {
    ["B", "KB", "MB", "GB"].map(|text| Entry::styled(text, gray()))
}

/// The widget of task `name` drawn with `composition`; the name is entry 0 of `names`
/// (`N` or `N0`).
///
/// # Errors
///
/// The [`BuildError`] of an invalid composition.
pub fn build(composition: &str, name: &str) -> Result<LineWidget, BuildError> {
    // The widget refuses control characters in its entries: a name read from stdin may
    // hold some.
    let name: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    LineWidget::builder(composition)
        .symbols(symbols())
        .barriers(barriers())
        .keywords(keywords())
        .names([Entry::styled(
            name,
            Style::new().add_modifier(Modifier::BOLD),
        )])
        .units(units())
        .illustration(
            Entry::styled("█", Style::new().fg(Color::Cyan)),
            Entry::styled("░", gray()),
        )
        .date_style(gray())
        .time_style(gray())
        .progress_style(Style::new().add_modifier(Modifier::BOLD))
        .value_styles(Style::new(), gray())
        .build()
}

/// Check `composition` by building a widget with a placeholder name, and return its
/// warnings.
///
/// # Errors
///
/// The [`BuildError`] of an invalid composition.
pub fn check(composition: &str) -> Result<Vec<Warning>, BuildError> {
    build(composition, "task").map(|widget| widget.warnings().to_vec())
}

/// Whether the `V` of `composition` has a unit (`U`) and a total (`/`): `set_value` must
/// match it exactly.
fn value_shape(composition: &str) -> (bool, bool) {
    composition
        .split(' ')
        .find(|token| token.starts_with('V'))
        .map_or((false, false), |token| {
            (token.contains('U'), token.contains('/'))
        })
}

/// Show the progress of a task: `ratio` (0.0–1.0), its current `step`, and `done_mb` of
/// `total_mb` downloaded.
pub fn update(widget: &mut LineWidget, composition: &str, ratio: f64, step: &str, total_mb: f64) {
    let state = State::at(ratio) as usize;
    let (unit, total) = value_shape(composition);
    let unit = unit.then_some(MEGABYTES);
    let total = total.then_some((total_mb, unit));
    // Each setter fails only when the composition lacks its component (or has a fixed one):
    // nothing to show then.
    widget.set_progress(ratio * 100.0).ok();
    widget.set_symbol(0, state).ok();
    widget.set_keyword(state).ok();
    widget.set_value(ratio * total_mb, unit, total).ok();
    widget.set_log(step);
}

/// The local date and time now, for `D` and `T`.
pub fn now() -> WallTime {
    wall_time(&jiff::ZonedDateTime::now())
}

/// `time` as a [`WallTime`] (a year outside 0–9999 becomes 0, which `set_time` refuses).
fn wall_time(time: &jiff::ZonedDateTime) -> WallTime {
    let small = |value: i8| u8::try_from(value).unwrap_or(0);
    WallTime {
        year: u16::try_from(time.year()).unwrap_or(0),
        month: small(time.month()),
        day: small(time.day()),
        hour: small(time.hour()),
        minute: small(time.minute()),
        second: small(time.second()),
        millis: u16::try_from(time.millisecond()).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use tilefab::{Buffer, Rect, Widget};

    use super::*;

    fn row(widget: &LineWidget, width: u16) -> String {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, 1));
        widget.render(buf.area(), &mut buf);
        buf.lines()[0].trim_end().to_owned()
    }

    #[test]
    fn the_default_composition_shows_the_task() {
        let mut widget = build(DEFAULT_COMPOSITION, "alpha").unwrap();
        assert_eq!(widget.warnings(), []);
        update(&mut widget, DEFAULT_COMPOSITION, 0.45, "compiling", 10.0);
        let text = row(&widget, 60);
        assert!(text.starts_with("alpha [00.000] 🟠 █"), "{text}");
        assert!(text.ends_with("░  45% compiling"), "{text}");
    }

    #[test]
    fn states_follow_the_progress() {
        assert_eq!(State::at(0.0), State::Waiting);
        assert_eq!(State::at(0.3), State::Running);
        assert_eq!(State::at(0.85), State::AlmostDone);
        assert_eq!(State::at(1.0), State::Done);
    }

    #[test]
    fn a_composition_can_use_every_list() {
        let composition = "D0 B2 T3 B3 S B4 K B5 N B6 V5U/ B7 I10 P5 B9 L";
        assert!(check(composition).is_ok(), "{:?}", check(composition));
        let mut widget = build(composition, "beta").unwrap();
        widget.set_time(now()).unwrap();
        update(&mut widget, composition, 1.0, "done", 50.0);
        let text = row(&widget, 120);
        assert!(text.contains("🟢: done"), "{text}");
        assert!(text.contains("50.0MB/50.0MB"), "{text}");
    }

    #[test]
    fn values_follow_the_shape_of_v() {
        for composition in ["V5", "V5U", "V5/", "V5U/"] {
            let mut widget = build(composition, "t").unwrap();
            update(&mut widget, composition, 0.5, "", 20.0);
            assert!(
                row(&widget, 20).trim_start().starts_with("10.0"),
                "{composition}"
            );
        }
    }

    #[test]
    fn control_characters_of_a_name_become_spaces() {
        let widget = build("N", "a\u{1b}b").unwrap();
        assert_eq!(row(&widget, 10), "a b");
    }

    #[test]
    fn invalid_compositions_and_warnings() {
        assert!(check("B0 X").is_err());
        assert!(check("L L").is_err());
        assert_eq!(check("L200").unwrap().len(), 1);
    }

    #[test]
    fn now_is_a_valid_wall_time() {
        assert_eq!(now().validate(), Ok(()));
    }
}
