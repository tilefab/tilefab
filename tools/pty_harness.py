#!/usr/bin/env python3
"""Pty harness + pyte terminal emulator for end-to-end testing of a TUI application.

Setup (once, from the repository root):

    python3 -m venv tools/.venv
    tools/.venv/bin/pip install --require-hashes --only-binary=:all: -r tools/requirements-pty.txt
    tools/.venv/bin/pip install -r tools/requirements.txt   # the video analysis only

Library usage (see tools/scenarios_pty.py):

    from pty_harness import Term
    t = Term(["target/debug/widget-demo"], rows=30, cols=100)
    t.wait_idle()
    t.send("start alpha\\r")
    t.wait_for("started")
    print("\\n".join(t.screen_lines()))
    t.close()

Command-line usage (quick demo):

    tools/.venv/bin/python tools/pty_harness.py [--rows 30] [--cols 100] -- CMD [ARGS…]

The emulator is pyte, with two additions:
  * `CSI n S` / `CSI n T` (scroll the active region up / down);
  * scrollback: lines leaving the top of the screen (including a scroll region
    starting at row 0) are retained in `Term.scrollback`.

Known limitations: pyte emulates neither line reflow on resize nor the actual
width of emoji (hence the checks in real terminals).
"""

from __future__ import annotations

import argparse
import fcntl
import os
import pty
import re
import select
import signal
import struct
import sys
import termios
import time

import pyte
from pyte.screens import Margins

# Synchronized update sequences (private mode 2026).
SYNC_BEGIN = b"\x1b[?2026h"
SYNC_END = b"\x1b[?2026l"


class _Screen(pyte.Screen):
    """Pyte screen + scrollback + CSI S/T + replies to terminal queries."""

    def __init__(self, columns: int, lines: int, reply):
        super().__init__(columns, lines)
        self.scrollback: list[str] = []
        self._reply = reply

    # pyte calls this method to reply to `ESC[6n` (cursor position).
    def write_process_input(self, data: str) -> None:
        self._reply(data.encode())

    def _region(self) -> Margins:
        return self.margins or Margins(0, self.lines - 1)

    def index(self) -> None:
        # A line feed at the region bottom pushes the top line into scrollback.
        top, bottom = self._region()
        if self.cursor.y == bottom and top == 0:
            self.scrollback.append(self.display[0].rstrip())
        super().index()

    def scroll_up(self, count: int = 0) -> None:
        """CSI n S: move the region's content up by n rows."""
        top, bottom = self._region()
        for _ in range(count or 1):
            if top == 0:
                self.scrollback.append(self.display[0].rstrip())
            for y in range(top, bottom):
                self.buffer[y] = self.buffer[y + 1]
            self.buffer.pop(bottom, None)
        self.dirty.update(range(self.lines))

    def scroll_down(self, count: int = 0) -> None:
        """CSI n T: move the region's content down by n rows."""
        top, bottom = self._region()
        for _ in range(count or 1):
            for y in range(bottom, top, -1):
                self.buffer[y] = self.buffer[y - 1]
            self.buffer.pop(top, None)
        self.dirty.update(range(self.lines))


class _Stream(pyte.ByteStream):
    csi = {**pyte.ByteStream.csi, "S": "scroll_up", "T": "scroll_down"}


