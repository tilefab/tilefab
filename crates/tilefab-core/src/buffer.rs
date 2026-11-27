//! `Cell` and `Buffer`: the cell grid in which widgets draw.
//!
//! # Wide characters
//!
//! An emoji or ideograph occupies **two columns**. It is stored as follows:
//!
//! ```text
//! column:     0     1    2
//! content:  "😀"   ""   "a"
//!            └head └─ continuation cell (empty symbol)
//! ```
//!
//! The *head* holds the grapheme; the next cell, with an empty symbol, is its *continuation*.
//! The backend never prints a continuation: the terminal already covered it when printing
//! the head. Writing over either half of a wide character replaces the other half
//! with a space, so half an emoji is never left on screen.
//!
//! Cells changed in place ([`Buffer::get_mut`], [`Buffer::content_mut`]) can break this
//! pairing: a wide symbol set with [`Cell::set_symbol`] before an ordinary cell, or a
//! continuation copied elsewhere. [`Buffer::normalize_wide`] repairs it, and `Terminal`
//! calls it on every frame: the screen never holds a cell the diff does not know about.

use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::convert::saturating_u16;
use super::layout::Rect;
use super::style::{Color, Modifier, Style};

/// Number of screen columns occupied by a grapheme: 0, 1, or 2.
///
/// Control characters count as 0 (the buffer ignores them), as do zero-width
/// graphemes. A cell holds at most two columns: a grapheme reported wider is shown as a
/// space (see [`Cell`]), so it counts as 1.
pub fn grapheme_width(grapheme: &str) -> usize {
    if grapheme.chars().any(char::is_control) {
        return 0;
    }
    match grapheme.width() {
        width @ 0..=2 => width,
        _ => 1,
    }
}

/// `area` clipped like [`Rect::new`]: its right and bottom edges do not exceed `u16::MAX`,
/// so that every cell has coordinates.
fn clipped(area: Rect) -> Rect {
    Rect::new(area.x, area.y, area.width, area.height)
}

/// Bytes a symbol holds without a heap allocation: enough for nearly every grapheme
/// (accented letters, CJK, most emoji). Longer ones (some emoji sequences, such as a family
/// joined by ZWJ) use the heap: writing one allocates once, copying its cell does not.
const INLINE_SYMBOL: usize = 22;

/// A cell's symbol, stored in the cell itself when it is short.
///
/// Same size as a `String`, but creating or copying a short symbol never allocates: a
/// frame clones and compares thousands of cells. The representation is canonical (inline
/// whenever it fits, unused bytes zeroed), so the derived equality compares contents. An
/// inline symbol also keeps its width, measured once when it is created: the diff, the
/// backend and the handling of wide characters read it for every cell of every frame.
#[derive(Clone, PartialEq, Eq)]
enum Symbol {
    /// The first `len` bytes of `bytes`, valid UTF-8, `width` columns wide: both packed in
    /// `packed` (see [`Symbol::pack`]), so that the symbol stays the size of a `String`.
    Inline {
        packed: u8,
        bytes: [u8; INLINE_SYMBOL],
    },
    /// Longer than `INLINE_SYMBOL` bytes, shared by its copies: copying a cell never
    /// allocates (only writing such a symbol does).
    Heap(Arc<str>),
}

impl Symbol {
    /// A continuation (second half of a wide character).
    const EMPTY: Self = Self::Inline {
        packed: Self::pack(0, 0),
        bytes: [0; INLINE_SYMBOL],
    };
    /// A space, the default symbol.
    const SPACE: Self = {
        let mut bytes = [0; INLINE_SYMBOL];
        bytes[0] = b' ';
        Self::Inline {
            packed: Self::pack(1, 1),
            bytes,
        }
    };

    /// `len` (at most `INLINE_SYMBOL`, 5 bits) and `width` (0 to 2) in one byte.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`len <= INLINE_SYMBOL < 32` and `width <= 2`: both fit in a byte"
    )]
    const fn pack(len: usize, width: usize) -> u8 {
        (len | width << 5) as u8
    }

    /// `symbol`, `width` columns wide (its width, as measured by the caller).
    fn with_width(symbol: &str, width: usize) -> Self {
        if symbol.len() <= INLINE_SYMBOL {
            let mut bytes = [0; INLINE_SYMBOL];
            bytes[..symbol.len()].copy_from_slice(symbol.as_bytes());
            Self::Inline {
                packed: Self::pack(symbol.len(), width),
                bytes,
            }
        } else {
            Self::Heap(symbol.into())
        }
    }

    /// `grapheme` (a single grapheme) as a symbol, measured once: `None` if it is not
    /// printable (a control character, or zero columns wide); a grapheme wider than two
    /// columns is shown as a space (see [`Cell`]). The symbol's width is [`grapheme_width`]
    /// of `grapheme`.
    fn measure(grapheme: &str) -> Option<Self> {
        // Fast path for the most common case, a single ASCII character.
        if let &[byte] = grapheme.as_bytes() {
            return (byte == b' ' || byte.is_ascii_graphic())
                .then(|| Self::with_width(grapheme, 1));
        }
        if grapheme.chars().any(char::is_control) {
            return None;
        }
        match grapheme.width() {
            0 => None,
            width @ (1 | 2) => Some(Self::with_width(grapheme, width)),
            _ => Some(Self::SPACE),
        }
    }

    /// The first grapheme of `symbol` if it is printable and one or two columns wide,
    /// otherwise a space (see [`Cell`]). Never a continuation.
    fn printable(symbol: &str) -> Self {
        let first = if symbol.len() == 1 {
            Some(symbol)
        } else {
            symbol.graphemes(true).next()
        };
        first.and_then(Self::measure).unwrap_or(Self::SPACE)
    }

    fn as_str(&self) -> &str {
        match self {
            // Always valid: the bytes come from a `&str` cut at its end. Without `unsafe`
            // (forbidden in this project), the check is redone; it is cheap on 22 bytes.
            Self::Inline { packed, bytes } => {
                std::str::from_utf8(&bytes[..usize::from(packed & 0x1f)]).unwrap_or_default()
            }
            Self::Heap(symbol) => symbol,
        }
    }

    /// Number of columns: stored when inline; measured again for a (rare) long symbol, a
    /// printable grapheme one or two columns wide.
    fn width(&self) -> usize {
        match self {
            Self::Inline { packed, .. } => usize::from(packed >> 5),
            Self::Heap(symbol) => symbol.width(),
        }
    }

    fn is_empty(&self) -> bool {
        matches!(self, Self::Inline { packed: 0, .. })
    }
}

