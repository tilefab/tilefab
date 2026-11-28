#!/usr/bin/env python3
"""End-to-end check of the one-call setup (`tilefab::init_inline`, `init_fullscreen`,
`DefaultTerminal`) and of `poll_key`, in a pty with a terminal emulator.

Scenarios:

  * `quickstart` (apps/quickstart) quit early with `q`: its log lines stay above, the
    viewport is cleared on exit, the terminal is back in line mode with a visible cursor;
  * `quickstart` run to the end: every log line (0 % to 100 %) appears once, in order;
  * the `panic_report` example, inline then fullscreen: a panic in the render function of the
    frame after a resize ends the program with code 101, and the panic report stays on screen
    (the frame must not write anything before `render`: a clearing written after the report
    would erase it), with the terminal back in line mode.

Usage:

    cargo build -p quickstart && cargo build -p tilefab --example panic_report
    tools/.venv/bin/python tools/check_default_terminal.py [--rows 24] [--prefill 40]

`--prefill N` writes N lines before launching each program (large N = screen bottom).
Exit code: 0 if everything passes, 1 otherwise.
"""

from __future__ import annotations

import argparse
import sys
import termios
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from pty_harness import Term  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
QUICKSTART = ROOT / "target" / "debug" / "quickstart"
PANIC_REPORT = ROOT / "target" / "debug" / "examples" / "panic_report"
REPORT = "deliberate panic in the render function"


def launch(argv: list[str], rows: int, cols: int, prefill: int) -> Term:
    """Start `argv` in a pty after `prefill` lines, the way a shell would leave the screen."""
    filler = "; ".join(f"echo prefill {i}" for i in range(prefill)) or "true"
    command = " ".join(f"'{arg}'" for arg in argv)
    # Without a backtrace: the report fits on the screen, where the check looks for it.
    env = {"RUST_BACKTRACE": "0", "RUST_LIB_BACKTRACE": "0"}
    return Term(["/bin/sh", "-c", f"{filler}; exec {command}"], rows=rows, cols=cols, env=env)


def restored(term: Term) -> list[str]:
    """Problems with the state the program left the terminal in."""
    problems = []
    attrs = termios.tcgetattr(term.fd)[3]
    if not attrs & termios.ICANON or not attrs & termios.ECHO:
        problems.append("terminal left in raw mode")
    raw = bytes(term.raw)
    if b"\x1b[?25l" in raw and raw.rfind(b"\x1b[?25h") < raw.rfind(b"\x1b[?25l"):
        problems.append("cursor left hidden")
    return problems


def quickstart_quit(rows: int, cols: int, prefill: int) -> list[str]:
    term = launch([str(QUICKSTART)], rows, cols, prefill)
    problems = []
    if not term.wait_for("[INFO] 0% downloaded"):
        problems.append("no first log line")
    if not term.wait_for("download "):
        problems.append("no progress bar")
    term.send("q")
    code = term.wait_exit()
    if code != 0:
        problems.append(f"exit code {code} after q (expected 0)")
    lines = term.screen_lines()
    if any(line.startswith("download") for line in lines):
        problems.append(f"the viewport was not cleared on exit: {lines}")
    if "[INFO] 0% downloaded" not in term.all_text():
        problems.append("the log line did not stay above")
    problems += restored(term)
    term.close()
    return problems


def quickstart_to_the_end(rows: int, cols: int, prefill: int) -> list[str]:
    term = launch([str(QUICKSTART)], rows, cols, prefill)
    problems = []
    code = term.wait_exit(timeout=20.0)
    if code != 0:
        problems.append(f"exit code {code} (expected 0)")
    logs = [line.strip() for line in term.all_lines() if line.startswith("[INFO]")]
    expected = [f"[INFO] {percent}% downloaded" for percent in range(0, 101, 10)]
    if logs != expected:
        problems.append(f"log lines lost, duplicated or out of order: {logs}")
    problems += restored(term)
    term.close()
    return problems


def panic_after_resize(rows: int, cols: int, prefill: int, fullscreen: bool) -> list[str]:
    argv = [str(PANIC_REPORT)] + (["--fullscreen"] if fullscreen else [])
    term = launch(argv, rows, cols, prefill)
    problems = []
    if not term.wait_for("waiting for a resize"):
        problems.append("the example did not draw")
    term.resize(rows, cols - 10)
    code = term.wait_exit()
    if code != 101:
        problems.append(f"exit code {code} (expected 101, a panic)")
    term.run_for(0.3)
    screen = "\n".join(term.screen_lines())
    if REPORT not in screen:
        problems.append(f"the panic report is not on screen: {screen!r}")
    if not fullscreen and "[INFO] started" not in term.all_text():
        problems.append("the log line inserted before the panic is lost")
    problems += restored(term)
    term.close()
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    parser.add_argument("--rows", type=int, default=24)
    parser.add_argument("--cols", type=int, default=100)
    parser.add_argument("--prefill", type=int, default=40, help="lines written before (0 = top of screen)")
    args = parser.parse_args()
    for program, build in [(QUICKSTART, "cargo build -p quickstart"),
                           (PANIC_REPORT, "cargo build -p tilefab --example panic_report")]:
        if not program.exists():
            print(f"{program} not found: run `{build}`", file=sys.stderr)
            return 2
    scenarios = [
        ("quickstart, quit with q", lambda: quickstart_quit(args.rows, args.cols, args.prefill)),
        ("quickstart, to the end", lambda: quickstart_to_the_end(args.rows, args.cols, args.prefill)),
        ("panic after a resize, inline",
         lambda: panic_after_resize(args.rows, args.cols, args.prefill, fullscreen=False)),
        ("panic after a resize, fullscreen",
         lambda: panic_after_resize(args.rows, args.cols, args.prefill, fullscreen=True)),
    ]
    failed = 0
    for name, scenario in scenarios:
        print(f"  {name} ({args.rows}x{args.cols}, prefill={args.prefill})")
        problems = scenario()
        for problem in problems:
            print(f"    FAILED: {problem}")
        print("    OK" if not problems else f"    {len(problems)} problem(s)")
        failed += bool(problems)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
