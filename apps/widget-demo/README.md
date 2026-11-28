# widget-demo

Demo of the [tilefab](https://crates.io/crates/tilefab) TUI library: log lines scroll above an
**inline viewport** that shows one progress bar per running task, with an input prompt on its last
line. The viewport grows and shrinks as tasks start and finish.

```text
> start beta
[INFO] task "alpha" started (duration 6.3 s)
[INFO] task "beta" started (duration 5.5 s)
alpha [05.123] 🟠 ███████████████░░░░░░░░░░░░░░░░░░░░░░░░░░  36% compiling
beta  [05.123] 🟠 ██████████████████░░░░░░░░░░░░░░░░░░░░░░░  42% compiling

> start gam█
```

To write a program of your own with tilefab, start with its
[quickstart](https://github.com/tilefab/tilefab/blob/main/crates/tilefab/QUICKSTART.md) rather than with this demo, which
shows everything at once.

## Install and run

Not on crates.io yet: run it from a clone of the repository, or install it from there.

```sh
git clone https://github.com/tilefab/tilefab
cd tilefab
cargo run                               # or: cargo install --path apps/widget-demo, then widget-demo
```

To hand a built binary to someone else, use `tools/dist.sh` (`target/dist/release/widget-demo`):
a plain release build holds paths under your home directory (the panic locations of the
dependencies), and therefore your user name; the script rewrites them, then checks that none is
left.

Outside a terminal (stdin or stdout redirected), the tool switches to **degraded mode**: no UI, commands are read line by line from stdin and logs are written to stderr. When stdin ends, it waits for the started tasks to finish.

```sh
echo "start a" | cargo run
printf 'start a\nstart b\nlist\n' | cargo run
```

In this mode there is no prompt, and `list` is the only way to see progress. All the piped lines are read at once, so in the second example `list` runs right after the two `start`, before any progress report (sent every 100 ms): both tasks show `0 %`. To see progress, wait before `list`:

```sh
{ printf 'start a\nstart b\n'; sleep 3; echo list; } | cargo run
```

Keys typed while it waits are echoed by the terminal but not read: stdin is the pipe.

## Commands

| Command | Effect |
|---|---|
| `start <name>` | starts a simulated task (3 to 10 s), drawn with the current composition; the name is the rest of the line |
| `start <name> "<composition>"` | the same, with a composition for this task only (the current one does not change) |
| `list` | logs the running tasks and their progress |
| `compose "<composition>"` | sets the composition of the tasks started next, for instance `compose "N B0 P B0 L"`; the composition goes between double quotes. An invalid composition is an ERROR and the current one is kept; a composition wider than 80 columns gives a WARN. Without an argument, shows the current composition. Each task keeps the composition it started with |
| `help` | logs the list of commands |
| `quit`, `exit`, Ctrl+C, Ctrl+D (on an empty line) | clean exit |

At most 7 simultaneous tasks (`MAX_TASKS`); beyond that, the command is refused with a WARN. An unknown command gives a WARN, `start` without a name an ERROR. A name that ends with a quoted part is read as a name and a composition: `start say "hi"` starts `say` with the composition `hi`, which is refused.

## Task lines

Each task is drawn on one row by a `LineWidget` of `tilefab-widget`, composed from a short string: one letter per component, separated by one space. The default composition is `N B0 B4 T2 B5 B0 S B0 I B0 P B0 L`:

```
alpha [05.123] 🟠 ████████████████░░░░░░░░░░░░░░  52% compiling
```

| Token | Component in the demo |
|---|---|
| `S`, `S0`–`S3` | state symbol: 🔴 waiting, 🟠 running, 🟡 almost done, 🟢 done (`S` follows the task, `S2` is always 🟡) |
| `B0`–`B9` | barriers: space, `-`, `:`, `\|`, `[`, `]`, `(`, `)`, `{`, `}` |
| `D0`–`D2` | date: `YYYY-MM-DD`, `YY-MM-DD`, `MM-DD` (local time, from the `jiff` crate) |
| `T0`–`T5` | time: `HH:MM:SS.SSS`, `MM:SS.SSS`, `SS.SSS`, `HH:MM:SS`, `HH:MM`, `MM:SS` |
| `P`, `P3`–`P9` | progress, rounded down, with `n − 4` decimals from `P5` (`P5`: ` 99.9%`) |
| `K`, `K0`–`K3` | state keyword: `waiting`, `running`, `finishing`, `done` |
| `N` | the task name |
| `L`, `L40` | the current step of the task (`compiling`…): elastic, or a fixed size |
| `V`, `V5U/` | simulated download: size of each number, `U` for the unit (MB), `/` for the total (`12.5MB/40.0MB`) |
| `I`, `I20`, `IL`, `I20D` | progress bar: elastic (10 columns at least when there is room), or a fixed size; then its look: `B` bar `█░` (default), `L` line `━─`, `D` dots `●·`, `S` squares `■□`, `O` circles `●○`, `R` diamonds `◆◇`, `G` segments `▰▱`, `H` shaded blocks `▓░`, `V` vertical bars `┃│`, `A` ASCII `#-`, `C` chevrons `>·` |

Any token can end with a **paint**, which changes the style of that component only: `.`, then at least one of, in this order: a text color, `:` and a background color, `*` for bold. Colors: `K` black, `R` red, `G` green, `Y` yellow, `B` blue, `M` magenta, `C` cyan, `W` white (the ANSI white, a light gray), followed by `+` for the bright version (`K+` dark gray, `W+` bright white). `I` and `V` take a second part after `/`, for the rest of the bar and for the units. The paint is laid over the demo's own styles (gray barriers and time, cyan bar, bold name and percentage): what it does not set is kept. For instance:

```
start build "N.W+:B* B0 B4.Y T2.Y B5.Y B0 IL.G/K+ B0 P.G* B0 L.C"
```

shows a bright white bold name on blue, a yellow time between yellow brackets, a green line on a dark gray track, a bold green percentage and a cyan step.

`D`, `T`, `P`, `K`, `L`, `V` and `I` appear at most once; `S`, `B` and `N` can be repeated. The line is laid out again at every frame, with the whole terminal width: the extra columns go to the elastic `L`, otherwise to the elastic `I`. When the window is too narrow, `L` and `I` shrink first (down to 2 columns, a cut text ends with `…`), then the names, then the last components are dropped; a barrier stays, which shows the line was cut. `help` lists the tokens.
The hidden `panic` command triggers a deliberate panic, to check that the terminal is properly restored. It exists in debug builds only (`cargo build`, used by `tools/check_app.py`); in a release build, `panic` is an unknown command. The hidden `count <n>` command (debug builds too) writes `n` numbered log lines at once, to check that every line can be found again by scrolling up in the terminal window (scenario `tools/scenarios/scrollback.scn`).

## Line editing

The prompt uses the line editor of `tilefab-input`: readline-style Emacs keys, Unicode-aware (the cursor moves by grapheme: an accented letter or an emoji is one step). A long line scrolls horizontally, keeping a few columns of context around the cursor; a dim `…` shows hidden text.

| Keys | Action |
|---|---|
| ←/→, Ctrl+B/Ctrl+F | one character left / right |
| Ctrl+←/→, Alt+←/→, Alt+B/Alt+F | previous word start / next word end |
| Home/End, Ctrl+A/Ctrl+E | start / end of the line |
| Backspace (Ctrl+H), Delete, Ctrl+D | delete the character before / under the cursor (Ctrl+D on an empty line quits) |
| Alt+Backspace, Alt+D (Ctrl+Delete) | delete the previous word / to the end of the next word |
| Ctrl+W | delete the previous whitespace-delimited word |
| Ctrl+U, Ctrl+K | delete to the start / to the end of the line |
| Ctrl+Y, then Alt+Y | paste back the last deleted text, then older ones (consecutive deletions are pasted back together) |
| Ctrl+T, Alt+T | swap two characters / two words |
| Alt+U, Alt+L, Alt+C | upper-case / lower-case / capitalize to the end of the word |
| Ctrl+_ (Ctrl+/, Ctrl+7, Ctrl+Z), Alt+_ | undo (a typed word at once) / redo |
| ↑/↓ | previous / next history entry that starts with what is typed |
| Ctrl+P/Ctrl+N, Alt+</Alt+> | previous / next history entry, oldest / newest |
| Ctrl+R | search the history (see below) |
| Tab, Shift+Tab | complete the command name (see below); with nothing to complete, Tab accepts the hint |
| Esc | hide the hint; back to the typed line while browsing the history |
| Enter (Ctrl+J, Ctrl+M) | run the line |

**History.** Lines run at the prompt are saved, each one once (the latest wins), up to 1000. A line that starts with a space is not saved. The file is `$TILEFAB_HISTORY` if set, else `$XDG_STATE_HOME/tilefab/history`, else `~/.local/state/tilefab/history`; on Windows, `%LOCALAPPDATA%\tilefab\history` replaces the last two. On Unix, it is private to the user (mode `0600`). `TILEFAB_HISTORY= cargo run` runs without a history file. The headless mode has no history.

**Search.** Ctrl+R replaces the prompt with `(search) query: ` and shows the newest entry that contains what is typed (case-insensitive unless the query has an upper-case letter), the match underlined. Ctrl+R / Ctrl+S go to older / newer matches; `(failing search)` means there is no other match. Enter runs the match; Esc or an arrow key keeps it in the line to edit it; Ctrl+G or Ctrl+C cancels.

**Hints.** While typing, the rest of the newest matching history entry is shown dim after the line: → / End / Ctrl+F accepts it, Alt+F accepts one word, Esc hides it.

**Completion.** Tab completes the command name (`st` → `start `). When several commands match, Tab inserts their common part, then a second Tab opens a menu below the prompt (the viewport grows to show it): Tab/↓ and Shift+Tab/↑ select, ←/→ change column, Enter keeps the selection without running the line, typing goes on after it, Esc restores the line. When there is nothing left to complete (for instance after `start `), Tab accepts the whole hint, like →.

**Paste.** The application turns bracketed paste on: a pasted text is inserted on the prompt line in one go (line breaks and tabs become spaces) and nothing runs before Enter. In a terminal without bracketed paste, pasted text arrives as typed keys and each line break runs a line.

Terminal notes:

- on macOS, the Alt shortcuts need "Use Option as Meta key" (Terminal.app, Profiles › Keyboard) or "Option key: Esc+" (iTerm2); otherwise Option+B types `∫`, which is inserted;
- many terminals send Ctrl+Backspace as Ctrl+H, which deletes one character;
- Ctrl+Z is undo, not suspend, and Ctrl+S is free (forward search): raw mode turns off the terminal's `ISIG` and `IXON`.

## Known limits

The limits of the library itself are listed at the end of its [guide](https://github.com/tilefab/tilefab/blob/main/crates/tilefab/GUIDE.md#known-limits).
Those of the demo:

- **Maximum height**: `MAX_TASKS + 1` lines (8); below 8 terminal lines, the viewport is capped to the screen height.
- **A task ending with fewer log lines than freed lines**: a blank line may appear below the viewport (case `d > n`). In the demo, every task end produces a line, so this does not happen.
- **Degraded mode**: no task lines; Ctrl+C there is the standard SIGINT signal (immediate stop).
- **Signals** (`kill` from another process: SIGTERM, SIGINT, SIGHUP): the first one quits through the normal clean-up, with exit code 128 + the signal number (143 for SIGTERM). If the UI is stuck (a terminal that reads nothing), a second signal ends the application at once: raw mode is disabled, the rest of the restoration is tried for half a second.

## License

MIT, see [LICENSE](https://github.com/tilefab/tilefab/blob/main/LICENSE).
