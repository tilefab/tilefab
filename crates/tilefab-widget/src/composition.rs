//! Composition parser: the string that lists the components of a [`LineWidget`] in display
//! order, for instance `"B0 T2 B1 B2 S B2 L"`.
//!
//! Grammar: tokens separated by exactly one space (U+0020), no leading or trailing space and
//! no other whitespace. A token is an upper-case letter followed by an optional parameter:
//!
//! | Token | Parameter | Range | Without parameter |
//! |---|---|---|---|
//! | `S` | symbol index, 1 digit | 0–9 | dynamic |
//! | `B` | barrier index, 1 digit | 0–9 | required |
//! | `D` | date format, 1 digit | 0–2 | required |
//! | `T` | time format, 1 digit | 0–5 | required |
//! | `P` | size, 1 digit | 3–9 | `P3` |
//! | `K` | keyword index, 1 digit | 0–9 | dynamic |
//! | `N` | name index, 1–2 digits | 0–99 | dynamic |
//! | `L` | size, 1–3 digits | 2–999 | elastic |
//! | `V` | size, 1–3 digits, then optional `U`, then optional `/` | 1–999 | size 5 |
//! | `I` | size, 1–3 digits, then optional look letter (see below) | 2–999 | elastic |
//!
//! `D`, `T`, `P`, `K`, `L`, `V` and `I` are unique; `S`, `B` and `N` can be repeated.
//!
//! A token may end with a *paint*, which changes the style of that component only: a `.`, then
//! at least one of, in this order, a foreground color, `:` and a background color, `*` for
//! bold (`B1.R`, `N.W+:B*`, `P.*`, `L.:B`; `S.` and `S.*R` are refused). Colors: the eight
//! ANSI colors, `K` black, `R` red, `G` green, `Y` yellow, `B` blue, `M` magenta, `C` cyan,
//! `W` white (ANSI color 7, a light gray: [`Color::Gray`]), `+` for the bright version (`K+`
//! dark gray, `W+` bright white: [`Color::White`]). `I` and `V` take a second part after `/`,
//! with the same rule, for their todo cells and their units (`I.C/K+`, `V5U/.W/K+`); other
//! components take only one.
//!
//! The look of `I` picks its two characters: `B` (bar) `█`/`░`, `L` (line) `━`/`─`, `D` (dots)
//! `●`/`·`, `S` (squares) `■`/`□`, `O` (circles) `●`/`○`, `R` (diamonds) `◆`/`◇`, `G`
//! (segments) `▰`/`▱`, `H` (shaded blocks) `▓`/`░`, `V` (vertical bars) `┃`/`│`, `A` (ASCII)
//! `#`/`-`, `C` (chevrons) `>`/`·`. Without a look, `I` uses the characters given to the
//! builder, or else the bar.
//!
//! [`LineWidget`]: crate::LineWidget

use std::fmt;

use tilefab_core::{Color, Modifier, Style};

use crate::builder::BuildError;

/// Default size of `P` (`P` alone is `P3`).
pub(crate) const DEFAULT_PROGRESS_SIZE: u8 = 3;
/// Default size of `V` (`V` alone is `V5`: `500.0`, or up to `99999` without decimals).
pub(crate) const DEFAULT_VALUE_SIZE: u16 = 5;

/// One component of a composition, with its parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Token {
    /// `S`: a symbol; `Some(index)` for a fixed index, `None` for a dynamic one.
    Symbol(Option<u8>),
    /// `B`: a barrier, always a fixed index.
    Barrier(u8),
    /// `D`: the date, format 0–2.
    Date(u8),
    /// `T`: the time, format 0–5.
    Time(u8),
    /// `P`: the progress number, size 3–9.
    Progress(u8),
    /// `K`: a keyword; `Some(index)` for a fixed index, `None` for a dynamic one.
    Keyword(Option<u8>),
    /// `N`: a name; `Some(index)` (0–99) for a fixed index, `None` for a dynamic one.
    Name(Option<u8>),
    /// `L`: the log text; `Some(size)` (2–999) for a fixed size, `None` when elastic.
    Log(Option<u16>),
    /// `V`: a value of `size` columns (1–999), with a unit (`U`) and a total (`/`).
    Value {
        /// Maximum number of columns of each number.
        size: u16,
        /// `U`: the numbers are followed by a unit.
        unit: bool,
        /// `/`: the value is followed by a total.
        total: bool,
    },
    /// `I`: the illustration.
    Illustration {
        /// `Some(size)` (2–999) for a fixed size, `None` when elastic.
        size: Option<u16>,
        /// The look given after the size, if any.
        look: Option<Look>,
    },
}

