#!/usr/bin/env python3
"""Frame-by-frame analysis of a tilefab recording (made by tools/record_check.sh).

Setup: see tools/pty_harness.py (same venv: tools/.venv).
System prerequisite: `ffmpeg` (Homebrew: `brew install ffmpeg`).

    tools/.venv/bin/python tools/frames_check.py VIDEO [--log LOG] [options]

Steps:
  1. calibrate on a sample of frames: background color, text area (excluding
     window chrome), and text line height;
  2. read ALL frames (ffmpeg, 60 fps); split each frame into text
     lines, classified as empty / nonempty;
  3. check invariants:
       gap         blank line between the last log and the viewport top;
       prompt      prompt line changes without a keystroke at that time (log);
       rewrite     an existing log line changes without scrolling as a block;
       transient   intermediate state lasting 1 to 2 frames, reverting to the previous state;
  4. Markdown report + a PNG panel (before / anomaly / after) per anomaly,
     in a `<video>.report/` directory.

Exit code: 0 if no anomalies, 1 otherwise, 2 for usage errors.

Useful options:
  --viewport dynamic      viewport height follows tasks (default);
  --viewport fixed:6      fixed-height viewport (an application that never resizes it):
                          blank lines INSIDE the viewport are not defects;
  --max-viewport 6        maximum viewport height (area where lines may
                          change without scrolling);
  --crop x0,y0,x1,y1      text area in pixels, if automatic detection fails
                          (tuned for Terminal.app / iTerm2 captures on macOS;
                          elsewhere, pass --crop and --pitch);
  --pitch PX --origin PX  line height and first line position (same applies).
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
from collections import deque
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

# Grayscale difference threshold for a pixel to count as changed (the codec adds noise).
PIXEL_THRESHOLD = 80
# Number of differing pixels required for a text line to count as modified.
ROW_CHANGED = 6
# A line with fewer ink pixels than this is considered empty.
ROW_EMPTY = 3
# Allowed delay (seconds) between a logged keystroke and a prompt change.
KEY_WINDOW = 0.4
# Radius (px) of the window's rounded bottom corners (macOS), ignored by automatic detection.
CORNER = 40
# Margin (seconds) before `stop` + offset, covering uncertainty in log/video alignment.
STOP_MARGIN = 0.1
# Minimum line height (px) accepted by automatic estimation.
MIN_PITCH = 16


# --------------------------------------------------------------------------- ffmpeg


def ffmpeg_exe() -> str:
    # A shell launched by a tool or script may have a reduced PATH without Homebrew's
    # directory: also look in the usual locations.
    found = shutil.which("ffmpeg", path=os.environ.get("PATH", "") + ":/opt/homebrew/bin:/usr/local/bin")
    if not found:
        sys.exit("ffmpeg not found (PATH, /opt/homebrew/bin, /usr/local/bin): `brew install ffmpeg`")
    return found


def probe_size(ff: str, video: str) -> tuple[int, int]:
    out = subprocess.run([ff, "-hide_banner", "-i", video], capture_output=True, text=True).stderr
    m = re.search(r"Video:.*?, (\d{2,5})x(\d{2,5})[ ,\[]", out)
    if not m:
        sys.exit(f"cannot read the video size:\n{out}")
    return int(m.group(1)), int(m.group(2))


def read_frames(ff, video, w, h, fps, pix="gray", step=None, only=None):
    """Iterate over frames (numpy arrays). `step`: every Nth frame; `only`: list of indices."""
    vf = f"fps={fps}"
    if step:
        vf += f",select=not(mod(n\\,{step}))"
    if only is not None:
        vf += ",select=" + "+".join(f"eq(n\\,{i})" for i in sorted(only))
    cmd = [ff, "-v", "error", "-i", video, "-vf", vf, "-fps_mode", "passthrough",
           "-f", "rawvideo", "-pix_fmt", pix, "-"]
    channels = 1 if pix == "gray" else 3
    size = w * h * channels
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE)
    try:
        while True:
            buf = proc.stdout.read(size)
            if len(buf) < size:
                break
            arr = np.frombuffer(buf, np.uint8)
            yield arr.reshape(h, w) if channels == 1 else arr.reshape(h, w, 3)
    finally:
        proc.kill()
        proc.wait()


# ---------------------------------------------------------------------- calibration


def longest_span(mask: np.ndarray) -> tuple[int, int]:
    """Longest consecutive run of True values: (start, exclusive end)."""
    best = (0, 0)
    start = None
    for i, v in enumerate(list(mask) + [False]):
        if v and start is None:
            start = i
        elif not v and start is not None:
            if i - start > best[1] - best[0]:
                best = (start, i)
            start = None
    return best


def estimate_pitch(prof: np.ndarray) -> tuple[float, float]:
    """Line height (px, possibly fractional) and first-line offset.

    `prof[y]` = ink on pixel row y, accumulated across the sample. Text lines
    repeat regularly: find the period by autocorrelation, refine it,
    then place line boundaries where there is the least ink (between lines)."""
    # Work on the profile's derivative: it keeps only text line edges
    # (large solid areas, such as gauge bars, no longer dominate the rest).
    p = np.diff(prof.astype(np.float64))
    p -= p.mean()
    n = len(p)
    # Normalize by overlap (n - offset): otherwise larger offsets are penalized.
    ac = np.correlate(p, p, mode="full")[n - 1:] / (n - np.arange(n))
    ac /= ac[0]
    # A text line is at least MIN_PITCH pixels tall: below that, autocorrelation
    # peaks come from within glyphs (stems, bars, gauge blocks).
    lo, hi = MIN_PITCH, min(120, n // 2)

    def comb(lag: int) -> float:
        """Mean autocorrelation at lag, 2·lag, 3·lag (±1 px): the true line height
        repeats at all its multiples, whereas a glyph-induced peak does not."""
        vals = [ac[k * lag - 1:k * lag + 2].max() for k in (1, 2, 3) if k * lag + 2 <= n]
        return float(np.mean(vals))

    scores = {lag: comb(lag) for lag in range(lo, hi)}
    # A multiple of the true height (2 lines) scores almost as well: take the
    # smallest lag reaching at least 95% of the best score.
    top = max(scores.values())
    lag = min(l for l, s in scores.items() if s >= 0.95 * top)
    # Every other (or third) line may resemble its neighbor and make a multiple win: if
    # half (or a third) of the chosen pitch scores similarly, that is the true line height.
    for k in (3, 2):
        sub = round(lag / k)
        if sub >= lo and abs(sub * k - lag) <= 1 and scores.get(sub, 0.0) >= 0.8 * top:
            lag = sub
            break
    ys = np.arange(len(prof))
    best_pitch, best_score, best_folded = float(lag), -1.0, None
    for pf in np.arange(lag - 1.0, lag + 1.0, 0.02):
        bins = ((ys % pf) / pf * 16).astype(int) % 16
        folded = np.bincount(bins, weights=prof, minlength=16)
        if folded.var() > best_score:
            best_pitch, best_score, best_folded = float(pf), folded.var(), folded
    # Boundary between two lines = midpoint of the longest ink-free run (line gap),
    # accurate to the pixel: `argmin` over 16 bins can miss by up to 2 px, causing the cursor
    # (which fills the entire cell) to spill into the neighboring line's band.
    nb = max(int(round(best_pitch)), 4)
    bins = ((ys % best_pitch) / best_pitch * nb).astype(int) % nb
    folded = np.bincount(bins, weights=prof, minlength=nb)
    low = folded <= 0.02 * folded.max()
    best_run, best_start = 0, 0
    for s in range(nb):
        if low[s] and not low[s - 1]:  # start of an empty run (circular)
            run = next((k for k in range(nb) if not low[(s + k) % nb]), nb)
            if run > best_run:
                best_run, best_start = run, s
    if best_run == 0:  # no empty run: fall back to the minimum
        origin = (int(np.argmin(folded)) + 0.5) / nb * best_pitch
    else:
        origin = ((best_start + best_run / 2) % nb) / nb * best_pitch
    return best_pitch, origin


class Layout:
    """Split a frame into text lines."""

    def __init__(self, x0, y0, x1, y1, pitch, origin, bg, static=None):
        self.x0, self.x1, self.y1, self.bg, self.pitch = x0, x1, y1, bg, pitch
        self.y0 = y0
        # Integer pitch (terminal lines have integer pixel heights): the origin must
        # also be an integer; otherwise `round` rounds some bands up and others down,
        # and a 1 px shift samples a different slice of glyphs (false positive).
        if pitch == int(pitch):
            origin = float(round(origin))
        count = int((y1 - y0 - origin) // pitch)
        starts = [int(round(y0 + origin + k * pitch)) for k in range(count)]
        # Margins at the top and bottom of each band:
        #  - bottom: the cursor and gauge blocks fill the whole cell and spill
        #    1-2 px into the neighboring line (antialiasing, rounding);
        #  - top, wider: descenders (g, q, p, y…) from the line above extend
        #    into the next band. The first line of the area has nothing above it, so the same
        #    text line would differ depending on whether it is at the top or lower down
        #    (false “rewrite” when everything scrolls).
        top = max(int(round(pitch * 0.2)), 3)
        bottom = max(int(round(pitch * 0.1)), 2)
        self.bh = max(int(pitch) - top - bottom, 1)
        self.rows_idx = np.array([s + top + np.arange(self.bh) for s in starts])  # (n, bh)
        self.n = count
        # Pixels to ignore (rounded window corners): replace them with the background color
        # in the bands, or a corner could make the last line appear nonempty.
        self.static = None if static is None else static[:, x0:x1][self.rows_idx]  # (n, bh, W)

    def bands(self, gray: np.ndarray) -> np.ndarray:
        bands = gray[:, self.x0:self.x1][self.rows_idx].astype(np.int16)  # (n, bh, W)
        if self.static is not None:
            bands[self.static] = self.bg
        return bands


def calibrate(ff, video, w, h, fps, crop, pitch, origin):
    hist = np.zeros(256, np.int64)
    acc = np.zeros((h, w), np.float32)
    lo = np.full((h, w), 255, np.uint8)  # minimum / maximum value of each pixel
    hi = np.zeros((h, w), np.uint8)      # across the sample: used to detect what MOVES
    count = 0
    head: list[np.ndarray] = []
    bg = None

    def add(g):
        nonlocal count
        mask = np.abs(g.astype(np.int16) - bg) > PIXEL_THRESHOLD
        # Frame almost entirely different from the background: window closing or fading
        # (iTerm2 at scenario end). This is not text; do not sample it.
        if count >= 3 and mask.mean() > 0.35:
            return
        acc.__iadd__(mask.astype(np.float32))
        np.minimum(lo, g, out=lo)
        np.maximum(hi, g, out=hi)
        count += 1

    for i, g in enumerate(read_frames(ff, video, w, h, fps, step=6)):
        if bg is None:
            head.append(g.copy())
            hist += np.bincount(g.ravel(), minlength=256)
            if len(head) == 3:
                bg = int(hist.argmax())
                for earlier in head:
                    add(earlier)
        else:
            add(g)
    if bg is None:  # fewer than 3 sampled frames
        if not head:
            sys.exit("empty video")
        bg = int(hist.argmax())
        for earlier in head:
            add(earlier)
    ink = acc / count
    # Pixels changing during the video: text (logs, bars), not the window.
    changed = (hi.astype(np.int16) - lo.astype(np.int16)) > PIXEL_THRESHOLD

    if crop:
        x0, y0, x1, y1 = crop
    else:
        # A pixel row belongs to the text area if it matches the background
        # (first frame) or changes at some point. The title bar, its separator, and the
        # bottom border do neither: they are constant and differ from the background.
        like_bg = np.abs(np.median(head[0], axis=1).astype(np.int16) - bg) <= 8
        rows_ok = like_bg | (changed.sum(axis=1) >= 3)
        y0, y1 = longest_span(rows_ok)
        cols_ok = ink[y0:y1].mean(axis=0) < 0.9   # columns excluding borders and scrollbar
        x0, x1 = longest_span(cols_ok)
    if y1 - y0 < 40 or x1 - x0 < 40:
        sys.exit(f"text area not found ({x0},{y0})-({x1},{y1}): use --crop")
    if pitch is None:
        # Ink profile of the text area (the window title is already excluded).
        prof = ink[y0:y1, x0:x1].sum(axis=1)
        pitch, gap = estimate_pitch(prof)
        # Text lines have integer heights (screen pixels): round if close to an integer.
        if abs(pitch - round(pitch)) < 0.2:
            pitch = float(round(pitch))
        if origin is None:
            # `gap` = a line gap; move back to the one preceding the first displayed line.
            first = int(np.flatnonzero(prof >= 3)[0]) if (prof >= 3).any() else 0
            origin = gap + pitch * np.floor((first - gap) / pitch)
    elif origin is None:
        origin = 0.0
    static = np.zeros((h, w), bool)
    if not crop:
        # Rounded lower window corners: they show what is BEHIND the window,
        # which changes between frames. Mask only the part OUTSIDE the arc
        # (text remains visible, even on the last line).
        r = CORNER
        lim = (r - 3) ** 2                   # 3 px margin: the arc edge is antialiased
        yy, xx = np.mgrid[0:r, 0:r]          # coordinates within the corner's r×r square
        static[h - r:, :r] |= (xx - r) ** 2 + yy ** 2 > lim     # bottom-left corner: arc center at (r, 0)
        static[h - r:, w - r:] |= xx ** 2 + yy ** 2 > lim       # bottom-right corner: arc center at (0, 0)
    return Layout(x0, y0, x1, y1, pitch, origin, bg, static=static)


# ------------------------------------------------------------------------- analysis


def rdiff(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    """Number of differing pixels per text line."""
    return (np.abs(a - b) > PIXEL_THRESHOLD).sum(axis=(1, 2))


def parse_log(path: str | None) -> list[tuple[float, str]]:
    events = []
    if path:
        for line in Path(path).read_text().splitlines():
            m = re.match(r"t=(-?[\d.]+)\s+(\S+)", line)
            if m:
                events.append((float(m.group(1)), m.group(2)))
    return events


def analyze(video, log, fps, viewport, max_vp, crop, pitch, origin, offset):
    ff = ffmpeg_exe()
    w, h = probe_size(ff, video)
    lay = calibrate(ff, video, w, h, fps, crop, pitch, origin)
    events = parse_log(log)
    key_times = [t + offset for t, op in events if op in ("line", "type", "key", "launch", "mark")]
    # `stop` event (written by record_check.sh just before `quit` / `exit`): afterward, the application
    # has ended and the terminal takes over (“[Process completed]”, window closing).
    # This is no longer the application: stop analysis slightly before the offset's error margin.
    stops = [t for t, op in events if op == "stop"]
    limit = int((stops[0] + offset - STOP_MARGIN) * fps) if stops else None

    empties, prompts = [], []
    d = {1: [], 2: [], 3: []}
    window: deque = deque(maxlen=4)
    prompt_changes, rewrites = {}, {}

    for k, gray in enumerate(read_frames(ff, video, w, h, fps)):
        if limit is not None and k >= limit:
            break
        bands = lay.bands(gray)
        ink = (np.abs(bands - lay.bg) > PIXEL_THRESHOLD).sum(axis=(1, 2))
        empty = ink < ROW_EMPTY
        nonempty = np.flatnonzero(~empty)
        prompts.append(int(nonempty[-1]) if len(nonempty) else -1)
        empties.append(empty)
        window.append(bands)
        for lag in (1, 2, 3):
            if len(window) > lag:
                d[lag].append(rdiff(window[-1 - lag], window[-1]))
        if k >= 1:
            before, after = window[-2], window[-1]
            pa, pb = prompts[k - 1], prompts[k]
            # Prompt: compare the same text line between frames, wherever it is on screen.
            if pa >= 0 and pb >= 0:
                n = int((np.abs(before[pa] - after[pb]) > PIXEL_THRESHOLD).sum())
                if n > ROW_CHANGED:
                    prompt_changes[k] = n
            # Existing logs: the area above the last `max_vp` lines.
            region = pb - max_vp - 1
            if region >= 0 and (d[1][k - 1][: region + 1] > ROW_CHANGED).any():
                best = None
                for s in range(0, max_vp + 1):
                    if s + region + 1 > lay.n:
                        break
                    bad = np.flatnonzero(rdiff(after[: region + 1], before[s: s + region + 1]) > ROW_CHANGED)
                    if best is None or len(bad) < len(best):
                        best = bad
                if best is not None and len(best):
                    rewrites[k] = tuple(int(r) for r in best)

    total = len(empties)
    found: list[dict] = []

    def add(kind, k, rows, detail):
        found.append({"kind": kind, "k": k, "rows": tuple(rows), "detail": detail})

    # 1. blank line between the last log and the viewport
    for k in range(total):
        p = prompts[k]
        nonempty = np.flatnonzero(~empties[k])
        if p < 0 or len(nonempty) == 0:
            continue
        first = int(nonempty[0])
        last = p if viewport is None else max(p - viewport + 1, first) - 1  # fixed:N: stop at the viewport
        gaps = [r for r in range(first, last + 1) if empties[k][r]]
        if gaps:
            add("gap", k, gaps, "blank line above the viewport")

    # 2. prompt changed without a keystroke
    for k, n in prompt_changes.items():
        t = k / fps
        if not key_times:
            break  # cannot assess without a log
        if not any(abs(t - kt) <= KEY_WINDOW for kt in key_times):
            add("prompt", k, [prompts[k]], f"{n} pixels changed with no key typed")

    # 3. logs rewritten without scrolling as a block
    for k, rows in rewrites.items():
        add("rewrite", k, rows, "log line modified without scrolling")

    # 4. intermediate states lasting 1 or 2 frames
    for j in range(1, total - 1):
        # A line cleared for one frame while filled before and after
        # (clear then redraw): the signature of an unsynchronized `clear`.
        blank = empties[j] & ~empties[j - 1] & ~empties[j + 1]
        if blank.any():
            add("transient", j, np.flatnonzero(blank), "blank line for one frame, filled before and after")
        a = d[1][j - 1] > ROW_CHANGED          # frame j differs from j-1
        eq2 = d[2][j - 1] <= ROW_CHANGED       # frame j-1 == frame j+1
        one = np.flatnonzero(a & eq2)
        if len(one):
            add("transient", j, one, "one-frame state, identical before and after")
        if j + 2 < total:
            eq3 = d[3][j - 1] <= ROW_CHANGED   # frame j-1 == frame j+2
            same = d[1][j] <= ROW_CHANGED      # frames j and j+1 are identical
            back = d[1][j + 1] > ROW_CHANGED   # j+1 differs from j+2
            two = np.flatnonzero(a & eq3 & same & back)
            if len(two):
                add("transient", j, two, "two-frame state, identical before and after")

    return lay, ff, (w, h), total, group(found)


def group(found: list[dict]) -> list[dict]:
    """Merge identical anomalies in consecutive frames."""
    found.sort(key=lambda a: (a["kind"], a["rows"], a["k"]))
    groups: list[dict] = []
    for a in found:
        g = groups[-1] if groups else None
        if g and g["kind"] == a["kind"] and g["rows"] == a["rows"] and a["k"] <= g["end"] + 1:
            g["end"] = a["k"]
        else:
            groups.append({**a, "start": a["k"], "end": a["k"]})
    groups.sort(key=lambda a: a["start"])
    return groups


# --------------------------------------------------------------------------- report


def write_report(video, out_dir, fps, lay, ff, size, total, anomalies, max_boards=20):
    out_dir.mkdir(parents=True, exist_ok=True)
    w, h = size
    wanted = set()
    for a in anomalies[:max_boards]:
        wanted.update({max(a["start"] - 1, 0), a["start"], min(a["end"] + 1, total - 1)})
    frames = {}
    if wanted:
        for idx, img in zip(sorted(wanted), read_frames(ff, video, w, h, fps, pix="rgb24", only=wanted)):
            frames[idx] = Image.fromarray(img).crop((lay.x0, lay.y0, lay.x1, lay.y1))

    lines = [
        f"# frames_check report: {Path(video).name}",
        "",
        f"- frames analyzed: {total} ({total / fps:.2f} s at {fps} fps)",
        f"- text area: x {lay.x0}-{lay.x1}, y {lay.y0}-{lay.y1}; {lay.n} lines of {lay.pitch:.2f} px",
        f"- anomalies: {len(anomalies)}",
        "",
    ]
    for i, a in enumerate(anomalies):
        span = f"frame {a['start']}" if a["start"] == a["end"] else f"frames {a['start']}-{a['end']}"
        lines.append(f"{i + 1}. **{a['kind']}** at t={a['start'] / fps:.3f} s ({span}), rows {list(a['rows'])}: {a['detail']}")
        if i < max_boards:
            board = make_board(frames, a, total)
            if board:
                name = f"anomaly-{i + 1:02d}-{a['kind']}.png"
                board.save(out_dir / name)
                lines.append(f"   - board: `{name}`")
    if not anomalies:
        lines.append("No anomalies.")
    report = out_dir / "report.md"
    report.write_text("\n".join(lines) + "\n")
    return report, "\n".join(lines)


def make_board(frames, a, total):
    picks = [("before", max(a["start"] - 1, 0)), ("anomaly", a["start"]), ("after", min(a["end"] + 1, total - 1))]
    imgs = [(label, frames[i]) for label, i in picks if i in frames]
    if not imgs:
        return None
    w, h = imgs[0][1].size
    board = Image.new("RGB", (w * len(imgs) + 10 * (len(imgs) - 1), h + 20), (255, 0, 255))
    draw = ImageDraw.Draw(board)
    for n, (label, img) in enumerate(imgs):
        board.paste(img, (n * (w + 10), 20))
        draw.text((n * (w + 10) + 4, 4), label, fill=(255, 255, 255))
    return board


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("video")
    ap.add_argument("--log", help="log written by record_check.sh")
    ap.add_argument("--fps", type=int, default=60)
    ap.add_argument("--viewport", default="dynamic", help="dynamic | fixed:N")
    ap.add_argument("--max-viewport", type=int, default=6)
    ap.add_argument("--crop", help="x0,y0,x1,y1")
    ap.add_argument("--pitch", type=float)
    ap.add_argument("--origin", type=float)
    ap.add_argument("--offset", type=float, default=1.0,
                    help="offset (s) between log and video (record_check.sh waits 1 s before T0; "
                         "measured: 1.05 to 1.13 s)")
    ap.add_argument("--out", help="report folder (default: <video>.report)")
    args = ap.parse_args()

    if not Path(args.video).exists():
        print(f"video not found: {args.video}", file=sys.stderr)
        return 2
    viewport = None
    if args.viewport.startswith("fixed:"):
        viewport = int(args.viewport.split(":")[1])
    elif args.viewport != "dynamic":
        print("--viewport: dynamic or fixed:N", file=sys.stderr)
        return 2
    crop = tuple(int(v) for v in args.crop.split(",")) if args.crop else None

    lay, ff, size, total, anomalies = analyze(
        args.video, args.log, args.fps, viewport, args.max_viewport, crop, args.pitch, args.origin, args.offset
    )
    out_dir = Path(args.out) if args.out else Path(args.video + ".report")
    report, text = write_report(args.video, out_dir, args.fps, lay, ff, size, total, anomalies)
    print(text)
    print(f"\nReport: {report}")
    return 1 if anomalies else 0


if __name__ == "__main__":
    sys.exit(main())
