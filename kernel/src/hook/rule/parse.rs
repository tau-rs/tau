//! Recursive descent over the tokens, with the static checks that make
//! evaluation total: every fact is one its point carries, every value has
//! the fact's type, `deny` appears only where it is admitted, and
//! parentheses stop at [`MAX_NESTING`].
//!
//! The grammar, from ADR-0008 §5:
//!
//! ```text
//! rule      := when <point> [ if <predicate> ] then <verdict>
//! point     := pre_send | pre_deliver | on_spawn | on_exit | on_budget ( <dim> , <int> )
//! predicate := <or>
//! or        := <and> { or <and> }
//! and       := <not> { and <not> }
//! not       := [ not ] <atom>
//! atom      := ( <predicate> )
//!            | <fact> <cmp> <value>
//!            | <fact> in { <value> { , <value> } }
//!            | payload contains <string>
//!            | payload starts_with <string>
//! fact      := depth | driver | kind | payload.len | remaining.<dim>
//! cmp       := == | != | < | <= | > | >=
//! verdict   := allow | deny <string> | emit [ to subject | to parent ] <string>
//! ```

use core::str::FromStr;

use super::lex::{tokens, Spanned, Tok};
use super::{
    carries, Atom, Cmp, Fact, Predicate, RuleError, RuleVerdict, Target, Ty, Value, Wish,
    MAX_NESTING,
};
use crate::abi::{DimKey, MsgKind, Name};
use crate::hook::HookPoint;

/// Parses a whole line.
pub(super) fn rule(src: &str) -> Result<(HookPoint, Option<Predicate>, RuleVerdict), RuleError> {
    let toks = tokens(src)?;
    if toks.is_empty() {
        return Err(RuleError::Empty);
    }
    let mut p = Parser {
        toks,
        pos: 0,
        end: src.len(),
        point: HookPoint::PreSend,
    };
    p.keyword("when", "`when`")?;
    p.point = p.point()?;
    let predicate = if p.is_ident("if") {
        p.pos += 1;
        Some(p.or(0)?)
    } else {
        None
    };
    p.keyword("then", "`then`")?;
    let verdict = p.verdict()?;
    if let Some(extra) = p.peek() {
        return Err(RuleError::Unexpected {
            at: extra.at,
            found: extra.tok.to_string(),
            expected: "end of rule",
        });
    }
    Ok((p.point, predicate, verdict))
}

