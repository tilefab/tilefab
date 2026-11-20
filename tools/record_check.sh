#!/bin/bash
# Run a scenario in Terminal.app or iTerm2 and record it, to check what the terminal really
# displays (flicker, blank lines). Record ONLY the terminal window (not the entire screen).
# macOS prerequisites, granted once by hand: "Screen Recording" for the terminal running this
# script, and "Automation" for `osascript`.
#
#   tools/record_check.sh <terminal|iterm> <scenario> [command]
#
#   scenario   file name in tools/scenarios/ (without .scn) or path to a .scn file
#   command    program to launch (default: target/debug/tilefab)
#
# Output: target/recordings/<date>-<terminal>-<scenario>.mov and .log (timestamped log).
# Then:  tools/.venv/bin/python tools/frames_check.py <video>.mov --log <video>.log
#
# Scenario format (one instruction per line, `#` = comment):
#   wait <seconds>     pause
#   line <text>        type text followed by Enter in the terminal
#   mark <name>        marker in the log (does nothing else)
#   stop               mark the end of the application (automatic before `quit` / `exit`);
#                      frames_check.py does not analyze what follows
#
# macOS prerequisites, granted manually ONCE (system dialogs):
#   - “Screen Recording” for the application launching this script;
#   - “Automation”: this application must be allowed to control Terminal / iTerm2 (osascript).
# The terminal must stay in the foreground, unobscured, during capture.
#
# Window: 100 x 30 characters, NON-blinking cursor (otherwise each blink is detected
# as a change during analysis). Variables: COLS, ROWS, FONT_SIZE.

set -euo pipefail
# A shell launched by a tool sometimes has a reduced PATH (without Homebrew).
export PATH="$PATH:/opt/homebrew/bin:/usr/local/bin"
# C locale: otherwise, in a French locale, `printf '%.2f'` rejects “1.11” (expecting “1,11”)
# and `set -e` stops the script while leaving screencapture running.
export LC_ALL=C

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TERM_NAME="${1:-}"
SCENARIO="${2:-}"
COMMAND="${3:-$ROOT/target/debug/tilefab}"
COLS="${COLS:-100}"
ROWS="${ROWS:-30}"
FONT_SIZE="${FONT_SIZE:-14}"
PROFILE="tui-test"

if [[ -z "$TERM_NAME" || -z "$SCENARIO" ]]; then
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
case "$TERM_NAME" in
    terminal | Terminal) TERM_NAME=terminal ;;
    iterm | iTerm | iterm2 | iTerm2) TERM_NAME=iterm ;;
    *) echo "unknown terminal: $TERM_NAME (terminal | iterm)" >&2; exit 2 ;;
esac
[[ -f "$SCENARIO" ]] || SCENARIO="$ROOT/tools/scenarios/$SCENARIO.scn"
[[ -f "$SCENARIO" ]] || { echo "scenario not found: $SCENARIO" >&2; exit 2; }
[[ -x "$COMMAND" ]] || { echo "command not found: $COMMAND (cargo build?)" >&2; exit 2; }

OUT_DIR="$ROOT/target/recordings"
mkdir -p "$OUT_DIR"
NAME="$(date +%Y%m%d-%H%M%S)-$TERM_NAME-$(basename "$SCENARIO" .scn)"
VIDEO="$OUT_DIR/$NAME.mov"
LOG="$OUT_DIR/$NAME.log"

now() { perl -MTime::HiRes=time -e 'printf "%.3f", time'; }

# --- test profile ----------------------------------------------------------------------------

# Terminal.app: copy the “Basic” profile into a .terminal file (steady cursor, window
# size), then import it by opening it.
make_terminal_profile() {
    local file="$OUT_DIR/$PROFILE.terminal"
    python3 - "$file" "$PROFILE" "$COLS" "$ROWS" "$FONT_SIZE" <<'PY'
import plistlib, subprocess, sys
path, name, cols, rows, size = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), sys.argv[5]
prefs = plistlib.loads(subprocess.run(["defaults", "export", "com.apple.Terminal", "-"],
                                      capture_output=True, check=True).stdout)
profile = dict(prefs["Window Settings"].get("Basic", {}))
profile.update({"name": name, "type": "Window Settings", "CursorBlink": False,
                "columnCount": cols, "rowCount": rows, "ShowActiveProcessInTitle": False})
with open(path, "wb") as f:
    plistlib.dump(profile, f)
PY
    open -g "$file"   # import the profile (also opens a window, closed afterward)
    sleep 2
    osascript -e 'tell application "Terminal" to close (every window whose name contains "'"$PROFILE"'")' >/dev/null 2>&1 || true
}

