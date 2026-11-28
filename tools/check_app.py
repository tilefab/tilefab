#!/usr/bin/env python3
"""End-to-end verification of `widget-demo` in a pty.

    cargo build            # or `cargo build --release` with --release
    tools/.venv/bin/python tools/check_app.py [--rows 24] [--cols 100] [--prefill 40] [--release]

With --release, the scenarios run `target/release/widget-demo` instead of `target/debug/widget-demo`:
release-only code paths (no overflow checks, no `debug_assert!`) are checked end to end too.

Scenarios:
  * idle: single-line viewport (the prompt), Unicode input, long input;
  * burst: as many tasks as the demo runs at once (`MAX_TASKS`), launched at once, typing
    while tasks finish;
  * resize: resizing while tasks run;
  * zero-width: the pty loses all its columns while a line is pending:
    no panic, and the line is shown once the width is back;
  * zero-height: the same with all the rows: no panic while nothing can be drawn,
    and the line is shown once the rows are back;
  * line-editor: ↑ history recall, Tab completion and its menu (opened and closed without
    stale rows), Ctrl+R search, a bracketed paste that does not run;
  * signals: SIGTERM, SIGINT and SIGHUP exit cleanly, with code 128 + the signal number;
  * stuck: with the UI loop blocked on a full pty, a second signal ends the application
    at once, raw mode disabled, with the code of that signal;
  * panic: the hidden `panic` command leaves a clean terminal (with --release, where
    the command does not exist: `panic` is reported as unknown and the application goes on);
  * headless: no terminal (stdin or stdout redirected), no UI; input escape
    sequences are echoed escaped.

For the burst scenario, also check emitted bytes: everything is inside an
`ESC[?2026h … ESC[?2026l` block, no `ESC[2J` or `ESC[J`, only one cursor position query,
each `started` / `finished` line appears exactly once, and the prompt stays
on the same screen row (once the screen is full). Exit code: 0 if everything passes.
"""

from __future__ import annotations

import argparse
import fcntl
import os
import re
import signal
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from pty_harness import SYNC_END, Term, outside_sync  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "debug" / "widget-demo"  # target/release/widget-demo with --release (see main)
# The most tasks the demo runs at once, read from its source so that the burst scenario
# follows it.
MAX_TASKS = int(
    re.search(
        r"pub const MAX_TASKS: usize = (\d+);",
        (ROOT / "apps" / "widget-demo" / "src" / "main.rs").read_text(),
    ).group(1)
)
NAMES = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota"][:MAX_TASKS]
# Sequences sought in emitted bytes (outside f-strings: Python 3.9 forbids backslashes).
CPR = b"\x1b[6n"  # cursor position query
SYNC_BEGIN_SEQ = b"\x1b[?2026h"
CURSOR_SHOW = b"\x1b[?25h"
CURSOR_HIDE = b"\x1b[?25l"
ERASE_RE = re.compile(rb"\x1b\[(2)?J")


class Check:
    def __init__(self) -> None:
        self.failures: list[str] = []

    def ok(self, condition: bool, message: str) -> None:
        if not condition:
            self.failures.append(message)
            print("    FAILED: " + message)


def launch(rows: int, cols: int, prefill: int) -> Term:
    """Launch the application after `prefill` text lines (the preceding shell)."""
    script = 'i=0; while [ $i -lt %d ]; do echo "before $i"; i=$((i+1)); done; exec %s' % (prefill, BIN)
    term = Term(["sh", "-c", script], rows=rows, cols=cols)
    if prefill:
        term.wait_for("before", timeout=10.0)
    term.wait_idle(idle=0.5)
    return term


def prompt_row(term: Term):
    rows = [i for i, line in enumerate(term.screen_lines()) if line.startswith(">")]
    return rows[-1] if rows else None


def terminal_is_clean(term: Term) -> bool:
    attrs = termios.tcgetattr(term.fd)[3]
    return bool(attrs & termios.ICANON) and bool(attrs & termios.ECHO)