impl std::fmt::Debug for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_str(), f)
    }
}

/// A screen cell: a symbol (a grapheme) and a *complete* style.
///
/// # Invariant: a symbol is a printable grapheme
///
/// A symbol is one printable grapheme one or two columns wide, or empty for a continuation.
/// [`Cell::new`] and [`Cell::set_symbol`] keep only the first grapheme of the string they
/// are given, and replace it with a space if it contains a control character (such as
/// `ESC`, `BEL` or a line feed), is zero-width (a lone combining mark) or is wider than two
/// columns: no escape sequence can reach the terminal through a cell, and a cell always
/// prints as many columns as [`Cell::width`] says. A continuation is only created by
/// [`Buffer::write_grapheme`] (and `Buffer::set_string`), right after a wide character.
/// `Buffer::set_string` and its variants also cut text into graphemes and skip control
/// characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The displayed grapheme. Empty for a continuation cell.
    symbol: Symbol,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// Bold, italic, underlined…
    pub modifier: Modifier,
}

impl Default for Cell {
    /// An unstyled space.
    fn default() -> Self {
        Self {
            symbol: Symbol::SPACE,
            fg: Color::Reset,
            bg: Color::Reset,
            modifier: Modifier::empty(),
        }
    }
}

impl Cell {
    /// An unstyled cell displaying the first grapheme of `symbol` (a space if it is not
    /// printable, see [`Cell`]). Never a continuation, even for `""`.
    ///
    /// A wide character (two columns) needs a continuation in the next cell: put such a cell
    /// in a buffer with [`Buffer::write_grapheme`] or [`Buffer::filled`], which create it,
    /// not through [`Buffer::content_mut`] or [`Buffer::get_mut`].
    #[must_use]
    pub fn new(symbol: &str) -> Self {
        Self {
            symbol: Symbol::printable(symbol),
            ..Self::default()
        }
    }

    /// The displayed grapheme: empty for a continuation, never a control character.
    #[must_use]
    pub fn symbol(&self) -> &str {
        self.symbol.as_str()
    }

    /// Change the symbol without changing the style: the first grapheme of `symbol`, or a
    /// space if it is not printable (see [`Cell`]). Never a continuation, even for `""`.
    ///
    /// For a wide character in a buffer, use [`Buffer::write_grapheme`] instead: it also turns
    /// the next cell into the continuation (see [`Cell::new`]).
    pub fn set_symbol(&mut self, symbol: &str) -> &mut Self {
        self.symbol = Symbol::printable(symbol);
        self
    }

    /// Apply a partial style: only fields set by `style` change.
    pub fn set_style(&mut self, style: Style) -> &mut Self {
        if let Some(fg) = style.fg {
            self.fg = fg;
        }
        if let Some(bg) = style.bg {
            self.bg = bg;
        }
        self.modifier.remove(style.sub_modifier);
        self.modifier.insert(style.add_modifier);
        self
    }

    /// The cell's style as a `Style` with every field set.
    #[must_use]
    pub fn style(&self) -> Style {
        Style {
            fg: Some(self.fg),
            bg: Some(self.bg),
            add_modifier: self.modifier,
            sub_modifier: Modifier::empty(),
        }
    }

    /// Reset the cell to its default state (an unstyled space).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// True for the second half of a wide character.
    #[must_use]
    pub fn is_continuation(&self) -> bool {
        self.symbol.is_empty()
    }

    /// Number of columns occupied by the symbol: 0 for a continuation, 2 for a wide
    /// character.
    #[must_use]
    pub fn width(&self) -> usize {
        self.symbol.width()
    }
}

/// A cell grid covering the rectangle `area`.
///
/// Coordinates are screen coordinates: the top-left cell is at
/// `(area.x, area.y)`. Cells are stored in row-major order.
///
/// The fields are private to keep the invariant `content().len() == area().area()`: cells
/// can be changed in place ([`Buffer::content_mut`], [`Buffer::get_mut`]…), but only
/// [`Buffer::reset_to`] changes the area, together with the number of cells.
#[derive(Debug, PartialEq, Eq)]
pub struct Buffer {
    /// The covered area, in screen coordinates.
    area: Rect,
    /// Cells in row-major order (`area.area()` elements).
    content: Vec<Cell>,
}

impl Clone for Buffer {
    fn clone(&self) -> Self {
        Self {
            area: self.area,
            content: self.content.clone(),
        }
    }

    /// Reuses this buffer's memory (the derived version would allocate a new `Vec`):
    /// `Terminal` copies buffers this way on every frame.
    fn clone_from(&mut self, source: &Self) {
        self.area = source.area;
        self.content.clone_from(&source.content);
    }
}