# iTerm2: dynamic profile (JSON), applied without restarting.
make_iterm_profile() {
    local dir="$HOME/Library/Application Support/iTerm2/DynamicProfiles"
    mkdir -p "$dir"
    cat >"$dir/$PROFILE.json" <<JSON
{"Profiles": [{
  "Name": "$PROFILE", "Guid": "tilefab-test-profile",
  "Dynamic Profile Parent Name": "Default",
  "Blinking Cursor": false, "Columns": $COLS, "Rows": $ROWS,
  "Normal Font": "Menlo-Regular $FONT_SIZE", "Custom Command": "No", "Silence Bell": true
}]}
JSON
    sleep 2
}

# --- terminal control ------------------------------------------------------------------------

# The *_open functions print “x,y,width,height” for the capture area.
terminal_open() {
    osascript <<OSA
tell application "Terminal"
    activate
    set t to do script ""
    set current settings of t to settings set "$PROFILE"
    -- Applying the profile does not always resize the window: set its size manually.
    set number of columns of t to $COLS
    set number of rows of t to $ROWS
    set w to front window
    set position of w to {80, 60}
    delay 0.5
    do script "clear; exec '$COMMAND'" in t
    set b to bounds of w
    return "" & (item 1 of b) & "," & (item 2 of b) & "," & ((item 3 of b) - (item 1 of b)) & "," & ((item 4 of b) - (item 2 of b))
end tell
OSA
}
terminal_line() {
    # `do script` in a busy tab sends text to the running program, followed by Enter.
    osascript -e "tell application \"Terminal\" to do script \"$1\" in selected tab of front window" >/dev/null
}
terminal_close() { osascript -e 'tell application "Terminal" to close front window' >/dev/null 2>&1 || true; }

iterm_open() {
    osascript <<OSA
tell application "iTerm"
    activate
    set w to (create window with profile "$PROFILE")
    tell current session of w to write text "clear; exec '$COMMAND'"
    delay 0.5
    set b to bounds of w
    return "" & (item 1 of b) & "," & (item 2 of b) & "," & ((item 3 of b) - (item 1 of b)) & "," & ((item 4 of b) - (item 2 of b))
end tell
OSA
}
iterm_line() {
    osascript -e "tell application \"iTerm\" to tell current session of current window to write text \"$1\"" >/dev/null
}
iterm_close() { osascript -e 'tell application "iTerm" to close current window' >/dev/null 2>&1 || true; }

# Escape " and \ for inclusion in an AppleScript string.
osa_escape() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'; }

# --- execution -------------------------------------------------------------------------------

echo "Terminal: $TERM_NAME   scenario: $SCENARIO   window: ${COLS}x${ROWS}"
if [[ "$TERM_NAME" == terminal ]]; then make_terminal_profile; else make_iterm_profile; fi

GEOM="$("${TERM_NAME}_open")"
IFS=, read -r X Y W H <<<"$GEOM"
echo "filmed area: $X,$Y ${W}x${H}"
sleep 1.5   # let the application start and draw

# `screencapture` records the mouse pointer: it appears in the video and is mistaken
# for text during analysis. Move it to a screen corner, outside the capture area.
osascript -l JavaScript -e 'ObjC.import("CoreGraphics"); $.CGWarpMouseCursorPosition($.CGPointMake(2, 2)); 0' >/dev/null 2>&1 || true

: >"$LOG"
T0=0
log() { printf 't=%.2f %s\n' "$(echo "$(now) - $T0" | bc -l)" "$*" >>"$LOG"; }

# `-v`: video; `-R`: region; no duration: stop with SIGINT at the end of the scenario.
screencapture -v -x -R"$X,$Y,$W,$H" "$VIDEO" &
CAP=$!
sleep 1.0
T0="$(now)"
log "launch $COMMAND"

while IFS= read -r raw || [[ -n "$raw" ]]; do
    line="${raw%%#*}"
    [[ -z "${line//[[:space:]]/}" ]] && continue
    op="${line%% *}"
    arg="${line#* }"
    [[ "$arg" == "$line" ]] && arg=""
    case "$op" in
        wait) sleep "$arg" ;;
        mark) log "mark $arg" ;;
        stop) log "stop" ;;   # the application is about to end (see `line` below)
        line)
            # After quit / exit, the application has ended: the log marks this so
            # frames_check.py does not analyze subsequent terminal activity (window closing…).
            [[ "$arg" == quit || "$arg" == exit ]] && log "stop"
            log "line $arg"; "${TERM_NAME}_line" "$(osa_escape "$arg")" ;;
        *) echo "unknown instruction: $op" >&2 ;;
    esac
done <"$SCENARIO"

sleep 1.0
log "end"
kill -INT "$CAP" 2>/dev/null || true
wait "$CAP" 2>/dev/null || true
"${TERM_NAME}_close"

if [[ -s "$VIDEO" ]]; then
    echo "Video:    $VIDEO"
    echo "Log:      $LOG"
    echo "Analysis: tools/.venv/bin/python tools/frames_check.py '$VIDEO' --log '$LOG'"
else
    echo "No video produced: \"Screen Recording\" permission missing?" >&2
    exit 1
fi