/// The look of `I`, the letter after its size: a pair of characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Look {
    /// `B` (bar): `█` / `░`.
    Bar,
    /// `L` (line): `━` / `─`.
    Line,
    /// `D` (dots): `●` / `·`.
    Dot,
    /// `S` (squares): `■` / `□`.
    Square,
    /// `O` (circles): `●` / `○`.
    Circle,
    /// `R` (diamonds): `◆` / `◇`.
    Diamond,
    /// `G` (segments): `▰` / `▱`.
    Segment,
    /// `H` (shaded blocks): `▓` / `░`.
    Shade,
    /// `V` (vertical bars): `┃` / `│`.
    Vertical,
    /// `A` (ASCII): `#` / `-`.
    Ascii,
    /// `C` (chevrons): `>` / `·`.
    Chevron,
}

impl Look {
    /// Every look.
    pub(crate) const ALL: [Self; 11] = [
        Self::Bar,
        Self::Line,
        Self::Dot,
        Self::Square,
        Self::Circle,
        Self::Diamond,
        Self::Segment,
        Self::Shade,
        Self::Vertical,
        Self::Ascii,
        Self::Chevron,
    ];

    /// The look written `letter` after the size of `I`.
    fn from_letter(letter: char) -> Option<Self> {
        Self::ALL.into_iter().find(|look| look.letter() == letter)
    }

    /// The letter of the look.
    pub(crate) fn letter(self) -> char {
        match self {
            Self::Bar => 'B',
            Self::Line => 'L',
            Self::Dot => 'D',
            Self::Square => 'S',
            Self::Circle => 'O',
            Self::Diamond => 'R',
            Self::Segment => 'G',
            Self::Shade => 'H',
            Self::Vertical => 'V',
            Self::Ascii => 'A',
            Self::Chevron => 'C',
        }
    }

    /// The *done* and *todo* characters of the look, each 1 column wide.
    pub(crate) fn characters(self) -> (&'static str, &'static str) {
        match self {
            Self::Bar => ("█", "░"),
            Self::Line => ("━", "─"),
            Self::Dot => ("●", "·"),
            Self::Square => ("■", "□"),
            Self::Circle => ("●", "○"),
            Self::Diamond => ("◆", "◇"),
            Self::Segment => ("▰", "▱"),
            Self::Shade => ("▓", "░"),
            Self::Vertical => ("┃", "│"),
            Self::Ascii => ("#", "-"),
            Self::Chevron => (">", "·"),
        }
    }
}

impl Token {
    /// The letter of the token.
    pub(crate) fn letter(self) -> char {
        match self {
            Self::Symbol(_) => 'S',
            Self::Barrier(_) => 'B',
            Self::Date(_) => 'D',
            Self::Time(_) => 'T',
            Self::Progress(_) => 'P',
            Self::Keyword(_) => 'K',
            Self::Name(_) => 'N',
            Self::Log(_) => 'L',
            Self::Value { .. } => 'V',
            Self::Illustration { .. } => 'I',
        }
    }

    /// Whether the component can appear only once in a composition.
    fn is_unique(self) -> bool {
        !matches!(self, Self::Symbol(_) | Self::Barrier(_) | Self::Name(_))
    }
}

/// The canonical text of the token: parsing it gives the same token back.
impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{}", self.letter()))?;
        match *self {
            Self::Symbol(index) | Self::Keyword(index) | Self::Name(index) => {
                if let Some(index) = index {
                    write!(f, "{index}")?;
                }
            }
            Self::Barrier(n) | Self::Date(n) | Self::Time(n) | Self::Progress(n) => {
                write!(f, "{n}")?;
            }
            Self::Log(size) => {
                if let Some(size) = size {
                    write!(f, "{size}")?;
                }
            }
            Self::Illustration { size, look } => {
                if let Some(size) = size {
                    write!(f, "{size}")?;
                }
                if let Some(look) = look {
                    write!(f, "{}", look.letter())?;
                }
            }
            Self::Value { size, unit, total } => {
                write!(f, "{size}")?;
                if unit {
                    f.write_str("U")?;
                }
                if total {
                    f.write_str("/")?;
                }
            }
        }
        Ok(())
    }
}

