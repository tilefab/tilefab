#!/usr/bin/env python3
"""Measurable flicker-free criterion for the inline viewport.

Launch the `inline` example (`crates/tilefab/examples/inline.rs`) in a pty, let it complete a 1 → 5 → 1 height cycle with concurrent
logs, and check the emitted bytes and emulated screen:

  * all output (after startup) is inside an `ESC[?2026h … ESC[?2026l` block;
  * no `ESC[2J` or `ESC[J` is emitted;
  * each log line appears exactly once, in order (nothing lost or duplicated);
  * after each block, the line just above the viewport is a log (never a blank line);
  * once the screen is full, the prompt stays on the same screen row;
  * on exit, the terminal is clean (line mode, visible cursor).

Usage:

    cargo build --example inline
    tools/.venv/bin/python tools/check_inline.py [--rows 24] [--prefill 40]

`--prefill N` writes N lines before launching the example (large N = screen bottom, 0 = top).
Exit code: 0 if everything passes, 1 otherwise.
"""

from __future__ import annotations

import argparse
import re
import sys
import termios
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from pty_harness import SYNC_BEGIN, SYNC_END, Term, outside_sync  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
EXAMPLE = ROOT / "target" / "debug" / "examples" / "inline"
CYCLE_SECONDS = 18.0  # 1 → 5 (4 changes), then 5 → 1 (4 changes), one every 2 seconds


def run(rows: int, cols: int, prefill: int) -> list[str]:
    """Return the list of observed issues (empty = everything passes)."""
    problems: list[str] = []
    # The shell writes `prefill` lines, then replaces its process with the example.
    script = f'i=0; while [ $i -lt {prefill} ]; do echo "before $i"; i=$((i+1)); done; exec {EXAMPLE}'
    term = Term(["sh", "-c", script], rows=rows, cols=cols)
    term.wait_for("before", timeout=10.0) if prefill else None
    term.wait_idle(idle=0.5)
    start = term.mark()

    prompt_rows: list[int] = []
    samples = 0
    import time

    deadline = time.monotonic() + CYCLE_SECONDS
    while time.monotonic() < deadline and term.alive():
        term.pump(0.02)
        # Observe the screen only between blocks: a real terminal would not display
        # intermediate states inside a block either.
        if not term.raw.endswith(SYNC_END):
            continue
        lines = term.screen_lines()
        prompt = [i for i, line in enumerate(lines) if line.startswith(">")]
        if len(prompt) != 1:
            problems.append(f"prompt not found or found several times: {prompt}")
            break
        p = prompt[0]
        # Viewport start: move upward while lines contain tasks.
        top = p
        while top > 0 and lines[top - 1].startswith("task"):
            top -= 1
        if top > 0 and not lines[top - 1].startswith(("[INFO]", "before")):
            problems.append(f"unexpected line above the viewport: {lines[top - 1]!r}")
            break
        # Count errors but do not stop: a single blank line is enough to fail.
        prompt_rows.append(p)
        samples += 1

    # The example may take time to start: begin at its first emitted byte. The cursor
    # position query (`ESC[6n`, once at initialization) is a *question* to the
    # terminal, not display output: it legitimately sits outside a synchronized block.
    first = term.raw.find(b"\x1b[6n")
    mid = bytes(term.raw[first:]) if first >= 0 else bytes(term.raw[start:])
    queries = mid.count(b"\x1b[6n")
    if queries != 1:
        problems.append(f"{queries} cursor position queries instead of one")
    # 1. Everything inside a synchronized block.
    leftover = outside_sync(mid).replace(b"\x1b[6n", b"")
    if leftover:
        problems.append(f"bytes emitted outside a synchronized block: {leftover[:80]!r}")
    # 2. No screen clearing or clearing to the bottom.
    if re.search(rb"\x1b\[(2)?J", term.raw):
        problems.append("ESC[2J or ESC[J emitted")
    # 3. Logs: each once, in order.
    logs = [l for l in term.all_lines() if l.startswith("[INFO] log")]
    # “Task n started / finished” lines follow the example's cycle: started 1 through 4,
    # then finished 4 through 1, and so on. The observed sequence must be an exact prefix.
    starts = [l.split("task")[1].strip() for l in term.all_lines() if "started" in l or "finished" in l]
    cycle = [f"{n} started" for n in (1, 2, 3, 4)] + [f"{n} finished" for n in (4, 3, 2, 1)]
    expected_starts = (cycle * (len(starts) // len(cycle) + 1))[: len(starts)]
    # Terminal as short as the viewport (rows <= 5): no room for logs; they are
    # ignored during these periods (limitation documented in terminal.rs). In this case,
    # check only for crashes, ghost lines, and clean exit.
    short = rows <= 5
    if not short and (starts != expected_starts or len(starts) < 8):
        problems.append(f"unexpected started/finished lines: {starts}")
    numbers = [int(l.split()[-1]) for l in logs if l.split()[-1].isdigit()]
    if not short and numbers != list(range(1, len(numbers) + 1)):
        problems.append(f"logs lost, duplicated or out of order: {numbers[:5]}…{numbers[-5:]}")
    if not short and len(numbers) < 40:
        problems.append(f"too few logs ({len(numbers)}): did the example run?")
    # 4. The prompt stays put once at the bottom of the screen.
    bottom = rows - 1
    if prefill >= rows:
        moved = sorted({r for r in prompt_rows if r != bottom})
        if moved:
            problems.append(f"the prompt moved: rows {moved} instead of {bottom}")
    # 5. Clean exit.
    term.send("q")
    term.run_for(0.5)
    attrs = termios.tcgetattr(term.fd)[3]
    if term.alive():
        problems.append("the example did not stop on \"q\"")
    if not attrs & termios.ICANON or not attrs & termios.ECHO:
        problems.append("terminal left in raw mode")
    tail = bytes(term.raw[-80:])
    if b"\x1b[?25h" not in tail and b"\x1b[?25l" in term.raw and term.raw.rfind(b"\x1b[?25h") < term.raw.rfind(b"\x1b[?25l"):
        problems.append("cursor left hidden")
    blocks = len(re.findall(re.escape(SYNC_BEGIN), mid))
    print(f"  {samples} states observed, {blocks_msg(blocks)}, {len(numbers)} logs")
    term.close()
    return problems


def blocks_msg(n: int) -> str:
    return f"{n} synchronized blocks"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    parser.add_argument("--rows", type=int, default=24)
    parser.add_argument("--cols", type=int, default=80)
    parser.add_argument("--prefill", type=int, default=40, help="lines written before (0 = top of screen)")
    args = parser.parse_args()
    if not EXAMPLE.exists():
        print(f"{EXAMPLE} not found: run `cargo build --example inline`", file=sys.stderr)
        return 2
    print(f"inline: {args.rows}x{args.cols}, prefill={args.prefill}")
    problems = run(args.rows, args.cols, args.prefill)
    for p in problems:
        print(f"  FAILED: {p}")
    print("  OK" if not problems else f"  {len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
