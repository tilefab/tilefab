//! Colors, modifiers, and styles.
//!
//! A [`Style`] is a *partial description* (“make the text red, leave the
//! background alone”); a buffer cell holds *complete* values (see `Cell`).
//! Combine styles with [`Style::patch`].

use std::ops::{BitOr, BitOrAssign, Sub};

/// A terminal color.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// Default terminal color (foreground or background).
    #[default]
    Reset,
    /// Black (ANSI color 0).
    Black,
    /// Red (ANSI color 1).
    Red,
    /// Green (ANSI color 2).
    Green,
    /// Yellow (ANSI color 3).
    Yellow,
    /// Blue (ANSI color 4).
    Blue,
    /// Magenta (ANSI color 5).
    Magenta,
    /// Cyan (ANSI color 6).
    Cyan,
    /// Light gray (ANSI “white”, color 7).
    Gray,
    /// Dark gray (ANSI “bright black”, color 8).
    DarkGray,
    /// Bright red (ANSI color 9).
    LightRed,
    /// Bright green (ANSI color 10).
    LightGreen,
    /// Bright yellow (ANSI color 11).
    LightYellow,
    /// Bright blue (ANSI color 12).
    LightBlue,
    /// Bright magenta (ANSI color 13).
    LightMagenta,
    /// Bright cyan (ANSI color 14).
    LightCyan,
    /// White (ANSI “bright white”, color 15).
    White,
    /// Color from the 256-color palette.
    Indexed(u8),
    /// True color (24-bit).
    Rgb(u8, u8, u8),
}

/// Set of text modifiers (bold, italic…), stored in a `u16`.
///
/// A small homemade “bitflags”: one constant per modifier, combined with `|`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Modifier(u16);

impl Modifier {
    /// Bold.
    pub const BOLD: Self = Self(1 << 0);
    /// Dim.
    pub const DIM: Self = Self(1 << 1);
    /// Italic.
    pub const ITALIC: Self = Self(1 << 2);
    /// Underlined.
    pub const UNDERLINED: Self = Self(1 << 3);
    /// Slow blink.
    pub const SLOW_BLINK: Self = Self(1 << 4);
    /// Rapid blink.
    pub const RAPID_BLINK: Self = Self(1 << 5);
    /// Reversed colors (foreground and background).
    pub const REVERSED: Self = Self(1 << 6);
    /// Hidden text.
    pub const HIDDEN: Self = Self(1 << 7);
    /// Crossed out.
    pub const CROSSED_OUT: Self = Self(1 << 8);

    /// No modifiers.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// All modifiers.
    #[must_use]
    pub const fn all() -> Self {
        Self((1 << 9) - 1)
    }

    /// True if no modifiers are present.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Raw bits (useful for tests and debugging).
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// True if all modifiers in `other` are present.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Add the modifiers in `other`.
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    /// Remove the modifiers in `other`.
    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    /// Iterate over the modifiers present, one at a time.
    pub fn iter(self) -> impl Iterator<Item = Self> {
        (0..9)
            .map(|bit| Self(1 << bit))
            .filter(move |flag| self.contains(*flag))
    }
}

impl BitOr for Modifier {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Modifier {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Set difference: `a - b` = modifiers in `a` that are not in `b`.
impl Sub for Modifier {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 & !rhs.0)
    }
}

/// A style: optional colors and modifiers to add / remove.
///
/// `None` means “leave unchanged”: applying `fg: None` to a cell preserves its
/// foreground color. `add_modifier` and `sub_modifier` serve the same purpose for modifiers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    /// Foreground color; `None` = leave unchanged.
    pub fg: Option<Color>,
    /// Background color; `None` = leave unchanged.
    pub bg: Option<Color>,
    /// Modifiers to enable.
    pub add_modifier: Modifier,
    /// Modifiers to disable (applied before `add_modifier`).
    pub sub_modifier: Modifier,
}