/// The colors of a paint: letter, normal color, bright color (with `+`). The letters follow
/// the ANSI names: `W`, ANSI "white", is color 7, a light gray (`Color::Gray`).
const COLORS: [(char, Color, Color); 8] = [
    ('K', Color::Black, Color::DarkGray),
    ('R', Color::Red, Color::LightRed),
    ('G', Color::Green, Color::LightGreen),
    ('Y', Color::Yellow, Color::LightYellow),
    ('B', Color::Blue, Color::LightBlue),
    ('M', Color::Magenta, Color::LightMagenta),
    ('C', Color::Cyan, Color::LightCyan),
    ('W', Color::Gray, Color::White),
];

/// The paint of a token: the style laid over the styles given to the builder, for that
/// component only. The default paint changes nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Paint {
    /// The first part: the whole component, or the done cells of `I` and the numbers of `V`.
    pub(crate) first: Style,
    /// The second part (`I` and `V` only): the todo cells, the units.
    pub(crate) second: Option<Style>,
}

impl Paint {
    /// The style of the second part: the first one when the paint has a single part.
    pub(crate) fn second(self) -> Style {
        self.second.unwrap_or(self.first)
    }
}

/// A token and its paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Painted {
    pub(crate) token: Token,
    pub(crate) paint: Paint,
}

/// The canonical text of the token and its paint.
impl fmt::Display for Painted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.token)?;
        if self.paint == Paint::default() {
            return Ok(());
        }
        f.write_str(".")?;
        write_part(f, self.paint.first)?;
        if let Some(second) = self.paint.second {
            f.write_str("/")?;
            write_part(f, second)?;
        }
        Ok(())
    }
}

/// Write one part of a paint: color, `:` and background, `*`.
fn write_part(f: &mut fmt::Formatter<'_>, style: Style) -> fmt::Result {
    let color = |f: &mut fmt::Formatter<'_>, color: Option<Color>| -> fmt::Result {
        for (letter, normal, bright) in COLORS {
            if color == Some(normal) {
                return write!(f, "{letter}");
            }
            if color == Some(bright) {
                return write!(f, "{letter}+");
            }
        }
        Ok(())
    };
    color(f, style.fg)?;
    if style.bg.is_some() {
        f.write_str(":")?;
        color(f, style.bg)?;
    }
    if style.add_modifier.contains(Modifier::BOLD) {
        f.write_str("*")?;
    }
    Ok(())
}

/// Parse a composition into its tokens, in display order.
///
/// # Errors
///
/// A [`BuildError`] for an empty composition, bad spacing, an unknown token, a bad
/// parameter (missing, out of range, too many digits, `P1` / `P2`, an unknown look as in
/// `IX`), a bad paint ([`TokenProblem::BadPaint`]: empty part, unknown color, wrong order,
/// more than two parts), a second part of paint on a component other than `I` and `V`
/// ([`TokenProblem::SecondPart`]) or a unique component repeated.
pub(crate) fn parse(composition: &str) -> Result<Vec<Painted>, BuildError> {
    if composition.is_empty() {
        return Err(BuildError::EmptyComposition);
    }
    if let Some((offset, _)) = composition
        .char_indices()
        .find(|&(_, c)| c.is_whitespace() && c != ' ')
    {
        return Err(BuildError::BadSpacing { offset });
    }
    let mut tokens: Vec<Painted> = Vec::new();
    let mut offset = 0;
    for (index, text) in composition.split(' ').enumerate() {
        if text.is_empty() {
            // A leading, trailing or double space.
            return Err(BuildError::BadSpacing { offset });
        }
        let painted = parse_painted(text).map_err(|problem| BuildError::BadToken {
            position: index + 1,
            token: text.to_owned(),
            problem,
        })?;
        let token = painted.token;
        if token.is_unique() && tokens.iter().any(|t| t.token.letter() == token.letter()) {
            return Err(BuildError::RepeatedComponent {
                position: index + 1,
                token: text.to_owned(),
            });
        }
        tokens.push(painted);
        offset += text.len() + 1;
    }
    Ok(tokens)
}