def quit_app(term: Term, check: Check) -> None:
    term.send("\x15quit\r")  # Ctrl+U clears any current input
    term.run_for(1.0)
    check.ok(not term.alive(), "the application did not stop on `quit`")
    check.ok(terminal_is_clean(term), "terminal left in raw mode")


def scenario_rest(rows: int, cols: int, prefill: int) -> list:
    check = Check()
    term = launch(rows, cols, prefill)
    p = prompt_row(term)
    check.ok(p is not None, "prompt missing at startup")
    if p is not None:
        # Idle: the viewport is ONE line, the prompt; nothing else is drawn below.
        below = term.screen_lines()[p + 1 :]
        check.ok(all(not line for line in below), "lines below the prompt when idle: %r" % below)
        if prefill >= rows:
            check.ok(p == rows - 1, "prompt on row %d, expected %d" % (p, rows - 1))
    # Unicode input.
    term.send("été 日本")
    term.run_for(0.5)
    p = prompt_row(term)
    check.ok(p is not None and "été 日本" in term.screen_lines()[p], "Unicode input not displayed")
    row, _col = term.cursor()
    check.ok(p is not None and row == p, "cursor on row %d, prompt on %s" % (row, p))
    # Input longer than the line: text scrolls, the cursor remains visible.
    term.send("\x15" + "x" * (cols + 20))  # Ctrl+U followed by very long input
    term.run_for(0.6)
    row, col = term.cursor()
    check.ok(col < cols, "cursor outside the screen (%d)" % col)
    line = term.screen_lines()[row]
    check.ok(line.startswith(">") and line.rstrip().endswith("x"), "long input: %r" % line)
    term.send("\x15")
    # Invalid command: a warning, no crash.
    term.send("zzz\r")
    term.wait_for("unknown command", timeout=5.0)
    check.ok("unknown command" in term.all_text(), "no warning for an unknown command")
    quit_app(term, check)
    term.close()
    return check.failures


def scenario_burst(rows: int, cols: int, prefill: int) -> list:
    check = Check()
    term = launch(rows, cols, prefill)
    first = term.raw.find(CPR)
    check.ok(first >= 0, "no cursor position query")
    rest_prompt = prompt_row(term)

    for name in NAMES:
        term.send("start %s\r" % name)
    # A task row starts with its name.
    task_row = re.compile(r"^(%s)\s" % "|".join(NAMES))
    # Continuous observation: at each stable state (between blocks), the structure is consistent.
    deadline = time.monotonic() + 16.0
    seen_heights = set()
    prompt_rows = set()
    typed = False
    while time.monotonic() < deadline and term.alive():
        term.pump(0.02)
        if not term.raw.endswith(SYNC_END):
            continue
        lines = term.screen_lines()
        pr = [i for i, line in enumerate(lines) if line.startswith(">")]
        if not pr:
            check.ok(False, "prompt not found")
            break
        # Command echoes (“> start alpha”) are in the logs: the prompt is the last one.
        p = pr[-1]
        top = p
        while top > 0 and task_row.match(lines[top - 1]):
            top -= 1
        seen_heights.add(p - top + 1)
        prompt_rows.add(p)
        # Immediately above the viewport: always a log or preceding text, never a blank line.
        if top > 0 and not lines[top - 1]:
            check.ok(False, "blank line above the viewport (top at %d)" % top)
            break
        # Type while tasks finish: once a few tasks have completed.
        if not typed and "finished" in term.all_text():
            term.send("help")
            typed = True
    # All tasks have finished: the viewport is back to one line.
    term.run_for(1.0)
    text = term.all_text()
    for name in NAMES:
        check.ok(text.count('task "%s" started' % name) == 1, "\"%s started\": not exactly once" % name)
        check.ok(text.count('task "%s" finished' % name) == 1, "\"%s finished\": not exactly once" % name)
    tallest = MAX_TASKS + 1
    check.ok(seen_heights >= {1, tallest}, "heights seen %r: expected 1 to %d" % (sorted(seen_heights), tallest))
    check.ok(max(seen_heights, default=0) <= tallest, "viewport taller than MAX_TASKS + 1")
    final = prompt_row(term)
    if prefill >= rows:
        check.ok(final == rest_prompt, "prompt moved after the tasks")
        check.ok(prompt_rows == {rows - 1}, "the prompt moved: rows %r" % sorted(prompt_rows))
    # What the user typed during that time is still in the prompt.
    if typed and final is not None:
        check.ok("help" in term.screen_lines()[final], "input lost while tasks were ending")

    # Measurable criteria on emitted bytes.
    raw = bytes(term.raw[first:]) if first >= 0 else bytes(term.raw)
    leftover = outside_sync(raw).replace(CPR, b"")
    check.ok(not leftover, "bytes emitted outside a synchronized block: %r" % leftover[:60])
    check.ok(not ERASE_RE.search(raw), "ESC[2J or ESC[J emitted")
    check.ok(raw.count(CPR) == 1, "%d cursor position queries" % raw.count(CPR))
    print(
        "    heights seen %r, %d blocks, %d bytes emitted"
        % (sorted(seen_heights), raw.count(SYNC_BEGIN_SEQ), len(raw))
    )
    quit_app(term, check)
    term.close()
    return check.failures