impl Style {
    /// Empty style: has no effect when applied.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fg: None,
            bg: None,
            add_modifier: Modifier::empty(),
            sub_modifier: Modifier::empty(),
        }
    }

    /// Set the foreground color.
    #[must_use]
    pub const fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    /// Set the background color.
    #[must_use]
    pub const fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// Add modifiers (and cancel any removal of the same modifiers).
    #[must_use]
    pub fn add_modifier(mut self, modifier: Modifier) -> Self {
        self.sub_modifier.remove(modifier);
        self.add_modifier.insert(modifier);
        self
    }

    /// Remove modifiers (and cancel any addition of the same modifiers).
    #[must_use]
    pub fn remove_modifier(mut self, modifier: Modifier) -> Self {
        self.add_modifier.remove(modifier);
        self.sub_modifier.insert(modifier);
        self
    }

    /// Overlay `other` on `self`: fields set by `other` take precedence; the rest is preserved.
    ///
    /// ```
    /// use tilefab_core::{Color, Modifier, Style};
    /// let base = Style::new().fg(Color::Red).add_modifier(Modifier::BOLD);
    /// let over = Style::new().bg(Color::Blue).remove_modifier(Modifier::BOLD);
    /// let both = base.patch(over);
    /// assert_eq!(both.fg, Some(Color::Red));
    /// assert_eq!(both.bg, Some(Color::Blue));
    /// assert!(!both.add_modifier.contains(Modifier::BOLD));
    /// ```
    #[must_use]
    pub fn patch(mut self, other: Style) -> Self {
        self.fg = other.fg.or(self.fg);
        self.bg = other.bg.or(self.bg);
        // Additions in `other` cancel removals in `self`, and vice versa.
        self.add_modifier.remove(other.sub_modifier);
        self.add_modifier.insert(other.add_modifier);
        self.sub_modifier.remove(other.add_modifier);
        self.sub_modifier.insert(other.sub_modifier);
        self
    }
}

impl From<Color> for Style {
    /// A color alone is a foreground color.
    fn from(color: Color) -> Self {
        Style::new().fg(color)
    }
}

impl From<Modifier> for Style {
    fn from(modifier: Modifier) -> Self {
        Style::new().add_modifier(modifier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_set_operations() {
        let mut m = Modifier::BOLD | Modifier::ITALIC;
        assert!(m.contains(Modifier::BOLD));
        assert!(m.contains(Modifier::BOLD | Modifier::ITALIC));
        assert!(!m.contains(Modifier::DIM));
        m.remove(Modifier::BOLD);
        assert_eq!(m, Modifier::ITALIC);
        assert_eq!(Modifier::all() - Modifier::all(), Modifier::empty());
        assert!(Modifier::empty().is_empty());
        assert_eq!(
            (Modifier::BOLD | Modifier::CROSSED_OUT).iter().count(),
            2,
            "iter() lists every modifier present"
        );
        assert_eq!(Modifier::all().iter().count(), 9);
    }

    #[test]
    fn patch_overrides_only_what_is_defined() {
        let base = Style::new().fg(Color::Red).bg(Color::Black);
        let patched = base.patch(Style::new().fg(Color::Green));
        assert_eq!(patched.fg, Some(Color::Green));
        assert_eq!(patched.bg, Some(Color::Black), "the background is kept");
    }

    #[test]
    fn patch_add_and_sub_modifiers_cancel_each_other() {
        let bold = Style::new().add_modifier(Modifier::BOLD | Modifier::ITALIC);
        let no_bold = Style::new().remove_modifier(Modifier::BOLD);

        let patched = bold.patch(no_bold);
        assert_eq!(patched.add_modifier, Modifier::ITALIC);
        assert_eq!(patched.sub_modifier, Modifier::BOLD);

        // And in the other direction: adding cancels removing.
        let back = patched.patch(Style::new().add_modifier(Modifier::BOLD));
        assert_eq!(back.add_modifier, Modifier::BOLD | Modifier::ITALIC);
        assert_eq!(back.sub_modifier, Modifier::empty());
    }

    #[test]
    fn patch_with_empty_style_is_identity() {
        let s = Style::new()
            .fg(Color::Rgb(1, 2, 3))
            .add_modifier(Modifier::DIM)
            .remove_modifier(Modifier::BOLD);
        assert_eq!(s.patch(Style::new()), s);
        assert_eq!(Style::new().patch(s), s);
    }

    #[test]
    fn builders_are_exclusive_per_modifier() {
        let s = Style::new()
            .add_modifier(Modifier::BOLD)
            .remove_modifier(Modifier::BOLD);
        assert_eq!(s.add_modifier, Modifier::empty());
        assert_eq!(s.sub_modifier, Modifier::BOLD);
    }
}