impl Buffer {
    /// The covered area, in screen coordinates.
    #[must_use]
    pub fn area(&self) -> Rect {
        self.area
    }

    /// Cells in row-major order (`area().area()` elements).
    #[must_use]
    pub fn content(&self) -> &[Cell] {
        &self.content
    }

    /// Cells in row-major order, to change in place: a slice, so that their number (tied to
    /// the area) cannot change.
    ///
    /// The pairing of wide characters is the caller's job here: a wide character written
    /// with [`Cell::set_symbol`] must be followed by its continuation, which only
    /// [`Buffer::write_grapheme`] creates; write wide characters with it (or
    /// [`Buffer::set_string`]) instead. A broken pair is shown as spaces (see
    /// [`Buffer::normalize_wide`]).
    pub fn content_mut(&mut self) -> &mut [Cell] {
        &mut self.content
    }

    /// A buffer with all cells set to spaces.
    #[must_use]
    pub fn empty(area: Rect) -> Self {
        Self::filled(area, Cell::default())
    }

    /// Make this an empty buffer covering `area` (all cells unstyled spaces), reusing its
    /// memory: no allocation once it has held that many cells. `Terminal` keeps its
    /// buffers from one frame to the next this way. The area is clipped as in
    /// [`Buffer::filled`].
    pub fn reset_to(&mut self, area: Rect) {
        self.area = clipped(area);
        self.content.clear();
        self.content
            .resize(self.area.area() as usize, Cell::default());
    }

    /// A buffer with all cells set to copies of `cell`.
    ///
    /// The area is first clipped like [`Rect::new`] does, so that no cell lies past
    /// coordinate `u16::MAX` (a `Rect` written literally skips that check). A wide `cell`
    /// (two columns) keeps the invariant of [`Cell`]: each copy is followed by its
    /// continuation, and when the width is odd the last column of each row is a space.
    #[must_use]
    pub fn filled(area: Rect, cell: Cell) -> Self {
        let area = clipped(area);
        let mut content = vec![cell; area.area() as usize];
        if content.first().is_some_and(|cell| cell.width() == 2) {
            let width = area.width as usize;
            for row in content.chunks_mut(width) {
                for pair in row.chunks_mut(2) {
                    match pair {
                        [_, tail] => tail.symbol = Symbol::EMPTY,
                        [last] => last.symbol = Symbol::SPACE,
                        _ => {}
                    }
                }
            }
        }
        Self { area, content }
    }

    /// A buffer built from text lines: convenient for
    /// writing expected test output. Its width is that of the widest line
    /// (in display columns); shorter lines are padded with spaces.
    pub fn with_lines<S: AsRef<str>>(lines: impl IntoIterator<Item = S>) -> Self {
        let lines: Vec<S> = lines.into_iter().collect();
        let width = lines.iter().map(|l| l.as_ref().width()).max().unwrap_or(0);
        let mut buffer = Self::empty(Rect::new(
            0,
            0,
            saturating_u16(width),
            saturating_u16(lines.len()),
        ));
        // Lines beyond `u16::MAX` are outside the buffer: `zip` stops there.
        for (y, line) in (0..=u16::MAX).zip(&lines) {
            buffer.set_string(0, y, line.as_ref(), Style::new());
        }
        buffer
    }