def scenario_resize(rows: int, cols: int, prefill: int) -> list:
    check = Check()
    term = launch(rows, cols, prefill)
    for name in NAMES[:3]:
        term.send("start %s\r" % name)
    term.run_for(1.0)
    term.resize(rows, cols // 2)
    term.run_for(1.0)
    check.ok(term.alive(), "the application stopped on resize")
    check.ok(prompt_row(term) is not None, "prompt missing after shrinking")
    term.resize(rows, cols)
    term.run_for(1.0)
    check.ok(prompt_row(term) is not None, "prompt missing after enlarging")
    check.ok(term.alive(), "the application stopped")
    # One query at startup, one per resize (no more).
    queries = bytes(term.raw).count(CPR)
    check.ok(queries <= 3, "%d cursor position queries" % queries)
    quit_app(term, check)
    term.close()
    return check.failures


def scenario_zero_width(rows: int, cols: int, prefill: int) -> list:
    """crossterm reports a width of 0 for a 0-column pty.

    Each round queues a "task started" line with the current width, then sets
    the pty to 0 columns (ioctl only, no SIGWINCH) just before Enter: the draw
    of the same iteration discovers the zero width while the line is pending.
    Without the guard in `draw_inline` / `commit`, this panics; a round can
    miss when a ticker draw sees the zero width first, hence several rounds.
    No line is lost: each "task started" line appears exactly once, once the width
    is back.
    """
    check = Check()
    term = launch(rows, cols, prefill)
    for name in NAMES[:3]:
        term.send("start %s" % name)
        term.wait_idle(0.3)
        fcntl.ioctl(term.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, 0, 0, 0))
        term.send("\r")
        term.run_for(0.5)
        check.ok(term.alive(), "the application stopped at width 0 (task %s)" % name)
        if not term.alive():
            break
        term.resize(rows, cols)
        # Wait for the line instead of a fixed delay: on a loaded machine the redraw can
        # take longer. A line that is really lost still fails, after the timeout.
        term.wait_for('task "%s" started' % name, timeout=10.0)
        term.run_for(0.3)
    if term.alive():
        check.ok(prompt_row(term) is not None, "prompt missing after restoring the width")
        text = term.all_text()
        for name in NAMES[:3]:
            count = text.count('task "%s" started' % name)
            check.ok(count == 1, "task %s started line seen %d times after a zero width" % (name, count))
        quit_app(term, check)
    check.ok("panicked" not in term.all_text(), "panic report after a zero width")
    term.close()
    return check.failures


