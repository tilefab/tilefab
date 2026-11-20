#!/usr/bin/env python3
"""Self-test of frames_check.py on synthetic videos (no macOS permissions required).

    tools/.venv/bin/python tools/selftest_frames.py

Generate a clean video (logs scrolling above a bottom-anchored viewport with variable
height), then videos with an injected anomaly, and check that frames_check.py:
  * reports nothing for the clean video (no false positives);
  * detects every injected anomaly.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

sys.path.insert(0, str(Path(__file__).parent))
import frames_check as fc  # noqa: E402

ROWS, COLS = 20, 60
PITCH, CHROME = 18, 40
W, H = 640, CHROME + ROWS * PITCH + 20
FPS = 60
HOLD = 6  # frames per state
FONT = next(
    (ImageFont.truetype(p, 14) for p in ("/System/Library/Fonts/Menlo.ttc", "/System/Library/Fonts/Monaco.ttf") if Path(p).exists()),
    ImageFont.load_default(),
)


def render(lines: list[str]) -> Image.Image:
    """A terminal window: title bar (chrome) + light text on a dark background."""
    img = Image.new("RGB", (W, H), (30, 30, 30))
    d = ImageDraw.Draw(img)
    d.rectangle((0, 0, W, CHROME - 6), fill=(200, 200, 200))  # light title bar
    for i, text in enumerate(lines[-ROWS:]):
        d.text((8, CHROME + i * PITCH), text, font=FONT, fill=(230, 230, 230))
    return img


def viewport(tasks: int) -> list[str]:
    return [f"tache{n}  ########---------  {n * 10:3d}%" for n in range(tasks)] + ["> _"]


def scenario(anomaly: str | None) -> list[list[str]]:
    """Sequence of screen states (each state is a list of lines, the last at the bottom)."""
    logs = [f"[INFO] log line number {i}" for i in range(30)]  # screen already full
    states = []
    tasks = 3
    for step in range(8):
        # task completion: viewport -1, log +1 (the prompt stays put)
        logs.append(f"[INFO] task {step} finished")
        tasks = max(tasks - 1, 0) if step % 2 == 0 else tasks + 1
        screen = logs + viewport(tasks)
        if anomaly == "gap" and step == 3:
            states.append(logs + [""] + viewport(tasks))          # blank line above the viewport
        if anomaly == "transient" and step == 4:
            blanked = viewport(tasks)
            blanked[0] = ""                                        # line cleared for one frame, then redrawn
            states.append(logs + blanked)
        if anomaly == "rewrite" and step == 5:
            old = list(logs)
            old[-12] = "[INFO] line REWRITTEN"                     # existing log (outside the viewport) modified
            states.append(old + viewport(tasks))
        states.append(screen)
    return states


def encode(states: list[list[str]], out: Path, hold_for=None) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        n = 0
        for idx, st in enumerate(states):
            # A single-frame anomaly lasts 1 frame; normal states last HOLD frames.
            hold = hold_for(idx) if hold_for else HOLD
            img = render(st)
            for _ in range(hold):
                img.save(f"{tmp}/{n:05d}.png")
                n += 1
        ff = fc.ffmpeg_exe()
        subprocess.run([ff, "-v", "error", "-y", "-framerate", str(FPS), "-i", f"{tmp}/%05d.png",
                        "-c:v", "libx264", "-crf", "12", "-pix_fmt", "yuv420p", str(out)], check=True)


def check(video: Path, expect_kind: str | None) -> bool:
    lay, ff, size, total, anomalies = fc.analyze(str(video), None, FPS, None, 6, None, None, None, 0.0)
    kinds = sorted({a["kind"] for a in anomalies})
    ok = (not anomalies) if expect_kind is None else (expect_kind in kinds)
    print(f"{video.name:20s} images={total:3d} pitch={lay.pitch:.2f} anomalies={kinds or 'none'} -> {'OK' if ok else 'FAILED'}")
    return ok


def main() -> int:
    ok = True
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        clean = tmp / "clean.mp4"
        encode(scenario(None), clean)
        ok &= check(clean, None)
        for kind in ("gap", "transient", "rewrite"):
            states = scenario(kind)
            # injected states = those differing from the clean version: held for only 1 frame
            base = scenario(None)
            injected = {i for i, s in enumerate(states) if s not in base}
            video = tmp / f"{kind}.mp4"
            encode(states, video, hold_for=lambda i, inj=injected: 1 if i in inj else HOLD)
            ok &= check(video, kind)
    print("\nSELF-TEST", "PASSED" if ok else "FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