    /// The content, one string per line (continuations add nothing).
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        if self.area.is_empty() {
            return vec![String::new(); self.area.height as usize];
        }
        self.content
            .chunks(self.area.width as usize)
            .map(|row| row.iter().map(Cell::symbol).collect())
            .collect()
    }

    /// Index of cell `(x, y)` in `content`.
    ///
    /// # Panics
    ///
    /// If `(x, y)` is outside the buffer.
    #[must_use]
    pub fn index_of(&self, x: u16, y: u16) -> usize {
        assert!(
            x >= self.area.left()
                && x < self.area.right()
                && y >= self.area.top()
                && y < self.area.bottom(),
            "cell ({x}, {y}) outside the buffer {:?}",
            self.area
        );
        (y - self.area.y) as usize * self.area.width as usize + (x - self.area.x) as usize
    }

    /// Coordinates `(x, y)` of the cell at `index`.
    #[must_use]
    pub fn pos_of(&self, index: usize) -> (u16, u16) {
        let width = self.area.width as usize;
        (
            self.area.x + saturating_u16(index % width),
            self.area.y + saturating_u16(index / width),
        )
    }

    /// The cell at `(x, y)`.
    ///
    /// # Panics
    ///
    /// If `(x, y)` is outside the buffer.
    #[must_use]
    pub fn get(&self, x: u16, y: u16) -> &Cell {
        &self.content[self.index_of(x, y)]
    }

    /// Mutable access to the cell at `(x, y)`. The same remark as for [`Buffer::content_mut`]
    /// applies to wide characters.
    ///
    /// # Panics
    ///
    /// If `(x, y)` is outside the buffer.
    pub fn get_mut(&mut self, x: u16, y: u16) -> &mut Cell {
        let index = self.index_of(x, y);
        &mut self.content[index]
    }

    /// Reset all cells to their default state.
    pub fn reset(&mut self) {
        for cell in &mut self.content {
            cell.reset();
        }
    }

    /// Apply `style` to all cells in `area` (ignore the portion outside the buffer).
    pub fn set_style(&mut self, area: Rect, style: Style) {
        let area = self.area.intersection(area);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                self.get_mut(x, y).set_style(style);
            }
        }
    }

    /// Write `string` starting at `(x, y)`, with no width limit other than the buffer edge.
    /// Return the position after the last character written.
    pub fn set_string(&mut self, x: u16, y: u16, string: &str, style: Style) -> (u16, u16) {
        self.set_stringn(x, y, string, usize::MAX, style)
    }

    /// Like [`Buffer::set_string`], but write at most `max_width` columns.
    ///
    /// Writing stops at the first grapheme that does not fit, whether at `max_width` or
    /// the buffer edge: a wide character is never split in half. Control characters
    /// and zero-width graphemes are ignored.
    pub fn set_stringn(
        &mut self,
        x: u16,
        y: u16,
        string: &str,
        max_width: usize,
        style: Style,
    ) -> (u16, u16) {
        let area = self.area;
        if y < area.top() || y >= area.bottom() || x < area.left() || x >= area.right() {
            return (x, y);
        }
        let mut x = x;
        let mut remaining = max_width.min((area.right() - x) as usize);
        for grapheme in string.graphemes(true) {
            // Measured once: the symbol keeps its width.
            let Some(symbol) = Symbol::measure(grapheme) else {
                continue;
            };
            let width = symbol.width();
            if width > remaining {
                break;
            }
            self.write_symbol(x, y, symbol, style);
            x += saturating_u16(width);
            remaining -= width;
        }
        (x, y)
    }

    /// Write a grapheme at `(x, y)` and return the number of columns written (1 or 2,
    /// [`grapheme_width`] of `grapheme`); the caller has already checked the available
    /// space (to write graphemes already measured). A wide
    /// grapheme also turns `(x + 1, y)` into its continuation.
    ///
    /// Unlike [`Buffer::set_stringn`], nothing is clipped. `grapheme` is normalized like
    /// [`Cell::set_symbol`]: only its first grapheme is kept, and an unprintable one is
    /// written as a space (1 column).
    ///
    /// # Panics
    ///
    /// If `(x, y)`, or `(x + 1, y)` for a wide grapheme, is outside the buffer.
    pub fn write_grapheme(&mut self, x: u16, y: u16, grapheme: &str, style: Style) -> usize {
        let symbol = Symbol::printable(grapheme);
        let width = symbol.width();
        self.write_symbol(x, y, symbol, style);
        width
    }

    /// Write `symbol` at `(x, y)`, and for a wide one turn `(x + 1, y)` into its
    /// continuation (see [`Buffer::write_grapheme`]).
    fn write_symbol(&mut self, x: u16, y: u16, symbol: Symbol, style: Style) {
        let width = symbol.width();
        // Overwriting half a wide character: the other half becomes a space.
        self.break_wide(x, y);
        if width == 2 {
            self.break_wide(x + 1, y);
        }
        let head = self.get_mut(x, y);
        head.symbol = symbol;
        head.set_style(style);
        if width == 2 {
            // The continuation copies the head's style to keep the background uniform.
            let (fg, bg, modifier) = (head.fg, head.bg, head.modifier);
            let tail = self.get_mut(x + 1, y);
            tail.symbol = Symbol::EMPTY;
            tail.fg = fg;
            tail.bg = bg;
            tail.modifier = modifier;
        }
    }

    /// If `(x, y)` is part of a wide character, replace the *other* half with a space.
    /// Cell `(x, y)` itself is unchanged: the caller will overwrite it.
    fn break_wide(&mut self, x: u16, y: u16) {
        let cell = self.get(x, y);
        if cell.is_continuation() {
            // Second half: the head is immediately left (a continuation is never in the first column).
            if x > self.area.left() {
                self.get_mut(x - 1, y).set_symbol(" ");
            }
        } else if cell.width() == 2 && x + 1 < self.area.right() {
            self.get_mut(x + 1, y).set_symbol(" ");
        }
    }

    /// Repair the pairing of wide characters (see the module documentation), in place and
    /// without allocating: a wide head not followed by its continuation (or in the last
    /// column, where it does not fit) becomes a space, and so does a continuation that does
    /// not follow a wide head. Styles are kept. A buffer written only through
    /// [`Buffer::set_string`], [`Buffer::write_grapheme`] or [`Buffer::filled`] is unchanged.
    pub fn normalize_wide(&mut self) {
        let width = usize::from(self.area.width);
        if width == 0 {
            return;
        }
        for row in self.content.chunks_mut(width) {
            let mut x = 0;
            while x < row.len() {
                if row[x].width() == 2 && row.get(x + 1).is_some_and(Cell::is_continuation) {
                    x += 2;
                    continue;
                }
                if row[x].is_continuation() || row[x].width() == 2 {
                    row[x].symbol = Symbol::SPACE;
                }
                x += 1;
            }
        }
    }

    /// Cells to rewrite to go from `previous` to `self`, in screen order.
    ///
    /// Both buffers must cover the same area. Continuations are never
    /// returned: writing a wide character's head is enough for the terminal. This needs the
    /// pairing of wide characters to hold in `self` ([`Buffer::normalize_wide`]).
    ///
    /// There is no need to “redraw neighbors” when replacing a wide character with a
    /// narrow one (or vice versa): a continuation has an empty symbol, so it always differs
    /// from a real cell, and the remaining half is detected like any other change.
    ///
    /// # Panics
    ///
    /// If `self` and `previous` do not cover the same area.
    #[must_use]
    pub fn diff<'a>(&'a self, previous: &Buffer) -> Vec<(u16, u16, &'a Cell)> {
        self.diff_iter(previous).collect()
    }

    /// [`Buffer::diff`] as an iterator, without collecting it: `Terminal` hands it straight
    /// to `Backend::draw`.
    ///
    /// # Panics
    ///
    /// If `self` and `previous` do not cover the same area (checked at the call, not lazily).
    pub fn diff_iter<'a, 'b>(
        &'a self,
        previous: &'b Buffer,
    ) -> impl Iterator<Item = (u16, u16, &'a Cell)> + use<'a, 'b> {
        assert_eq!(
            self.area, previous.area,
            "diff between two buffers with different areas"
        );
        self.content
            .iter()
            .zip(&previous.content)
            .enumerate()
            .filter(|(_, (current, previous))| !current.is_continuation() && current != previous)
            .map(|(i, (current, _))| {
                let (x, y) = self.pos_of(i);
                (x, y, current)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn symbols_with_control_characters_become_a_space() {
        assert_eq!(Cell::new("a").symbol(), "a");
        assert_eq!(Cell::new("😀").symbol(), "😀");
        // Only the first grapheme is kept: what follows it, control characters included,
        // never reaches the terminal.
        assert_eq!(Cell::new("a\nb").symbol(), "a");
        assert_eq!(Cell::new("\nb").symbol(), " ");
        assert_eq!(Cell::new("\r\n").symbol(), " ");
        let mut cell = Cell::new("x");
        cell.set_symbol("\x1b[2J");
        assert_eq!(cell.symbol(), " ");
        assert_eq!(cell.width(), 1);
    }

    proptest::proptest! {
        #[test]
        fn the_stored_width_is_the_measured_one(
            text in "[a ~\u{7f}\u{1b}\u{301}\u{200d}é界😀👨👩\u{fe0f}\u{1f3fb}\u{a0}\u{ad}]{0,8}",
        ) {
            let measured = |cell: &Cell| -> usize {
                unicode_width::UnicodeWidthStr::width(cell.symbol())
            };
            let cell = Cell::new(&text);
            proptest::prop_assert_eq!(cell.width(), measured(&cell));
            let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
            buf.set_string(0, 0, &text, Style::new());
            for cell in buf.content() {
                proptest::prop_assert_eq!(cell.width(), measured(cell), "{:?}", cell);
            }
        }
    }

    #[test]
    fn short_symbols_are_inline_and_long_ones_round_trip_through_the_heap() {
        assert_eq!(std::mem::size_of::<Symbol>(), std::mem::size_of::<String>());
        assert!(matches!(Cell::default().symbol, Symbol::Inline { .. }));
        // 22 bytes: the longest inline symbol.
        let longest = "é".repeat(11);
        assert!(matches!(Symbol::printable(&longest), Symbol::Inline { .. }));
        assert_eq!(Symbol::printable(&longest).as_str(), "é");
        assert!(matches!(
            Symbol::with_width(&longest, 1),
            Symbol::Inline { .. }
        ));
        assert_eq!(Symbol::with_width(&longest, 1).as_str(), longest);
        // A family emoji: 25 bytes, one grapheme.
        let family = "👨\u{200d}👩\u{200d}👧\u{200d}👦";
        assert!(family.len() > INLINE_SYMBOL);
        let cell = Cell::new(family);
        assert!(matches!(cell.symbol, Symbol::Heap(_)));
        assert_eq!(cell.symbol(), family);
        assert_eq!(
            cell,
            Cell::new(family),
            "equality across clones of a heap symbol"
        );
        // Canonical form: the same text always compares equal, whatever was there before.
        let mut reused = Cell::new("abcdefgh");
        reused.set_symbol("a");
        assert_eq!(reused, Cell::new("a"));
    }

    #[test]
    fn cell_debug_shows_the_symbol_text_and_style_is_complete() {
        let mut cell = Cell::new("a");
        cell.set_style(Style::new().fg(Color::Red));
        assert!(format!("{cell:?}").contains("symbol: \"a\""), "{cell:?}");
        assert_eq!(
            cell.style(),
            Style {
                fg: Some(Color::Red),
                bg: Some(Color::Reset),
                add_modifier: Modifier::empty(),
                sub_modifier: Modifier::empty(),
            }
        );
    }

    #[test]
    fn a_cell_holds_one_printable_grapheme_one_or_two_columns_wide() {
        // Only the first grapheme is kept (it used to print 3 columns for one cell).
        assert_eq!(Cell::new("abc").symbol(), "a");
        assert_eq!(Cell::new("日本").symbol(), "日");
        assert_eq!(Cell::new("e\u{301}x").symbol(), "e\u{301}");
        // A lone combining mark is zero-width: a space, not a cell printing 0 columns.
        assert_eq!(Cell::new("\u{301}").symbol(), " ");
        assert_eq!(Cell::new("\u{200b}").symbol(), " ");
        // A control character in the first grapheme, even after printable text in it.
        assert_eq!(Cell::new("\x1b").symbol(), " ");
        // Every cell prints exactly as many columns as it claims.
        for symbol in [
            "a", " ", "~", "é", "日", "😀", "abc", "\u{301}", "\x7f", "\t",
        ] {
            let cell = Cell::new(symbol);
            assert!(matches!(cell.width(), 1 | 2), "{symbol:?}: {cell:?}");
            assert_eq!(cell.width(), grapheme_width(cell.symbol()), "{symbol:?}");
        }
    }

    #[test]
    fn the_empty_string_is_a_space_not_a_continuation() {
        // An orphan continuation is skipped by the diff and the backend: what it covered
        // stayed on screen while the buffer believed it erased.
        assert!(!Cell::new("").is_continuation());
        assert_eq!(Cell::new("").symbol(), " ");
        let mut cell = Cell::new("x");
        cell.set_symbol("");
        assert!(!cell.is_continuation());
        assert_eq!(cell.symbol(), " ");
        // The previous "x" is rewritten as a space.
        let previous = Buffer::with_lines(["x"]);
        let mut next = previous.clone();
        next.get_mut(0, 0).set_symbol("");
        assert_eq!(next.diff(&previous).len(), 1);
    }

    #[test]
    fn write_grapheme_returns_the_written_width_and_normalizes() {
        let mut b = buf(4, 1);
        assert_eq!(b.write_grapheme(0, 0, "日", Style::new()), 2);
        assert!(b.get(1, 0).is_continuation());
        // Several graphemes: only the first one; an unprintable one: a space.
        assert_eq!(b.write_grapheme(2, 0, "ab", Style::new()), 1);
        assert_eq!(b.write_grapheme(3, 0, "\u{301}", Style::new()), 1);
        assert_eq!(b.lines(), ["日a "]);
    }

    #[test]
    fn overwriting_hand_made_wide_cells_at_the_edges_stays_in_the_buffer() {
        // `set_string` never puts a wide head in the last column nor a continuation in
        // the first one, but the `Cell` API can put a head there (and this module a
        // continuation): overwriting them must not reach outside.
        let mut b = buf(3, 1);
        b.get_mut(2, 0).set_symbol("日");
        b.set_string(2, 0, "x", Style::new());
        assert_eq!(b.lines(), ["  x"]);
        let mut c = buf(3, 1);
        c.get_mut(0, 0).symbol = Symbol::EMPTY;
        c.set_string(0, 0, "y", Style::new());
        assert_eq!(c.lines(), ["y  "]);
    }

    #[test]
    fn writes_ascii_and_returns_next_position() {
        let mut b = buf(6, 2);
        let next = b.set_string(1, 0, "abc", Style::new());
        assert_eq!(next, (4, 0));
        assert_eq!(b, Buffer::with_lines([" abc  ", "      "]));
    }

    #[test]
    fn writes_accented_text_and_combining_marks() {
        let mut b = buf(5, 1);
        b.set_string(0, 0, "été", Style::new());
        assert_eq!(b, Buffer::with_lines(["été  "]));
        // “e” + combining accent = one grapheme, one column.
        let mut c = buf(3, 1);
        c.set_string(0, 0, "e\u{301}x", Style::new());
        assert_eq!(c.get(0, 0).symbol(), "e\u{301}");
        assert_eq!(c.get(1, 0).symbol(), "x");
    }

    #[test]
    fn writes_wide_characters_with_continuation_cells() {
        let mut b = buf(6, 1);
        let next = b.set_string(0, 0, "a😀b日", Style::new());
        assert_eq!(next, (6, 0));
        assert_eq!(b.get(1, 0).symbol(), "😀");
        assert!(b.get(2, 0).is_continuation());
        assert_eq!(b.get(3, 0).symbol(), "b");
        assert_eq!(b.get(4, 0).symbol(), "日");
        assert!(b.get(5, 0).is_continuation());
        assert_eq!(b, Buffer::with_lines(["a😀b日"]));
        assert_eq!(b.lines(), ["a😀b日"]);
    }

    #[test]
    fn emoji_sequences_are_one_wide_grapheme() {
        let mut b = buf(4, 1);
        b.set_string(0, 0, "👍🏽🇫🇷", Style::new());
        assert_eq!(b.get(0, 0).symbol(), "👍🏽");
        assert!(b.get(1, 0).is_continuation());
        assert_eq!(b.get(2, 0).symbol(), "🇫🇷");
        assert!(b.get(3, 0).is_continuation());
    }

    #[test]
    fn wide_character_is_cut_at_the_edge_not_split() {
        // “a” fits; the emoji (2 columns) has only one column left: it is omitted.
        let mut b = buf(2, 1);
        let next = b.set_string(0, 0, "a😀", Style::new());
        assert_eq!(b, Buffer::with_lines(["a "]));
        assert_eq!(next, (1, 0));
        // Starting in the last column: nothing can be written.
        let mut c = buf(5, 1);
        c.set_string(4, 0, "日本", Style::new());
        assert_eq!(c, Buffer::with_lines(["     "]));
        // An area that is exactly wide enough.
        let mut d = buf(4, 1);
        d.set_string(0, 0, "日本语", Style::new());
        assert_eq!(d, Buffer::with_lines(["日本"]));
    }

    #[test]
    fn set_stringn_limits_the_width() {
        let mut b = buf(10, 1);
        b.set_stringn(0, 0, "abcdef", 3, Style::new());
        assert_eq!(b, Buffer::with_lines(["abc       "]));
        let mut c = buf(10, 1);
        c.set_stringn(0, 0, "日本語", 5, Style::new()); // 2 + 2, then the third character (2) does not fit
        assert_eq!(c, Buffer::with_lines(["日本      "]));
    }

    #[test]
    fn ignores_control_characters_and_out_of_range_positions() {
        let mut b = buf(4, 2);
        b.set_string(0, 0, "a\tb\nc", Style::new());
        assert_eq!(b.lines(), ["abc ", "    "]);
        assert_eq!(b.set_string(0, 5, "x", Style::new()), (0, 5));
        assert_eq!(b.set_string(9, 0, "x", Style::new()), (9, 0));
        assert_eq!(b.lines(), ["abc ", "    "]);
    }

    #[test]
    fn respects_a_buffer_that_does_not_start_at_the_origin() {
        let mut b = Buffer::empty(Rect::new(3, 2, 4, 2));
        b.set_string(4, 3, "ab", Style::new());
        assert_eq!(b.get(4, 3).symbol(), "a");
        assert_eq!(b.lines(), ["    ", " ab "]);
        assert_eq!(
            b.set_string(2, 2, "x", Style::new()),
            (2, 2),
            "left of the area"
        );
    }

    #[test]
    fn overwriting_half_of_a_wide_character_blanks_the_other_half() {
        // Right half overwritten: the head becomes a space.
        let mut b = buf(3, 1);
        b.set_string(0, 0, "😀", Style::new());
        b.set_string(1, 0, "x", Style::new());
        assert_eq!(b.lines(), [" x "]);
        // Left half overwritten: the continuation becomes a space.
        let mut c = buf(3, 1);
        c.set_string(0, 0, "😀", Style::new());
        c.set_string(0, 0, "x", Style::new());
        assert_eq!(c.lines(), ["x  "]);
        // A wide character straddling two others destroys both.
        let mut d = buf(4, 1);
        d.set_string(0, 0, "日本", Style::new());
        d.set_string(1, 0, "😀", Style::new());
        assert_eq!(d.lines(), [" 😀 "]);
        assert!(d.get(2, 0).is_continuation());
    }

    #[test]
    fn set_string_patches_style_and_keeps_the_rest() {
        let mut b = buf(3, 1);
        b.set_style(b.area, Style::new().bg(Color::Blue));
        b.set_string(
            0,
            0,
            "ab",
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        );
        let a = b.get(0, 0);
        assert_eq!(
            (a.fg, a.bg, a.modifier),
            (Color::Red, Color::Blue, Modifier::BOLD)
        );
        assert_eq!(b.get(2, 0).bg, Color::Blue);
        assert_eq!(b.get(2, 0).fg, Color::Reset);
    }

    #[test]
    fn wide_character_continuation_shares_the_style() {
        let mut b = buf(2, 1);
        b.set_string(0, 0, "😀", Style::new().fg(Color::Green).bg(Color::Black));
        assert_eq!(b.get(1, 0).fg, Color::Green);
        assert_eq!(b.get(1, 0).bg, Color::Black);
    }

    #[test]
    fn set_style_is_clipped_to_the_buffer() {
        let mut b = buf(2, 2);
        b.set_style(Rect::new(1, 1, 10, 10), Style::new().fg(Color::Cyan));
        assert_eq!(b.get(1, 1).fg, Color::Cyan);
        assert_eq!(b.get(0, 0).fg, Color::Reset);
    }

    #[test]
    fn reset_clears_symbols_and_styles() {
        let mut b = buf(3, 1);
        b.set_string(0, 0, "abc", Style::new().fg(Color::Red));
        b.reset();
        assert_eq!(b, buf(3, 1));
    }

    #[test]
    fn with_lines_pads_short_lines_and_measures_display_width() {
        let b = Buffer::with_lines(["ab", "日本x", ""]);
        assert_eq!(b.area, Rect::new(0, 0, 5, 3));
        assert_eq!(b.lines(), ["ab   ", "日本x", "     "]);
        assert_eq!(
            Buffer::with_lines(Vec::<&str>::new()).area,
            Rect::new(0, 0, 0, 0)
        );
    }

    #[test]
    fn index_and_position_round_trip() {
        let b = Buffer::empty(Rect::new(2, 3, 4, 2));
        let i = b.index_of(5, 4);
        assert_eq!(i, 7);
        assert_eq!(b.pos_of(i), (5, 4));
    }

    #[test]
    #[should_panic(expected = "outside the buffer")]
    fn get_outside_the_buffer_panics() {
        let _ = buf(2, 2).get(2, 0);
    }

    fn diff_positions(next: &Buffer, prev: &Buffer) -> Vec<(u16, u16, String)> {
        next.diff(prev)
            .into_iter()
            .map(|(x, y, c)| (x, y, c.symbol().to_string()))
            .collect()
    }

    #[test]
    fn diff_of_identical_buffers_is_empty() {
        let a = Buffer::with_lines(["abc", "日本"]);
        assert_eq!(a.diff(&a.clone()), []);
    }

    #[test]
    fn diff_reports_changed_cells_in_screen_order() {
        let prev = Buffer::with_lines(["abc", "def"]);
        let next = Buffer::with_lines(["aXc", "deZ"]);
        assert_eq!(
            diff_positions(&next, &prev),
            [(1, 0, "X".to_string()), (2, 1, "Z".to_string())]
        );
    }

    #[test]
    fn diff_detects_style_only_changes() {
        let prev = Buffer::with_lines(["ab"]);
        let mut next = prev.clone();
        next.set_style(Rect::new(1, 0, 1, 1), Style::new().fg(Color::Red));
        assert_eq!(diff_positions(&next, &prev), [(1, 0, "b".to_string())]);
    }

    #[test]
    fn normalize_wide_turns_broken_pairs_into_spaces() {
        let red = Style::new().fg(Color::Red);
        let mut buf = Buffer::with_lines(["ab😀cd界e"]);
        let continuation = buf.get(3, 0).clone();
        // A wide head set in place before an ordinary cell: no continuation.
        buf.get_mut(0, 0).set_symbol("界").set_style(red);
        // A continuation copied after an ordinary cell, and one in the first column.
        *buf.get_mut(5, 0) = continuation.clone();
        let mut row2 = Buffer::with_lines(["xy"]);
        *row2.get_mut(0, 0) = continuation;
        // In the last column (8): a wide head that does not fit.
        let last = buf.area().width - 1;
        buf.get_mut(last, 0).set_symbol("界");

        buf.normalize_wide();
        row2.normalize_wide();
        let symbols: Vec<&str> = buf.content().iter().map(Cell::symbol).collect();
        assert_eq!(symbols, [" ", "b", "😀", "", "c", " ", "界", "", " "]);
        assert_eq!(buf.get(0, 0).fg, Color::Red, "the style is kept");
        assert_eq!(row2.get(0, 0).symbol(), " ");
    }

    #[test]
    fn normalize_wide_keeps_a_buffer_written_by_set_string() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 7, 2));
        buf.set_string(0, 0, "a😀界b", Style::new());
        buf.set_string(1, 1, "界x😀", Style::new());
        let before = buf.clone();
        buf.normalize_wide();
        assert_eq!(buf, before);
    }

    #[test]
    fn diff_never_returns_continuation_cells() {
        let prev = Buffer::with_lines(["ab "]);
        let next = Buffer::with_lines(["😀 "]);
        // The head changes; the continuation (column 1) is covered by the head.
        assert_eq!(diff_positions(&next, &prev), [(0, 0, "😀".to_string())]);
    }

    #[test]
    fn diff_wide_replaced_by_narrow_redraws_the_freed_cell() {
        // “😀” (columns 0-1) replaced by “a” + space: column 1 must be redrawn,
        // otherwise the right half of the emoji would remain visible.
        let prev = Buffer::with_lines(["😀x"]);
        let mut next = prev.clone();
        next.set_string(0, 0, "a", Style::new());
        assert_eq!(
            diff_positions(&next, &prev),
            [(0, 0, "a".to_string()), (1, 0, " ".to_string())]
        );
    }

    #[test]
    fn diff_narrow_replaced_by_wide_and_wide_replaced_by_wide() {
        let prev = Buffer::with_lines(["abx"]);
        let mut next = prev.clone();
        next.set_string(0, 0, "日", Style::new());
        assert_eq!(diff_positions(&next, &prev), [(0, 0, "日".to_string())]);

        let wide_prev = Buffer::with_lines(["日x"]);
        let mut wide_next = wide_prev.clone();
        wide_next.set_string(0, 0, "本", Style::new());
        assert_eq!(
            diff_positions(&wide_next, &wide_prev),
            [(0, 0, "本".to_string())]
        );
    }

    #[test]
    fn diff_after_shifting_a_wide_character_by_one_column() {
        // “日” in columns 0-1 becomes “日” in columns 1-2.
        let prev = Buffer::with_lines(["日 "]);
        let mut next = buf(3, 1);
        next.set_string(1, 0, "日", Style::new());
        assert_eq!(
            diff_positions(&next, &prev),
            [(0, 0, " ".to_string()), (1, 0, "日".to_string())]
        );
    }

    #[test]
    fn filled_with_a_wide_cell_writes_head_and_continuation_pairs() {
        let buf = Buffer::filled(Rect::new(0, 0, 4, 1), Cell::new("界"));
        let symbols: Vec<&str> = buf.content().iter().map(Cell::symbol).collect();
        assert_eq!(symbols, ["界", "", "界", ""]);
        assert!(buf.content()[1].is_continuation());
        // Odd width: the last column cannot hold a wide character.
        let odd = Buffer::filled(Rect::new(0, 0, 3, 2), Cell::new("界"));
        assert_eq!(odd.lines(), ["界 ", "界 "]);
        // The diff never writes a wide character in the last column.
        let empty = Buffer::empty(odd.area());
        for (x, _, cell) in odd.diff_iter(&empty) {
            assert!(x + u16::try_from(cell.width()).unwrap() <= odd.area().right());
        }
    }

    #[test]
    fn a_literal_rect_past_the_coordinate_range_is_clipped() {
        let area = Rect {
            x: 65534,
            y: 0,
            width: 3,
            height: 1,
        };
        let empty = Buffer::empty(area);
        let filled = Buffer::filled(area, Cell::new("x"));
        assert_eq!(empty.area(), Rect::new(65534, 0, 1, 1));
        assert_eq!(filled.diff(&empty).len(), 1);
        let mut reused = Buffer::empty(Rect::new(0, 0, 2, 2));
        reused.reset_to(Rect {
            x: 0,
            y: 65535,
            width: 2,
            height: 4,
        });
        assert_eq!(reused.area(), Rect::new(0, 65535, 2, 0));
        assert_eq!(reused.content().len(), 0);
    }

    #[test]
    #[should_panic(expected = "different areas")]
    fn diff_requires_same_area() {
        let _ = buf(2, 1).diff(&buf(3, 1));
    }

    #[test]
    fn clone_from_copies_area_and_cells_into_the_kept_memory() {
        let source = Buffer::with_lines(["ab", "cd"]);
        let mut copy = Buffer::empty(Rect::new(0, 0, 10, 10));
        let capacity = copy.content.capacity();
        copy.clone_from(&source);
        assert_eq!(copy, source);
        assert_eq!(copy.content.capacity(), capacity, "memory not reused");
        assert_eq!(source.clone(), source);
    }

    #[test]
    fn accessors_expose_the_area_and_the_cells() {
        let mut buf = Buffer::empty(Rect::new(1, 2, 3, 1));
        assert_eq!(buf.area(), Rect::new(1, 2, 3, 1));
        assert_eq!(buf.content().len(), 3);
        buf.content_mut()[1].set_symbol("x");
        assert_eq!(buf.lines(), [" x "]);
    }
}
