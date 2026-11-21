//! Helpers shared by the text fuzz targets: text built from tricky pieces, and the
//! invariant check of a `Buffer`.

use arbitrary::Arbitrary;
use tilefab_core::Buffer;
use tilefab_core::buffer::grapheme_width;

/// Strings that random bytes would rarely produce: wide characters, combining marks,
/// emoji sequences, control characters and escape sequences, tabs and line breaks.
const PIECES: &[&str] = &[
    " ",
    "a",
    "é",
    "e\u{301}",
    "\u{301}",
    "日",
    "本語",
    "🙂",
    "👩‍👩‍👧",
    "🇫🇷",
    "\u{200b}",
    "\u{feff}",
    "\t",
    "\n",
    "\r\n",
    "\x1b[31m",
    "\x07",
    "\x00",
    "  ",
    "ｗ",
    "\u{2028}",
];

/// A piece of text: either arbitrary (valid UTF-8 from the fuzzer's bytes) or a tricky
/// piece picked from [`PIECES`].
#[derive(Debug, Arbitrary)]
pub enum Piece {
    Raw(String),
    Pick(u8),
}

/// Concatenate the pieces.
pub fn text(pieces: &[Piece]) -> String {
    let mut text = String::new();
    for piece in pieces {
        match piece {
            Piece::Raw(raw) => text.push_str(raw),
            Piece::Pick(index) => text.push_str(PIECES[usize::from(*index) % PIECES.len()]),
        }
    }
    text
}

/// Check the invariants of `buf` (see `tilefab_core::buffer::Cell`): one cell per position,
/// each cell a printable grapheme one or two columns wide, a wide character always followed
/// by its continuation, and a continuation only right after a wide character.
pub fn check_buffer(buf: &Buffer) {
    let area = buf.area();
    assert_eq!(
        buf.content().len(),
        usize::try_from(area.area()).unwrap(),
        "content length differs from the area"
    );
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = buf.get(x, y);
            if cell.is_continuation() {
                assert!(
                    x > area.left(),
                    "continuation in the first column at ({x}, {y})"
                );
                assert_eq!(
                    buf.get(x - 1, y).width(),
                    2,
                    "continuation not after a wide character at ({x}, {y})"
                );
                continue;
            }
            let symbol = cell.symbol();
            assert!(
                !symbol.chars().any(char::is_control),
                "control character in a cell at ({x}, {y}): {symbol:?}"
            );
            let width = cell.width();
            assert!(
                matches!(width, 1 | 2),
                "cell width {width} at ({x}, {y}): {symbol:?}"
            );
            assert_eq!(
                grapheme_width(symbol),
                width,
                "measured width differs: {symbol:?}"
            );
            if width == 2 {
                assert!(
                    x + 1 < area.right() && buf.get(x + 1, y).is_continuation(),
                    "wide character without continuation at ({x}, {y}): {symbol:?}"
                );
            }
        }
    }
}
