//! Construction of a [`LineWidget`](crate::LineWidget): the builder, its errors and its
//! warnings.

use std::fmt;

pub use crate::composition::TokenProblem;

/// Why a [`LineWidget`](crate::LineWidget) could not be built.
///
/// Positions of tokens count from 1, in composition order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuildError {
    /// The composition is empty.
    EmptyComposition,
    /// A leading, trailing or double space, or whitespace other than U+0020, at byte
    /// `offset` of the composition.
    BadSpacing {
        /// Byte offset of the problem in the composition.
        offset: usize,
    },
    /// A token that the grammar does not accept.
    BadToken {
        /// Position of the token (from 1).
        position: usize,
        /// The text of the token.
        token: String,
        /// What is wrong with it.
        problem: TokenProblem,
    },
    /// A unique component (`D`, `T`, `P`, `K`, `L`, `V`, `I`) that appears a second time.
    RepeatedComponent {
        /// Position of the second occurrence (from 1).
        position: usize,
        /// The text of the token.
        token: String,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyComposition => f.write_str("empty composition"),
            Self::BadSpacing { offset } => write!(
                f,
                "bad spacing at byte {offset}: tokens are separated by exactly one space"
            ),
            Self::BadToken {
                position,
                token,
                problem,
            } => write!(f, "token {position} ({token:?}): {problem}"),
            Self::RepeatedComponent { position, token } => write!(
                f,
                "token {position} ({token:?}): component {} can appear only once",
                token.chars().next().unwrap_or(' ')
            ),
        }
    }
}

impl std::error::Error for BuildError {}