class Term:
    """A program launched in a pty, observed by a terminal emulator."""

    def __init__(self, argv, rows: int = 30, cols: int = 100, env: dict | None = None):
        self.rows, self.cols = rows, cols
        self.raw = bytearray()  # all bytes emitted by the program
        self._exited = False
        self.exit_code: int | None = None  # set once the program has exited (see `alive`)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:  # child
            # TILEFAB_HISTORY="" keeps the checks away from the user's history file
            # (a scenario can still pass its own path in `env`).
            child_env = {**os.environ, "TERM": "xterm-256color", "TILEFAB_HISTORY": "", **(env or {})}
            os.execvpe(argv[0], list(argv), child_env)
        self._set_winsize()
        self.screen = _Screen(cols, rows, self._write)
        self.stream = _Stream(self.screen, strict=False)

    # --- input / output ------------------------------------------------------

    def _write(self, data: bytes) -> None:
        try:
            os.write(self.fd, data)
        except OSError:
            pass

    def send(self, text: str | bytes) -> None:
        """Send bytes to the program (`\\r` = Enter, `\\x03` = Ctrl+C…)."""
        self._write(text.encode() if isinstance(text, str) else text)

    def pump(self, timeout: float = 0.05) -> bytes:
        """Read available data, send it to pyte, and return the bytes read."""
        got = b""
        while select.select([self.fd], [], [], timeout)[0]:
            try:
                data = os.read(self.fd, 65536)
            except OSError:  # EIO: the child closed the pty
                self._exited = True
                break
            if not data:
                self._exited = True
                break
            got += data
            timeout = 0  # then drain without waiting
        if got:
            self.raw += got
            self.stream.feed(got)
        return got

    def wait_idle(self, idle: float = 0.3, timeout: float = 10.0) -> None:
        """Wait until the program has emitted nothing for `idle` seconds."""
        deadline = time.monotonic() + timeout
        last = time.monotonic()
        while time.monotonic() < deadline:
            if self.pump(0.05):
                last = time.monotonic()
            elif time.monotonic() - last >= idle or self._exited:
                return

    def wait_for(self, text: str, timeout: float = 10.0) -> bool:
        """Wait for `text` to appear on screen or in scrollback."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.pump(0.05)
            if text in self.all_text():
                return True
            if self._exited:
                break
        return False

    def run_for(self, seconds: float) -> None:
        """Let the program run for `seconds` seconds."""
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.pump(0.02)

    def mark(self) -> int:
        """Current position in `raw` (to isolate the bytes of a step)."""
        return len(self.raw)

    # --- observation ---------------------------------------------------------

    def screen_lines(self) -> list[str]:
        return [line.rstrip() for line in self.screen.display]

    @property
    def scrollback(self) -> list[str]:
        return self.screen.scrollback

    def all_lines(self) -> list[str]:
        """Scrollback + screen, without trailing blank lines."""
        lines = self.scrollback + self.screen_lines()
        while lines and not lines[-1]:
            lines.pop()
        return lines

    def all_text(self) -> str:
        return "\n".join(self.all_lines())

    def cursor(self) -> tuple[int, int]:
        """Cursor (row, column), zero-based."""
        return self.screen.cursor.y, self.screen.cursor.x

    # --- window / exit -------------------------------------------------------

    def _set_winsize(self) -> None:
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", self.rows, self.cols, 0, 0))

    def resize(self, rows: int, cols: int) -> None:
        """Change the window size (TIOCSWINSZ + SIGWINCH, like a real terminal)."""
        self.rows, self.cols = rows, cols
        self._set_winsize()
        self.screen.resize(rows, cols)
        os.kill(self.pid, signal.SIGWINCH)

    def _reap(self) -> None:
        """Collect the exit status if the program has exited (non-blocking)."""
        if self.exit_code is not None:
            return
        try:
            done, status = os.waitpid(self.pid, os.WNOHANG)
        except ChildProcessError:
            return
        if done:
            self._exited = True
            # Exit code, or -N if killed by signal N.
            self.exit_code = os.waitstatus_to_exitcode(status)

    def alive(self) -> bool:
        self._reap()
        return not self._exited

    def wait_exit(self, timeout: float = 5.0) -> int | None:
        """Wait for the program to exit; return its exit code (None on timeout).

        `pump` sees the pty close (EIO) slightly before the process can be reaped:
        poll until the status is available.
        """
        deadline = time.monotonic() + timeout
        while self.exit_code is None and time.monotonic() < deadline:
            self.pump(0.05)
            self._reap()
        return self.exit_code

    def close(self) -> None:
        try:
            os.kill(self.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            os.waitpid(self.pid, 0)
        except ChildProcessError:
            pass
        os.close(self.fd)


def sync_blocks(raw: bytes) -> list[bytes]:
    """Split emitted bytes into `ESC[?2026h … ESC[?2026l` blocks (without delimiters)."""
    pattern = re.escape(SYNC_BEGIN) + b"(.*?)" + re.escape(SYNC_END)
    return re.findall(pattern, raw, flags=re.S)


def outside_sync(raw: bytes) -> bytes:
    """Bytes emitted outside any synchronized update block."""
    pattern = re.escape(SYNC_BEGIN) + b".*?" + re.escape(SYNC_END)
    return re.sub(pattern, b"", raw, flags=re.S)


def main() -> int:
    parser = argparse.ArgumentParser(description="Run CMD in a pty and display the emulated screen.")
    parser.add_argument("--rows", type=int, default=30)
    parser.add_argument("--cols", type=int, default=100)
    parser.add_argument("--seconds", type=float, default=2.0, help="observation time")
    parser.add_argument("cmd", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    cmd = args.cmd[1:] if args.cmd[:1] == ["--"] else args.cmd
    if not cmd:
        parser.error("missing command (… -- CMD [ARGS…])")
    term = Term(cmd, rows=args.rows, cols=args.cols)
    try:
        term.run_for(args.seconds)
        print("\n".join(term.all_lines()))
        print(f"-- cursor {term.cursor()}, {len(term.raw)} bytes emitted")
    finally:
        term.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