def scenario_zero_height(rows: int, cols: int, prefill: int) -> list:
    """The pty loses all its rows (ws_row = 0) while a line is pending.

    Symmetric of `scenario_zero_width`: each round queues a "task started" line, then
    sets the pty to 0 rows (ioctl only, no SIGWINCH) just before Enter, and lets several
    ticks draw on the screen with no rows. Nothing can be drawn meanwhile: the application
    must keep running, keep the line, and show it exactly once when the rows are back.
    """
    check = Check()
    term = launch(rows, cols, prefill)
    for name in NAMES[:3]:
        term.send("start %s" % name)
        term.wait_idle(0.3)
        fcntl.ioctl(term.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 0, cols, 0, 0))
        term.send("\r")
        term.run_for(0.5)
        check.ok(term.alive(), "the application stopped at height 0 (task %s)" % name)
        if not term.alive():
            break
        term.resize(rows, cols)
        term.wait_for('task "%s" started' % name, timeout=10.0)
        term.run_for(0.3)
    if term.alive():
        check.ok(prompt_row(term) is not None, "prompt missing after restoring the rows")
        text = term.all_text()
        for name in NAMES[:3]:
            count = text.count('task "%s" started' % name)
            check.ok(count == 1, "task %s started line seen %d times after a zero height" % (name, count))
        quit_app(term, check)
    check.ok("panicked" not in term.all_text(), "panic report after a zero height")
    term.close()
    return check.failures


def last_row(term: Term):
    """Index and text of the last non-empty screen row (the prompt line, or the bottom of
    the completion menu), or (None, "")."""
    lines = term.screen_lines()
    for i in range(len(lines) - 1, -1, -1):
        if lines[i].strip():
            return i, lines[i]
    return None, ""


def scenario_line_editor(rows: int, cols: int, prefill: int) -> list:
    """The line editor in the application: ↑ recalls the history, Tab completes and opens
    a menu below the prompt (the viewport grows, and shrinks back without stale rows),
    Ctrl+R searches the history, and a bracketed paste is inserted without running."""
    check = Check()
    term = launch(rows, cols, prefill)
    for line in ("list", "help"):
        term.send(line + "\r")
        term.wait_for("> " + line, timeout=5.0)
        term.wait_idle(0.3)
    # ↑ / ↑: the newest entries, newest first.
    term.send("\x1b[A")
    term.wait_idle(0.3)
    check.ok(last_row(term)[1].rstrip() == "> help", "↑ did not recall help: %r" % last_row(term)[1])
    term.send("\x1b[A")
    term.wait_idle(0.3)
    check.ok(last_row(term)[1].rstrip() == "> list", "↑↑ did not recall list: %r" % last_row(term)[1])
    # Tab with one candidate: the command and its space.
    term.send("\x15st\t")
    term.wait_idle(0.3)
    check.ok(last_row(term)[1].rstrip() == "> start", "Tab did not complete start: %r" % last_row(term)[1])
    check.ok(term.cursor()[1] == len("> start "), "no space after start: cursor %r" % (term.cursor(),))
    # Tab on an empty line: the menu of the five commands opens below the prompt.
    term.send("\x15\t")
    term.wait_idle(0.5)
    lines = term.screen_lines()
    p = prompt_row(term)
    menu = [l for l in lines[p + 1 :] if l] if p is not None else []
    check.ok(p is not None and lines[p].rstrip() == "> start", "menu: prompt %r" % (lines[p] if p is not None else None))
    check.ok(len(menu) == 5 and menu[0].startswith("start") and "start a task" in menu[0], "menu rows: %r" % menu)
    check.ok(term.cursor()[0] == p, "cursor not on the prompt while the menu is open")
    # Esc closes it. The viewport shrinks in place (placement rule of the inline viewport:
    # the prompt keeps its row, the freed rows below it are cleared): no menu row is left.
    term.send("\x1b")
    term.wait_idle(0.5)
    p = prompt_row(term)
    check.ok(p is not None and all(not l for l in term.screen_lines()[p + 1 :]), "rows left below the prompt after Esc")
    check.ok("start a task" not in "\n".join(term.screen_lines()), "stale menu rows after Esc")
    # Ctrl+R: the search prompt and the newest match; Enter runs it.
    term.send("\x15\x12li")
    term.wait_idle(0.3)
    check.ok(last_row(term)[1].startswith("(search) li: list"), "Ctrl+R: %r" % last_row(term)[1])
    before = term.all_text().count("no task running")
    term.send("\r")
    term.wait_idle(0.5)
    check.ok(term.all_text().count("no task running") == before + 1, "the search match did not run")
    # Bracketed paste: one line, nothing runs before Enter.
    term.send("\x1b[200~start pasted\nhelp\x1b[201~")
    term.wait_idle(0.5)
    check.ok(last_row(term)[1].rstrip() == "> start pasted help", "paste: %r" % last_row(term)[1])
    check.ok('task "pasted' not in term.all_text(), "the pasted text ran")
    quit_app(term, check)
    term.close()
    return check.failures


