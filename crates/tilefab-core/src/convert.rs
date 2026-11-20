//! Integer conversions shared by the library, the application and the examples.

/// Convert a `usize` (a length, a count, a display width) to `u16`, saturating at
/// `u16::MAX`.
///
/// Screen coordinates are `u16`: a larger value cannot be displayed anyway, so
/// clamping it is the intended behavior (a plain `as u16` would wrap around instead).
///
/// ```
/// use tilefab_core::convert::saturating_u16;
///
/// assert_eq!(saturating_u16(42), 42);
/// assert_eq!(saturating_u16(70_000), u16::MAX);
/// ```
#[must_use]
pub fn saturating_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}