/// What is wrong with one token of a composition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenProblem {
    /// Not one of the letters of the grammar (lower case included).
    UnknownComponent,
    /// `B`, `D` and `T` need a parameter.
    MissingParameter,
    /// The parameter is not made of digits as the grammar says (`SX`, `VU5`…).
    BadParameter,
    /// More digits than the grammar allows (`S12`, `L1000`).
    TooManyDigits,
    /// The parameter is outside its range (`D3`, `L1`).
    OutOfRange,
    /// `P1` and `P2` cannot show `100`.
    ProgressTooSmall,
    /// The paint after `.` does not follow its grammar (`S.`, `S.X`, `S.*R`).
    BadPaint,
    /// A second part of paint (`/`) on a component other than `I` and `V`.
    SecondPart,
}

impl fmt::Display for TokenProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownComponent => "unknown component",
            Self::MissingParameter => "missing parameter",
            Self::BadParameter => "bad parameter",
            Self::TooManyDigits => "too many digits",
            Self::OutOfRange => "parameter out of range",
            Self::ProgressTooSmall => "P needs at least 3 columns to show 100",
            Self::BadPaint => "bad paint (.[color][:color][*], colors K R G Y B M C W, + bright)",
            Self::SecondPart => "only I and V take a second part of paint after /",
        })
    }
}

/// Parse one token and its paint, after the first `.` (never empty).
fn parse_painted(text: &str) -> Result<Painted, TokenProblem> {
    let (head, paint) = match text.split_once('.') {
        Some((head, paint)) => (head, Some(paint)),
        None => (text, None),
    };
    let token = parse_token(head)?;
    let paint = match paint {
        None => Paint::default(),
        Some(paint) => {
            let mut parts = paint.split('/');
            let first = parse_part(parts.next().unwrap_or(""))?;
            let second = parts.next().map(parse_part).transpose()?;
            if parts.next().is_some() {
                return Err(TokenProblem::BadPaint);
            }
            if second.is_some()
                && !matches!(token, Token::Illustration { .. } | Token::Value { .. })
            {
                return Err(TokenProblem::SecondPart);
            }
            Paint { first, second }
        }
    };
    Ok(Painted { token, paint })
}

/// One part of a paint: `[color][:color][*]`, not empty.
fn parse_part(text: &str) -> Result<Style, TokenProblem> {
    if text.is_empty() {
        return Err(TokenProblem::BadPaint);
    }
    let mut rest = text;
    let mut style = Style::new();
    if let Some((color, after)) = parse_color(rest) {
        style = style.fg(color);
        rest = after;
    }
    if let Some(after) = rest.strip_prefix(':') {
        let (color, after) = parse_color(after).ok_or(TokenProblem::BadPaint)?;
        style = style.bg(color);
        rest = after;
    }
    if let Some(after) = rest.strip_prefix('*') {
        style = style.add_modifier(Modifier::BOLD);
        rest = after;
    }
    if rest.is_empty() {
        Ok(style)
    } else {
        Err(TokenProblem::BadPaint)
    }
}

/// A color letter and its optional `+` at the start of `text`, and what follows them.
fn parse_color(text: &str) -> Option<(Color, &str)> {
    let mut chars = text.chars();
    let letter = chars.next()?;
    let (_, normal, bright) = COLORS.into_iter().find(|&(l, _, _)| l == letter)?;
    let rest = chars.as_str();
    Some(match rest.strip_prefix('+') {
        Some(rest) => (bright, rest),
        None => (normal, rest),
    })
}

