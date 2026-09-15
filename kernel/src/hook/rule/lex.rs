//! Tokens of the rule language.
//!
//! Identifiers are `[a-z][a-z0-9_-]*`, the [`Name`] grammar, so a driver
//! or a custom dimension can be written bare. Keywords are identifiers the
//! parser recognises by text. Strings are double-quoted with five escapes.
//! Integers are decimal `u64`. Everything else is punctuation, including
//! the characters the language does not use — `+`, `~`, `[` — which the
//! lexer keeps so the parser can say what the author was reaching for.
//!
//! [`Name`]: crate::abi::Name

use core::fmt;
use core::iter::Peekable;
use core::str::CharIndices;

use super::RuleError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Tok<'a> {
    Ident(&'a str),
    Int(u64),
    Str(String),
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `~` or `=~`: the regex wish.
    Tilde,
    /// `+ - * / %`: the arithmetic wish.
    Arith(char),
}

impl fmt::Display for Tok<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ident(s) => write!(f, "`{s}`"),
            Self::Int(n) => write!(f, "`{n}`"),
            Self::Str(_) => f.write_str("a string"),
            Self::LParen => f.write_str("`(`"),
            Self::RParen => f.write_str("`)`"),
            Self::LBrace => f.write_str("`{`"),
            Self::RBrace => f.write_str("`}`"),
            Self::LBracket => f.write_str("`[`"),
            Self::RBracket => f.write_str("`]`"),
            Self::Comma => f.write_str("`,`"),
            Self::Dot => f.write_str("`.`"),
            Self::Eq => f.write_str("`==`"),
            Self::Ne => f.write_str("`!=`"),
            Self::Lt => f.write_str("`<`"),
            Self::Le => f.write_str("`<=`"),
            Self::Gt => f.write_str("`>`"),
            Self::Ge => f.write_str("`>=`"),
            Self::Tilde => f.write_str("`~`"),
            Self::Arith(c) => write!(f, "`{c}`"),
        }
    }
}

/// A token and the byte offset it starts at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Spanned<'a> {
    pub(super) tok: Tok<'a>,
    pub(super) at: usize,
}

/// Tokenises the whole line.
///
/// # Errors
///
/// A character no token starts with, an unterminated string, a bad
/// escape, or an integer past `u64::MAX`.
pub(super) fn tokens(src: &str) -> Result<Vec<Spanned<'_>>, RuleError> {
    let mut out = Vec::new();
    let mut chars = src.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let tok = match c {
            c if c.is_whitespace() => continue,
            'a'..='z' => Tok::Ident(ident(src, at, &mut chars)),
            '0'..='9' => Tok::Int(int(at, c, &mut chars)?),
            '"' => Tok::Str(string(at, &mut chars)?),
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '{' => Tok::LBrace,
            '}' => Tok::RBrace,
            '[' => Tok::LBracket,
            ']' => Tok::RBracket,
            ',' => Tok::Comma,
            '.' => Tok::Dot,
            '~' => Tok::Tilde,
            '+' | '-' | '*' | '/' | '%' => Tok::Arith(c),
            '=' => match chars.peek() {
                Some((_, '=')) => {
                    chars.next();
                    Tok::Eq
                }
                Some((_, '~')) => {
                    chars.next();
                    Tok::Tilde
                }
                _ => return Err(RuleError::UnexpectedChar { at, found: c }),
            },
            '!' => match chars.peek() {
                Some((_, '=')) => {
                    chars.next();
                    Tok::Ne
                }
                _ => return Err(RuleError::UnexpectedChar { at, found: c }),
            },
            '<' => match chars.peek() {
                Some((_, '=')) => {
                    chars.next();
                    Tok::Le
                }
                _ => Tok::Lt,
            },
            '>' => match chars.peek() {
                Some((_, '=')) => {
                    chars.next();
                    Tok::Ge
                }
                _ => Tok::Gt,
            },
            found => return Err(RuleError::UnexpectedChar { at, found }),
        };
        out.push(Spanned { tok, at });
    }
    Ok(out)
}

/// The rest of an identifier that started at `start`.
fn ident<'a>(src: &'a str, start: usize, chars: &mut Peekable<CharIndices<'a>>) -> &'a str {
    let mut end = src.len();
    while let Some(&(at, c)) = chars.peek() {
        let more = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';
        if !more {
            end = at;
            break;
        }
        chars.next();
    }
    // `start` and `end` are char boundaries handed out by `char_indices`,
    // so this is `Some`; the fallback cannot be reached.
    src.get(start..end).unwrap_or("")
}

/// The rest of an integer whose first digit was `first`.
fn int(at: usize, first: char, chars: &mut Peekable<CharIndices<'_>>) -> Result<u64, RuleError> {
    let mut n = u64::from(first as u8 - b'0');
    while let Some(&(_, c)) = chars.peek() {
        let Some(d) = c.to_digit(10) else {
            break;
        };
        chars.next();
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add(u64::from(d)))
            .ok_or(RuleError::IntTooLarge { at })?;
    }
    Ok(n)
}

/// The body of a string whose opening quote was at `at`.
fn string(at: usize, chars: &mut Peekable<CharIndices<'_>>) -> Result<String, RuleError> {
    let mut s = String::new();
    loop {
        match chars.next() {
            None => return Err(RuleError::UnterminatedString { at }),
            Some((_, '"')) => return Ok(s),
            Some((esc_at, '\\')) => match chars.next() {
                Some((_, '"')) => s.push('"'),
                Some((_, '\\')) => s.push('\\'),
                Some((_, 'n')) => s.push('\n'),
                Some((_, 'r')) => s.push('\r'),
                Some((_, 't')) => s.push('\t'),
                Some((_, found)) => return Err(RuleError::BadEscape { at: esc_at, found }),
                None => return Err(RuleError::UnterminatedString { at }),
            },
            Some((_, c)) => s.push(c),
        }
    }
}
