#!/usr/bin/env python3
"""End-to-end verification of `tilefab` in a pty.

    cargo build
    tools/.venv/bin/python tools/check_app.py [--rows 24] [--cols 100] [--prefill 40]

Scenarios:
  * idle: single-line viewport (the prompt), Unicode input, long input;
  * burst: 5 tasks launched at once, typing while tasks finish;
  * resize: resizing while tasks run;
  * zero-width: the pty loses all its columns while a line is pending:
    no panic, and the line is shown once the width is back;
  * zero-height: the same with all the rows: no panic while nothing can be drawn,
    and the line is shown once the rows are back;
  * signals: SIGTERM, SIGINT and SIGHUP exit cleanly, with code 0;
  * panic: the hidden `panic` command leaves a clean terminal;
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
BIN = ROOT / "target" / "debug" / "tilefab"
NAMES = ["alpha", "beta", "gamma", "delta", "epsilon"]
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
        while top > 0 and re.match(r"^(alpha|beta|gamma|delta|epsilon)\s", lines[top - 1]):
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
    check.ok(seen_heights >= {1, 6}, "heights seen %r: expected 1 to 6" % sorted(seen_heights))
    check.ok(max(seen_heights, default=0) <= 6, "viewport taller than MAX_TASKS + 1")
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
        check.ok(code == 0, "exit code %r after %s, expected 0" % (code, name))
        check.ok(terminal_is_clean(term), "terminal left in raw mode after %s" % name)
        raw = bytes(term.raw)
        check.ok(raw.rfind(CURSOR_SHOW) > raw.rfind(CURSOR_HIDE), "cursor left hidden after %s" % name)
        check.ok('task "alpha" started' in term.all_text(), "log lost after %s" % name)
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
    args = parser.parse_args()
    if not BIN.exists():
        print("%s not found: run `cargo build`" % BIN, file=sys.stderr)
        return 2

    total = 0
    scenarios = [
        ("idle", lambda: scenario_rest(args.rows, args.cols, args.prefill)),
        ("burst", lambda: scenario_burst(args.rows, args.cols, args.prefill)),
        ("resize", lambda: scenario_resize(args.rows, args.cols, args.prefill)),
        ("zero-width", lambda: scenario_zero_width(args.rows, args.cols, args.prefill)),
        ("zero-height", lambda: scenario_zero_height(args.rows, args.cols, args.prefill)),
        ("signals", lambda: scenario_signals(args.rows, args.cols, args.prefill)),
        ("panic", lambda: scenario_panic(args.rows, args.cols, args.prefill)),
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