/// Parse one token (never empty).
fn parse_token(text: &str) -> Result<Token, TokenProblem> {
    let mut chars = text.chars();
    let letter = chars.next().unwrap_or(' ');
    let rest = chars.as_str();
    match letter {
        'S' => optional(rest, 1, 0, 9).map(|i| Token::Symbol(i.map(narrow))),
        'K' => optional(rest, 1, 0, 9).map(|i| Token::Keyword(i.map(narrow))),
        'N' => optional(rest, 2, 0, 99).map(|i| Token::Name(i.map(narrow))),
        'B' => required(rest, 1, 0, 9).map(|i| Token::Barrier(narrow(i))),
        'D' => required(rest, 1, 0, 2).map(|i| Token::Date(narrow(i))),
        'T' => required(rest, 1, 0, 5).map(|i| Token::Time(narrow(i))),
        'P' => match optional(rest, 1, 0, 9)? {
            None => Ok(Token::Progress(DEFAULT_PROGRESS_SIZE)),
            Some(1 | 2) => Err(TokenProblem::ProgressTooSmall),
            Some(0) => Err(TokenProblem::OutOfRange),
            Some(size) => Ok(Token::Progress(narrow(size))),
        },
        'L' => optional(rest, 3, 2, 999).map(Token::Log),
        'I' => parse_illustration(rest),
        'V' => parse_value(rest),
        _ => Err(TokenProblem::UnknownComponent),
    }
}

/// Narrow a parameter already checked to fit (at most 99) to a `u8`.
fn narrow(value: u16) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

/// A parameter that must be present.
fn required(rest: &str, max_digits: usize, min: u16, max: u16) -> Result<u16, TokenProblem> {
    optional(rest, max_digits, min, max)?.ok_or(TokenProblem::MissingParameter)
}

/// An optional parameter: `None` when `rest` is empty.
fn optional(
    rest: &str,
    max_digits: usize,
    min: u16,
    max: u16,
) -> Result<Option<u16>, TokenProblem> {
    if rest.is_empty() {
        return Ok(None);
    }
    number(rest, max_digits, min, max).map(Some)
}

/// A number made only of ASCII digits, with at most `max_digits` digits, in `min..=max`.
fn number(digits: &str, max_digits: usize, min: u16, max: u16) -> Result<u16, TokenProblem> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(TokenProblem::BadParameter);
    }
    if digits.len() > max_digits {
        return Err(TokenProblem::TooManyDigits);
    }
    // At most 3 digits: always fits.
    let value: u16 = digits.parse().map_err(|_| TokenProblem::BadParameter)?;
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(TokenProblem::OutOfRange)
    }
}

/// The parameter of `I`: optional size, then optional look.
fn parse_illustration(rest: &str) -> Result<Token, TokenProblem> {
    let look = rest.chars().next_back().and_then(Look::from_letter);
    let rest = if look.is_some() {
        &rest[..rest.len() - 1]
    } else {
        rest
    };
    let size = optional(rest, 3, 2, 999)?;
    Ok(Token::Illustration { size, look })
}