def scenario_signals(rows: int, cols: int, prefill: int) -> list:
    """A signal from another process goes through the normal exit path."""
    check = Check()
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        term = launch(rows, cols, prefill)
        term.send("start alpha\r")
        term.run_for(0.5)
        os.kill(term.pid, sig)
        term.run_for(1.0)
        name = sig.name
        code = term.wait_exit(3.0)
        check.ok(code is not None, "the application is still alive after %s" % name)
        expected = 128 + sig.value
        check.ok(code == expected, "exit code %r after %s, expected %d" % (code, name, expected))
        check.ok(terminal_is_clean(term), "terminal left in raw mode after %s" % name)
        raw = bytes(term.raw)
        check.ok(raw.rfind(CURSOR_SHOW) > raw.rfind(CURSOR_HIDE), "cursor left hidden after %s" % name)
        check.ok('task "alpha" started' in term.all_text(), "log lost after %s" % name)
        term.close()
    return check.failures


def scenario_stuck(rows: int, cols: int, prefill: int) -> list:
    """A second signal ends the application even when its UI loop is stuck.

    Nothing reads the pty: the running tasks fill its buffer, the next write of the UI
    loop blocks, and the first signal (SIGTERM) cannot be handled. The second one (SIGINT)
    ends the process at once, raw mode disabled, with its own code: 130, where the cleanup
    of the first one would give 143 once the pty is read again. (The process cannot finish
    exiting before then: closing a pty waits for its unread output.)
    """
    check = Check()
    term = launch(rows, cols, prefill)
    term.send("start a\rstart b\rstart c\rstart d\rstart e\r")
    time.sleep(3.0)  # no pump: the pty buffer fills
    os.kill(term.pid, signal.SIGTERM)
    time.sleep(1.0)
    check.ok(term.alive(), "inconclusive: the UI loop was not stuck (the first SIGTERM quit)")
    os.kill(term.pid, signal.SIGINT)
    time.sleep(1.5)  # still no pump: reading would unblock the loop
    code = term.wait_exit(3.0)
    check.ok(code is not None, "the application is still alive after a second signal")
    expected = 128 + signal.SIGINT.value
    check.ok(code == expected, "exit code %r after a second signal, expected %d" % (code, expected))
    check.ok(terminal_is_clean(term), "terminal left in raw mode after a second signal")
    term.close()
    return check.failures


def scenario_panic(rows: int, cols: int, prefill: int) -> list:
    check = Check()
    term = launch(rows, cols, prefill)
    term.send("start alpha\r")
    term.run_for(0.5)
    term.send("panic\r")
    term.run_for(1.5)
    check.ok(not term.alive(), "the application is still alive after `panic`")
    check.ok(terminal_is_clean(term), "terminal left in raw mode after a panic")
    check.ok("deliberate panic" in term.all_text(), "panic report missing")
    raw = bytes(term.raw)
    check.ok(raw.rfind(CURSOR_SHOW) > raw.rfind(CURSOR_HIDE), "cursor left hidden after a panic")
    term.close()
    return check.failures