struct Parser<'a> {
    toks: Vec<Spanned<'a>>,
    pos: usize,
    /// The source length: where "end of rule" is.
    end: usize,
    /// The point, once parsed; facts are checked against it as they appear.
    point: HookPoint,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Spanned<'a>> {
        self.toks.get(self.pos)
    }

    /// The offset of the next token, or of the end.
    fn at(&self) -> usize {
        self.peek().map_or(self.end, |s| s.at)
    }

    fn is_ident(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Spanned { tok: Tok::Ident(s), .. }) if *s == word)
    }

    fn unexpected(&self, expected: &'static str) -> RuleError {
        RuleError::Unexpected {
            at: self.at(),
            found: self
                .peek()
                .map_or_else(|| "end of rule".to_owned(), |s| s.tok.to_string()),
            expected,
        }
    }

    fn next(&mut self, expected: &'static str) -> Result<Spanned<'a>, RuleError> {
        let s = self
            .peek()
            .cloned()
            .ok_or_else(|| self.unexpected(expected))?;
        self.pos += 1;
        Ok(s)
    }

    fn keyword(&mut self, word: &str, expected: &'static str) -> Result<(), RuleError> {
        if self.is_ident(word) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.unexpected(expected))
        }
    }

    fn punct(&mut self, tok: &Tok<'_>, expected: &'static str) -> Result<(), RuleError> {
        match self.peek() {
            Some(s) if s.tok == *tok => {
                self.pos += 1;
                Ok(())
            }
            _ => Err(self.unexpected(expected)),
        }
    }

    fn ident(&mut self, expected: &'static str) -> Result<(&'a str, usize), RuleError> {
        match self.peek() {
            Some(Spanned {
                tok: Tok::Ident(s),
                at,
            }) => {
                let out = (*s, *at);
                self.pos += 1;
                Ok(out)
            }
            _ => Err(self.unexpected(expected)),
        }
    }

    fn string(&mut self, expected: &'static str) -> Result<String, RuleError> {
        match self.next(expected)? {
            Spanned {
                tok: Tok::Str(s), ..
            } => Ok(s),
            other => Err(RuleError::Unexpected {
                at: other.at,
                found: other.tok.to_string(),
                expected,
            }),
        }
    }

    fn int(&mut self, expected: &'static str) -> Result<u64, RuleError> {
        match self.next(expected)? {
            Spanned {
                tok: Tok::Int(n), ..
            } => Ok(n),
            other => Err(RuleError::Unexpected {
                at: other.at,
                found: other.tok.to_string(),
                expected,
            }),
        }
    }

    fn name(&mut self, expected: &'static str) -> Result<Name, RuleError> {
        let (s, at) = self.ident(expected)?;
        Name::new(s).map_err(|error| RuleError::BadName { at, error })
    }

    fn dim(&mut self) -> Result<DimKey, RuleError> {
        let (s, at) = self.ident("a budget dimension")?;
        DimKey::from_str(s).map_err(|error| RuleError::BadName { at, error })
    }

    fn point(&mut self) -> Result<HookPoint, RuleError> {
        let (word, at) = self.ident("a point")?;
        Ok(match word {
            "pre_send" => HookPoint::PreSend,
            "pre_deliver" => HookPoint::PreDeliver,
            "on_spawn" => HookPoint::OnSpawn,
            "on_exit" => HookPoint::OnExit,
            "on_budget" => {
                self.punct(&Tok::LParen, "`(` after on_budget")?;
                let dim = self.dim()?;
                self.punct(&Tok::Comma, "`,` between the dimension and the line")?;
                let below = self.int("the line, an integer")?;
                self.punct(&Tok::RParen, "`)` after the line")?;
                HookPoint::OnBudget { dim, below }
            }
            found => {
                return Err(RuleError::UnknownPoint {
                    at,
                    found: found.to_owned(),
                })
            }
        })
    }

    fn or(&mut self, depth: usize) -> Result<Predicate, RuleError> {
        let mut left = self.and(depth)?;
        while self.is_ident("or") {
            self.pos += 1;
            let right = self.and(depth)?;
            left = Predicate::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self, depth: usize) -> Result<Predicate, RuleError> {
        let mut left = self.not(depth)?;
        while self.is_ident("and") {
            self.pos += 1;
            let right = self.not(depth)?;
            left = Predicate::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// `not` is counted, not recursed: `not not x` is `x`.
    fn not(&mut self, depth: usize) -> Result<Predicate, RuleError> {
        let mut negate = false;
        while self.is_ident("not") {
            self.pos += 1;
            negate = !negate;
        }
        let atom = self.atom(depth)?;
        Ok(if negate {
            Predicate::Not(Box::new(atom))
        } else {
            atom
        })
    }

    fn atom(&mut self, depth: usize) -> Result<Predicate, RuleError> {
        if self.peek().map(|s| &s.tok) == Some(&Tok::LParen) {
            if depth >= MAX_NESTING {
                return Err(RuleError::TooDeep { limit: MAX_NESTING });
            }
            self.pos += 1;
            let inner = self.or(depth + 1)?;
            self.punct(&Tok::RParen, "`)`")?;
            return Ok(inner);
        }
        let (word, at) = self.ident("a fact, `not`, or `(`")?;
        let fact = match word {
            "depth" => Fact::Depth,
            "driver" => Fact::Driver,
            "kind" => Fact::Kind,
            "remaining" => {
                self.punct(&Tok::Dot, "`.<dim>` after remaining")?;
                Fact::Remaining(self.dim()?)
            }
            "payload" => match self.peek().map(|s| &s.tok) {
                Some(Tok::Dot) => {
                    self.pos += 1;
                    if self.is_ident("len") {
                        self.pos += 1;
                        Fact::PayloadLen
                    } else {
                        return Err(self.wish(Wish::JsonField));
                    }
                }
                Some(Tok::LBracket) => return Err(self.wish(Wish::JsonField)),
                Some(Tok::Tilde) => return Err(self.wish(Wish::Regex)),
                Some(Tok::Ident("matches" | "regex" | "like")) => {
                    return Err(self.wish(Wish::Regex))
                }
                Some(Tok::Ident("contains")) => {
                    self.pos += 1;
                    self.fact_at(&Fact::PayloadLen, "payload")?;
                    let s = self.string("a string after contains")?;
                    return Ok(Predicate::Atom(Atom::Contains(s)));
                }
                Some(Tok::Ident("starts_with")) => {
                    self.pos += 1;
                    self.fact_at(&Fact::PayloadLen, "payload")?;
                    let s = self.string("a string after starts_with")?;
                    return Ok(Predicate::Atom(Atom::StartsWith(s)));
                }
                _ => {
                    return Err(
                        self.unexpected("`.len`, `contains`, or `starts_with` after payload")
                    )
                }
            },
            other => return Err(unknown_fact(other, at, self.peek())),
        };
        let fact_text = fact.to_string();
        self.fact_at(&fact, &fact_text)?;
        match self.peek().map(|s| &s.tok) {
            Some(Tok::Arith(_)) => return Err(self.wish(Wish::Arithmetic)),
            Some(Tok::Tilde) => return Err(self.wish(Wish::Regex)),
            Some(Tok::Ident("in")) => {
                self.pos += 1;
                self.punct(&Tok::LBrace, "`{` after in")?;
                let mut values = vec![self.value(&fact, &fact_text)?];
                while self.peek().map(|s| &s.tok) == Some(&Tok::Comma) {
                    self.pos += 1;
                    values.push(self.value(&fact, &fact_text)?);
                }
                self.punct(&Tok::RBrace, "`,` or `}`")?;
                return Ok(Predicate::Atom(Atom::In { fact, values }));
            }
            _ => {}
        }
        let op_at = self.at();
        let cmp = match self.next("a comparison or `in`")?.tok {
            Tok::Eq => Cmp::Eq,
            Tok::Ne => Cmp::Ne,
            Tok::Lt => Cmp::Lt,
            Tok::Le => Cmp::Le,
            Tok::Gt => Cmp::Gt,
            Tok::Ge => Cmp::Ge,
            other => {
                return Err(RuleError::Unexpected {
                    at: op_at,
                    found: other.to_string(),
                    expected: "a comparison or `in`",
                })
            }
        };
        if cmp.is_order() && fact.ty() != Ty::Int {
            return Err(RuleError::Unordered {
                at: op_at,
                fact: fact_text,
            });
        }
        let value = self.value(&fact, &fact_text)?;
        if self
            .peek()
            .map(|s| &s.tok)
            .is_some_and(|t| matches!(t, Tok::Arith(_)))
        {
            return Err(self.wish(Wish::Arithmetic));
        }
        Ok(Predicate::Atom(Atom::Cmp { fact, cmp, value }))
    }

    /// The static check: the point's events carry this fact.
    fn fact_at(&self, fact: &Fact, text: &str) -> Result<(), RuleError> {
        if fact.at(&self.point) {
            Ok(())
        } else {
            Err(RuleError::FactNotAtPoint {
                fact: text.to_owned(),
                point: self.point.clone(),
                carries: carries(&self.point),
            })
        }
    }

    /// A value of the fact's type.
    fn value(&mut self, fact: &Fact, fact_text: &str) -> Result<Value, RuleError> {
        let ty = fact.ty();
        let at = self.at();
        let mismatch = |at| RuleError::TypeMismatch {
            at,
            fact: fact_text.to_owned(),
            expected: ty.describe(),
        };
        match (ty, self.peek().map(|s| &s.tok)) {
            (Ty::Int, Some(Tok::Int(n))) => {
                let n = *n;
                self.pos += 1;
                Ok(Value::Int(n))
            }
            (Ty::Driver, Some(Tok::Ident(_))) => Ok(Value::Driver(self.name("a driver name")?)),
            (Ty::Msg, Some(Tok::Ident(word))) => {
                let kind = match *word {
                    "request" => MsgKind::Request,
                    "reply" => MsgKind::Reply,
                    "partial" => MsgKind::Partial,
                    "notice" => MsgKind::Notice,
                    _ => return Err(mismatch(at)),
                };
                self.pos += 1;
                Ok(Value::MsgKind(kind))
            }
            (_, Some(Tok::Ident(_) | Tok::Int(_) | Tok::Str(_))) => Err(mismatch(at)),
            _ => Err(self.unexpected("a value")),
        }
    }

    fn verdict(&mut self) -> Result<RuleVerdict, RuleError> {
        let (word, at) = self.ident("a verdict")?;
        Ok(match word {
            "allow" => RuleVerdict::Allow,
            "deny" => {
                if !self.point.admits_deny() {
                    return Err(RuleError::DenyNotAdmitted {
                        point: self.point.clone(),
                    });
                }
                RuleVerdict::Deny(self.string("a reason, as a string")?)
            }
            "emit" => {
                let to = if self.is_ident("to") {
                    self.pos += 1;
                    let (target, at) = self.ident("`subject` or `parent`")?;
                    match target {
                        "subject" => Target::Subject,
                        "parent" => Target::Parent,
                        other => {
                            return Err(RuleError::Unexpected {
                                at,
                                found: format!("`{other}`"),
                                expected: "`subject` or `parent`",
                            })
                        }
                    }
                } else {
                    Target::Subject
                };
                let note = self.string("a note, as a string")?;
                RuleVerdict::Emit { to, note }
            }
            "rewrite" | "replace" | "redact" | "modify" | "set" => {
                return Err(RuleError::NotExpressible {
                    at,
                    wish: Wish::Rewrite,
                })
            }
            found => {
                return Err(RuleError::UnknownVerdict {
                    at,
                    found: found.to_owned(),
                })
            }
        })
    }

    fn wish(&self, wish: Wish) -> RuleError {
        RuleError::NotExpressible {
            at: self.at(),
            wish,
        }
    }
}

/// An identifier in fact position that is not a fact: either one of the
/// things the ADR says a rule cannot say, named with its home, or simply
/// unknown.
fn unknown_fact(word: &str, at: usize, next: Option<&Spanned<'_>>) -> RuleError {
    let wish = match word {
        "count" | "counts" | "rate" | "times" | "seen" | "last" | "previous" | "history"
        | "total" | "sum" | "since" => Some(Wish::AcrossEvents),
        "parent" | "sibling" | "siblings" | "child" | "children" | "agent" | "agents" | "root"
        | "tree" => Some(Wish::AnotherAgent),
        "now" | "time" | "clock" | "elapsed" | "wall" | "date" => Some(Wish::Time),
        "lower" | "upper" | "trim" | "len" | "length" | "hash" | "json" | "decode" => {
            Some(Wish::StringTransform)
        }
        "regex" | "matches" | "match" => Some(Wish::Regex),
        "args" | "field" | "fields" | "body" => Some(Wish::JsonField),
        _ => match next.map(|s| &s.tok) {
            // `something.field`: a path into the payload.
            Some(Tok::Dot | Tok::LBracket) => Some(Wish::JsonField),
            _ => None,
        },
    };
    match wish {
        Some(wish) => RuleError::NotExpressible { at, wish },
        None => RuleError::UnknownFact {
            at,
            found: word.to_owned(),
        },
    }
}