/// The parameter of `V`: optional size, then optional `U`, then optional `/`.
fn parse_value(rest: &str) -> Result<Token, TokenProblem> {
    let (rest, total) = match rest.strip_suffix('/') {
        Some(rest) => (rest, true),
        None => (rest, false),
    };
    let (rest, unit) = match rest.strip_suffix('U') {
        Some(rest) => (rest, true),
        None => (rest, false),
    };
    let size = optional(rest, 3, 1, 999)?.unwrap_or(DEFAULT_VALUE_SIZE);
    Ok(Token::Value { size, unit, total })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The tokens of a composition, without their paints.
    fn parse(composition: &str) -> Result<Vec<Token>, BuildError> {
        super::parse(composition).map(|painted| painted.into_iter().map(|p| p.token).collect())
    }

    /// The paint of a composition of one token.
    fn paint(text: &str) -> Result<Paint, BuildError> {
        super::parse(text).map(|painted| painted[0].paint)
    }

    #[test]
    fn paints() {
        let fg = |color| Style::new().fg(color);
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let cases = [
            ("B1.R", fg(Color::Red), None),
            ("B1.R+", fg(Color::LightRed), None),
            ("S.K", fg(Color::Black), None),
            ("S.K+", fg(Color::DarkGray), None),
            ("S.W", fg(Color::Gray), None),
            ("S.W+", fg(Color::White), None),
            ("N.:B", Style::new().bg(Color::Blue), None),
            ("P.*", bold, None),
            (
                "N.W+:B+*",
                fg(Color::White)
                    .bg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD),
                None,
            ),
            ("I20L.C/K+", fg(Color::Cyan), Some(fg(Color::DarkGray))),
            (
                "V5U/.W/:K*",
                fg(Color::Gray),
                Some(Style::new().bg(Color::Black).add_modifier(Modifier::BOLD)),
            ),
            ("V/.G", fg(Color::Green), None),
        ];
        for (text, first, second) in cases {
            assert_eq!(paint(text), Ok(Paint { first, second }), "{text}");
        }
        assert_eq!(paint("I.R").unwrap().second(), fg(Color::Red));
        assert_eq!(paint("I").unwrap(), Paint::default());
        // The token before the `.` is checked as before.
        assert_eq!(
            parse("I20L.C/K+").unwrap(),
            [Token::Illustration {
                size: Some(20),
                look: Some(Look::Line)
            }]
        );
        assert_eq!(
            parse("V5U/.W").unwrap(),
            [Token::Value {
                size: 5,
                unit: true,
                total: true
            }]
        );
    }

    fn bad_token(text: &str, problem: TokenProblem) -> BuildError {
        BuildError::BadToken {
            position: 1,
            token: text.into(),
            problem,
        }
    }

    #[test]
    fn bad_paints() {
        for text in [
            "S.", "S.X", "S.r", "S.+R", "S.R++", "S.R::B", "S.:", "S.*R", "S.**", "S.R*:B", "I./R",
            "I.R/", "I.R/G/B", "V.R.G", "S..R", "P./R", ".R",
        ] {
            let expected = if text == ".R" {
                TokenProblem::UnknownComponent
            } else {
                TokenProblem::BadPaint
            };
            assert_eq!(paint(text), Err(bad_token(text, expected)), "{text}");
        }
        for text in ["S.R/G", "B0.R/G", "N.:K/*", "T2.*/*"] {
            assert_eq!(
                paint(text),
                Err(bad_token(text, TokenProblem::SecondPart)),
                "{text}"
            );
        }
        assert_eq!(
            super::parse("S.X").unwrap_err().to_string(),
            "token 1 (\"S.X\"): bad paint (.[color][:color][*], colors K R G Y B M C W, + bright)"
        );
    }

    fn bad(composition: &str) -> TokenProblem {
        match parse(composition) {
            Err(BuildError::BadToken { problem, .. }) => problem,
            other => panic!("expected a bad token for {composition:?}, got {other:?}"),
        }
    }

    #[test]
    fn the_reference_example() {
        assert_eq!(
            parse("B0 T2 B1 B2 S B2 L").unwrap(),
            [
                Token::Barrier(0),
                Token::Time(2),
                Token::Barrier(1),
                Token::Barrier(2),
                Token::Symbol(None),
                Token::Barrier(2),
                Token::Log(None),
            ]
        );
    }

    #[test]
    fn every_row_of_the_grammar() {
        let illustration = |size, look| Token::Illustration { size, look };
        let cases = [
            ("S", Token::Symbol(None)),
            ("S0", Token::Symbol(Some(0))),
            ("S9", Token::Symbol(Some(9))),
            ("B0", Token::Barrier(0)),
            ("B9", Token::Barrier(9)),
            ("D0", Token::Date(0)),
            ("D2", Token::Date(2)),
            ("T0", Token::Time(0)),
            ("T5", Token::Time(5)),
            ("P", Token::Progress(3)),
            ("P3", Token::Progress(3)),
            ("P9", Token::Progress(9)),
            ("K", Token::Keyword(None)),
            ("K9", Token::Keyword(Some(9))),
            ("N", Token::Name(None)),
            ("N0", Token::Name(Some(0))),
            ("N14", Token::Name(Some(14))),
            ("N99", Token::Name(Some(99))),
            ("L", Token::Log(None)),
            ("L2", Token::Log(Some(2))),
            ("L999", Token::Log(Some(999))),
            ("I", illustration(None, None)),
            ("I2", illustration(Some(2), None)),
            ("I999", illustration(Some(999), None)),
            ("IB", illustration(None, Some(Look::Bar))),
            ("IL", illustration(None, Some(Look::Line))),
            ("ID", illustration(None, Some(Look::Dot))),
            ("I20B", illustration(Some(20), Some(Look::Bar))),
            ("I2L", illustration(Some(2), Some(Look::Line))),
            ("I999D", illustration(Some(999), Some(Look::Dot))),
            ("IS", illustration(None, Some(Look::Square))),
            ("IO", illustration(None, Some(Look::Circle))),
            ("IR", illustration(None, Some(Look::Diamond))),
            ("IG", illustration(None, Some(Look::Segment))),
            ("IH", illustration(None, Some(Look::Shade))),
            ("IV", illustration(None, Some(Look::Vertical))),
            ("IA", illustration(None, Some(Look::Ascii))),
            ("I12C", illustration(Some(12), Some(Look::Chevron))),
        ];
        for (text, token) in cases {
            assert_eq!(parse(text).unwrap(), [token], "{text}");
        }
    }

    #[test]
    fn the_eight_forms_of_v() {
        let value = |size, unit, total| Token::Value { size, unit, total };
        let cases = [
            ("V", value(5, false, false)),
            ("V5", value(5, false, false)),
            ("VU", value(5, true, false)),
            ("V5U", value(5, true, false)),
            ("V5/", value(5, false, true)),
            ("V5U/", value(5, true, true)),
            ("VU/", value(5, true, true)),
            ("V/", value(5, false, true)),
            ("V1", value(1, false, false)),
            ("V999", value(999, false, false)),
        ];
        for (text, token) in cases {
            assert_eq!(parse(text).unwrap(), [token], "{text}");
        }
    }

    #[test]
    fn empty_composition() {
        assert_eq!(parse(""), Err(BuildError::EmptyComposition));
    }

    #[test]
    fn bad_spacing() {
        for (text, offset) in [
            (" S", 0),
            ("S ", 2),
            ("S  B0", 2),
            ("S\tB0", 1),
            ("S\u{a0}B0", 1),
            ("S B0\n", 4),
        ] {
            assert_eq!(
                parse(text),
                Err(BuildError::BadSpacing { offset }),
                "{text:?}"
            );
        }
    }

    #[test]
    fn unknown_tokens() {
        for text in ["X", "s", "b0", "l", "é", "\u{1b}"] {
            assert_eq!(bad(text), TokenProblem::UnknownComponent, "{text:?}");
        }
    }

    #[test]
    fn missing_parameters() {
        for text in ["B", "D", "T"] {
            assert_eq!(bad(text), TokenProblem::MissingParameter, "{text}");
        }
    }

    #[test]
    fn bad_parameters() {
        for text in [
            "SX", "B-1", "L+5", "VU5", "V/U", "V5//", "VUU", "N1a", "P½", "IX", "IBL", "IB2", "I2b",
        ] {
            assert_eq!(bad(text), TokenProblem::BadParameter, "{text}");
        }
    }

    #[test]
    fn too_many_digits() {
        for text in [
            "S10", "B00", "D01", "K12", "N100", "N000", "L1000", "V1000", "I0002",
        ] {
            assert_eq!(bad(text), TokenProblem::TooManyDigits, "{text}");
        }
    }

    #[test]
    fn out_of_range() {
        for text in [
            "D3", "T6", "P0", "L0", "L1", "I0", "I1", "I1D", "V0", "V000U",
        ] {
            assert_eq!(bad(text), TokenProblem::OutOfRange, "{text}");
        }
    }

    #[test]
    fn leading_zeros_are_accepted() {
        assert_eq!(parse("N05").unwrap(), [Token::Name(Some(5))]);
        assert_eq!(parse("L002").unwrap(), [Token::Log(Some(2))]);
    }

    #[test]
    fn p1_and_p2_cannot_show_100() {
        assert_eq!(bad("P1"), TokenProblem::ProgressTooSmall);
        assert_eq!(bad("P2"), TokenProblem::ProgressTooSmall);
    }

    #[test]
    fn unique_components_cannot_repeat() {
        for text in ["D0 D1", "T0 B0 T1", "P P4", "K K1", "L L5", "V V/", "I I3"] {
            assert!(
                matches!(parse(text), Err(BuildError::RepeatedComponent { .. })),
                "{text}"
            );
        }
        assert!(parse("S S1 B0 B0 N N3").is_ok());
    }

    #[test]
    fn errors_name_the_token_and_its_position() {
        assert_eq!(
            parse("B0 T2 X").unwrap_err().to_string(),
            "token 3 (\"X\"): unknown component"
        );
        assert_eq!(
            parse("L B0 L9").unwrap_err().to_string(),
            "token 3 (\"L9\"): component L can appear only once"
        );
    }

    /// Any valid token.
    fn token() -> impl Strategy<Value = Token> {
        prop_oneof![
            proptest::option::of(0..=9u8).prop_map(Token::Symbol),
            (0..=9u8).prop_map(Token::Barrier),
            (0..=2u8).prop_map(Token::Date),
            (0..=5u8).prop_map(Token::Time),
            (3..=9u8).prop_map(Token::Progress),
            proptest::option::of(0..=9u8).prop_map(Token::Keyword),
            proptest::option::of(0..=99u8).prop_map(Token::Name),
            proptest::option::of(2..=999u16).prop_map(Token::Log),
            (1..=999u16, any::<bool>(), any::<bool>())
                .prop_map(|(size, unit, total)| Token::Value { size, unit, total }),
            (
                proptest::option::of(2..=999u16),
                proptest::option::of(proptest::sample::select(Look::ALL.to_vec()))
            )
                .prop_map(|(size, look)| Token::Illustration { size, look }),
        ]
    }

    /// Any valid part of a paint.
    fn part() -> impl Strategy<Value = Style> {
        let color = (0..8usize, any::<bool>()).prop_map(|(i, bright)| {
            let (_, normal, light) = COLORS[i];
            if bright { light } else { normal }
        });
        (
            proptest::option::of(color.clone()),
            proptest::option::of(color),
            any::<bool>(),
        )
            .prop_filter("not empty", |(fg, bg, bold)| {
                fg.is_some() || bg.is_some() || *bold
            })
            .prop_map(|(fg, bg, bold)| {
                let mut style = Style::new();
                style.fg = fg;
                style.bg = bg;
                if bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                style
            })
    }

    /// Any valid token with any valid paint.
    fn painted() -> impl Strategy<Value = Painted> {
        (
            token(),
            proptest::option::of((part(), proptest::option::of(part()))),
        )
            .prop_map(|(token, paint)| {
                let paint = paint.map_or(Paint::default(), |(first, second)| Paint {
                    first,
                    second: second.filter(|_| {
                        matches!(token, Token::Illustration { .. } | Token::Value { .. })
                    }),
                });
                Painted { token, paint }
            })
    }

    /// A valid list of tokens: the unique components are kept at their first occurrence.
    fn tokens() -> impl Strategy<Value = Vec<Token>> {
        proptest::collection::vec(token(), 1..20).prop_map(|tokens| {
            let mut kept: Vec<Token> = Vec::new();
            for token in tokens {
                if !token.is_unique() || kept.iter().all(|t| t.letter() != token.letter()) {
                    kept.push(token);
                }
            }
            kept
        })
    }

    proptest! {
        #[test]
        fn parse_never_panics(text in ".{0,40}") {
            let _ = parse(&text);
        }

        #[test]
        fn parse_never_panics_on_grammar_like_text(
            text in "[SBDTPKNLVIUORGHACYMW/.:*+0-9 ]{0,30}"
        ) {
            let _ = parse(&text);
        }

        #[test]
        fn display_round_trips(tokens in tokens()) {
            let text = tokens.iter().map(ToString::to_string).collect::<Vec<_>>().join(" ");
            prop_assert_eq!(parse(&text).unwrap(), tokens);
        }

        #[test]
        fn painted_display_round_trips(painted in painted()) {
            let text = painted.to_string();
            prop_assert_eq!(super::parse(&text).unwrap(), [painted]);
        }

        #[test]
        fn parse_never_panics_on_paint_like_text(
            text in "[SBDTPKNLVI0-9][0-9BLDSORGHVAC]{0,3}[.KRGYBMCW+:*/]{0,8}"
        ) {
            let _ = super::parse(&text);
        }
    }
}