def scenario_panic_absent(rows: int, cols: int, prefill: int) -> list:
    """Release build: `panic` is an unknown command, the application keeps running."""
    check = Check()
    term = launch(rows, cols, prefill)
    term.send("panic\r")
    term.run_for(1.0)
    check.ok(term.alive(), "the release application stopped after `panic`")
    check.ok('unknown command: "panic"' in term.all_text(), "no unknown command warning for `panic`")
    term.send("quit\r")
    term.run_for(1.0)
    check.ok(not term.alive(), "the application is still alive after `quit`")
    check.ok(terminal_is_clean(term), "terminal left in raw mode after `quit`")
    term.close()
    return check.failures


def scenario_degraded() -> list:
    check = Check()
    out = subprocess.run(
        [str(BIN)], input="help\nquit\n", capture_output=True, text=True, timeout=20
    )
    check.ok("\x1b[" not in out.stdout + out.stderr, "escape sequences in degraded mode")
    check.ok("commands" in out.stderr, "no help on stderr: %r" % out.stderr)
    check.ok(out.stdout == "", "stdout not empty in degraded mode: %r" % out.stdout)
    # Task messages bypass tracing: check they are printed too (a task lasts 10 s at most).
    out = subprocess.run(
        [str(BIN)], input="start a\n", capture_output=True, text=True, timeout=20
    )
    for word in ("started", "finished"):
        count = out.stderr.count('task "a" %s' % word)
        check.ok(count == 1, "task %s message seen %d times: %r" % (word, count, out.stderr))
    # An unknown command is echoed in a log; escape sequences in the
    # input must reach stderr escaped, never raw.
    out = subprocess.run(
        [str(BIN)], input="x\x1b[31mRED\x07\nquit\n", capture_output=True, text=True, timeout=20
    )
    check.ok("RED" in out.stderr, "unknown command not echoed: %r" % out.stderr)
    check.ok(
        not any(ch in out.stdout + out.stderr for ch in "\x1b\x07"),
        "raw control character echoed in degraded mode: %r" % out.stderr,
    )
    return check.failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    parser.add_argument("--rows", type=int, default=24)
    parser.add_argument("--cols", type=int, default=100)
    parser.add_argument("--prefill", type=int, default=40, help="lines before the app (0 = top of screen)")
    parser.add_argument("--release", action="store_true", help="run the release build (target/release)")
    args = parser.parse_args()
    global BIN
    if args.release:
        BIN = ROOT / "target" / "release" / "widget-demo"
    if not BIN.exists():
        build = "cargo build --release" if args.release else "cargo build"
        print("%s not found: run `%s`" % (BIN, build), file=sys.stderr)
        return 2
    print("binary: %s" % BIN)

    total = 0
    scenarios = [
        ("idle", lambda: scenario_rest(args.rows, args.cols, args.prefill)),
        ("burst", lambda: scenario_burst(args.rows, args.cols, args.prefill)),
        ("resize", lambda: scenario_resize(args.rows, args.cols, args.prefill)),
        ("zero-width", lambda: scenario_zero_width(args.rows, args.cols, args.prefill)),
        ("zero-height", lambda: scenario_zero_height(args.rows, args.cols, args.prefill)),
        ("line-editor", lambda: scenario_line_editor(args.rows, args.cols, args.prefill)),
        ("signals", lambda: scenario_signals(args.rows, args.cols, args.prefill)),
        ("stuck", lambda: scenario_stuck(args.rows, args.cols, args.prefill)),
        ("panic", lambda: (scenario_panic_absent if args.release else scenario_panic)(args.rows, args.cols, args.prefill)),
        ("degraded", scenario_degraded),
    ]
    for name, fn in scenarios:
        print("  %s (%dx%d, prefill=%d)" % (name, args.rows, args.cols, args.prefill))
        failures = fn()
        total += len(failures)
        print("    OK" if not failures else "    %d failure(s)" % len(failures))
    return 1 if total else 0


if __name__ == "__main__":
    sys.exit(main())
