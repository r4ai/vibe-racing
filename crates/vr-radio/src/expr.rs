//! Conditions over the live values: `gap_behind < 1 and lap > 3`,
//! `lap % 5 == 0`, `abs(steer) > 45`. Written by the agent for the driver,
//! checked ten times a second.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

pub type Values = BTreeMap<String, f64>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Or,
    And,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl Op {
    fn of(token: &str) -> Option<(Self, u8)> {
        Some(match token {
            "or" | "||" => (Self::Or, 1),
            "and" | "&&" => (Self::And, 2),
            "<" => (Self::Lt, 4),
            "<=" => (Self::Le, 4),
            ">" => (Self::Gt, 4),
            ">=" => (Self::Ge, 4),
            "==" | "=" => (Self::Eq, 4),
            "!=" => (Self::Ne, 4),
            "+" => (Self::Add, 5),
            "-" => (Self::Sub, 5),
            "*" => (Self::Mul, 6),
            "/" => (Self::Div, 6),
            "%" => (Self::Rem, 6),
            _ => return None,
        })
    }
}

/// Binds weaker than a comparison: `not a < 1`.
const NOT: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fun {
    Abs,
    Min,
    Max,
    Floor,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Number(f64),
    Value(String),
    Not(Box<Node>),
    Neg(Box<Node>),
    Call(Fun, Vec<Node>),
    Binary(Op, Box<Node>, Box<Node>),
}

/// A condition as it was written, ready to be checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    source: String,
    node: Node,
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

fn truth(b: bool) -> Option<f64> {
    Some(f64::from(u8::from(b)))
}

/// Values closer than this are equal.
const EQUAL: f64 = 1e-6;

impl Node {
    /// `None`: a value it needs is not known.
    fn eval(&self, v: &Values) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            Self::Value(k) => v.get(k).copied(),
            Self::Neg(n) => Some(-n.eval(v)?),
            Self::Not(n) => truth(n.eval(v)? == 0.0),
            Self::Call(f, args) => {
                let args = args
                    .iter()
                    .map(|a| a.eval(v))
                    .collect::<Option<Vec<f64>>>()?;
                let all = args.iter().copied();
                match f {
                    Fun::Abs => args.first().map(|a| a.abs()),
                    Fun::Floor => args.first().map(|a| a.floor()),
                    Fun::Min => all.reduce(f64::min),
                    Fun::Max => all.reduce(f64::max),
                }
            }
            Self::Binary(op, a, b) => {
                let (a, b) = (a.eval(v), b.eval(v));
                // One side may decide without the other.
                match op {
                    Op::And if a == Some(0.0) || b == Some(0.0) => return Some(0.0),
                    Op::Or if [a, b].iter().flatten().any(|x| *x != 0.0) => return Some(1.0),
                    _ => {}
                }
                let (a, b) = (a?, b?);
                match op {
                    Op::Or => truth(a != 0.0 || b != 0.0),
                    Op::And => truth(a != 0.0 && b != 0.0),
                    Op::Lt => truth(a < b),
                    Op::Le => truth(a <= b),
                    Op::Gt => truth(a > b),
                    Op::Ge => truth(a >= b),
                    Op::Eq => truth((a - b).abs() < EQUAL),
                    Op::Ne => truth((a - b).abs() >= EQUAL),
                    Op::Add => Some(a + b),
                    Op::Sub => Some(a - b),
                    Op::Mul => Some(a * b),
                    Op::Div => (b != 0.0).then(|| a / b),
                    Op::Rem => (b != 0.0).then(|| a.rem_euclid(b)),
                }
            }
        }
    }

    fn keys<'a>(&'a self, out: &mut BTreeSet<&'a str>) {
        match self {
            Self::Number(_) => {}
            Self::Value(k) => {
                out.insert(k);
            }
            Self::Not(n) | Self::Neg(n) => n.keys(out),
            Self::Call(_, args) => args.iter().for_each(|a| a.keys(out)),
            Self::Binary(_, a, b) => {
                a.keys(out);
                b.keys(out);
            }
        }
    }
}

impl Expr {
    /// Whether it holds; not while a value it needs is missing.
    pub fn holds(&self, v: &Values) -> bool {
        self.node.eval(v).is_some_and(|x| x != 0.0)
    }

    /// The values it reads.
    pub fn keys(&self) -> BTreeSet<&str> {
        let mut out = BTreeSet::new();
        self.node.keys(&mut out);
        out
    }
}

fn tokens(s: &str) -> Result<Vec<String>, String> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let start = i;
        let word = |k: char| k.is_ascii_alphanumeric() || k == '_' || k == '.';
        if c[i].is_whitespace() {
            i += 1;
            continue;
        } else if word(c[i]) {
            while i < c.len() && word(c[i]) {
                i += 1;
            }
        } else {
            let two: String = c[i..].iter().take(2).collect();
            i += if matches!(two.as_str(), "<=" | ">=" | "==" | "!=" | "&&" | "||") {
                2
            } else {
                1
            };
        }
        let t: String = c[start..i].iter().collect();
        out.push(match t.as_str() {
            // Written by people and by models alike.
            "≤" => "<=".into(),
            "≥" => ">=".into(),
            "≠" => "!=".into(),
            "＜" => "<".into(),
            "＞" => ">".into(),
            "×" => "*".into(),
            "÷" => "/".into(),
            "AND" => "and".into(),
            "OR" => "or".into(),
            "NOT" | "!" => "not".into(),
            _ => t,
        });
    }
    if out.is_empty() {
        return Err("式が空".into());
    }
    Ok(out)
}

/// Deeper than any condition: the parser calls itself.
const MAX_DEPTH: usize = 40;

struct Parser {
    tokens: Vec<String>,
    at: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.at).map(String::as_str)
    }

    fn next(&mut self) -> Option<String> {
        let t = self.tokens.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn expect(&mut self, want: &str) -> Result<(), String> {
        match self.next() {
            Some(t) if t == want => Ok(()),
            t => Err(format!("{want} が必要: {}", t.unwrap_or("終わり".into()))),
        }
    }

    /// Operators that bind at least as strongly as `least`.
    fn binary(&mut self, least: u8) -> Result<Node, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("式が複雑すぎる".into());
        }
        let mut left = self.unary()?;
        while let Some((op, level)) = self.peek().and_then(Op::of) {
            if level < least {
                break;
            }
            self.at += 1;
            let right = self.binary(level + 1)?;
            left = Node::Binary(op, Box::new(left), Box::new(right));
        }
        self.depth -= 1;
        Ok(left)
    }

    fn unary(&mut self) -> Result<Node, String> {
        match self.peek() {
            Some("not") => {
                self.at += 1;
                Ok(Node::Not(Box::new(self.binary(NOT + 1)?)))
            }
            Some("-") => {
                self.at += 1;
                // Not `binary`: the sign belongs to the operand only.
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return Err("式が複雑すぎる".into());
                }
                let n = self.unary()?;
                self.depth -= 1;
                Ok(Node::Neg(Box::new(n)))
            }
            _ => self.operand(),
        }
    }

    fn operand(&mut self) -> Result<Node, String> {
        let t = self.next().ok_or("式が途中で終わっている")?;
        if t == "(" {
            let n = self.binary(0)?;
            self.expect(")")?;
            return Ok(n);
        }
        if t.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
            return match t.parse::<f64>() {
                Ok(n) if n.is_finite() => Ok(Node::Number(n)),
                _ => Err(format!("数ではない: {t}")),
            };
        }
        if !t.starts_with(|c: char| c.is_ascii_alphabetic()) || Op::of(&t).is_some() {
            return Err(format!("読めない: {t}"));
        }
        match t.as_str() {
            "true" | "on" => return Ok(Node::Number(1.0)),
            "false" | "off" => return Ok(Node::Number(0.0)),
            _ => {}
        }
        if self.peek() != Some("(") {
            return Ok(Node::Value(t));
        }
        let f = match t.as_str() {
            "abs" => Fun::Abs,
            "min" => Fun::Min,
            "max" => Fun::Max,
            "floor" => Fun::Floor,
            _ => return Err(format!("関数ではない: {t}")),
        };
        self.at += 1;
        let mut args = vec![self.binary(0)?];
        while self.peek() == Some(",") {
            self.at += 1;
            args.push(self.binary(0)?);
        }
        self.expect(")")?;
        Ok(Node::Call(f, args))
    }
}

impl FromStr for Expr {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let source = s.trim().to_string();
        let mut p = Parser {
            tokens: tokens(&source)?,
            at: 0,
            depth: 0,
        };
        let node = p.binary(0)?;
        match p.peek() {
            Some(t) => Err(format!("余分: {t}")),
            None => Ok(Self { source, node }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> Values {
        [
            ("gap_behind", 0.8),
            ("lap", 10.0),
            ("brake", 0.0),
            ("steer", -50.0),
            ("yellow", 1.0),
        ]
        .map(|(k, v)| (k.to_string(), v))
        .into()
    }

    #[track_caller]
    fn check(s: &str, want: bool) {
        let e: Expr = s.parse().unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(e.holds(&values()), want, "{s}");
    }

    #[test]
    fn comparisons_and_logic() {
        check("gap_behind < 1", true);
        check("gap_behind<0.8", false);
        check("gap_behind <= 0.8", true);
        check("lap >= 10 and gap_behind > 1", false);
        check("lap >= 10 or gap_behind > 1", true);
        check("lap == 10", true);
        check("lap = 10", true);
        check("lap != 10", false);
        check("not lap == 9", true);
        check("not yellow", false);
        check("yellow", true);
        check("!brake && yellow == on", true);
        check("brake == off", true);
        check("not (lap > 9 and yellow)", false);
        // `and` binds more strongly than `or`.
        check("yellow or brake and brake", true);
        check("(yellow or brake) and brake", false);
        check("gap_behind ≤ 0.8 AND lap ≥ 10", true);
    }

    #[test]
    fn arithmetic() {
        check("lap % 5 == 0", true);
        check("lap % 3 == 0", false);
        check("2 + 3 * 4 == 14", true);
        check("10 - 2 - 3 == 5", true);
        check("12 / 2 / 3 == 2", true);
        check("-steer > 45", true);
        check("abs(steer) > 50", false);
        check("abs(steer) >= 50", true);
        check("abs(-steer + 1) == 51", true);
        check("-2 + 5 == 3", true);
        check("min(lap, 3) + max(1, 2, gap_behind) == 5", true);
        check("floor(gap_behind) == 0", true);
        check("-1 % 5 == 4", true);
        check("lap / 0 == lap", false);
        check("lap % 0 == 0", false);
    }

    #[test]
    fn unknown_values_decide_nothing() {
        check("gap_ahead < 1", false);
        check("not (gap_ahead < 1)", false);
        check("gap_ahead < 1 or yellow", true);
        check("yellow or gap_ahead < 1", true);
        check("gap_ahead < 1 or brake", false);
        check("gap_ahead < 1 and brake", false);
        check("not (gap_ahead < 1 and brake)", true);
        check("yellow and gap_ahead < 1", false);
        check("abs(gap_ahead) >= 0", false);
    }

    #[test]
    fn what_cannot_be_read_is_an_error() {
        for s in [
            "",
            "  ",
            "gap_behind <",
            "< 1",
            "gap_behind < 1 2",
            "(lap > 1",
            "lap > 1)",
            "sin(lap) > 0",
            "abs(lap",
            "1.2.3 > 1",
            "前 < 1",
            "lap and",
            "and lap",
            "lap > > 1",
        ] {
            assert!(s.parse::<Expr>().is_err(), "{s}");
        }
        assert!("(".repeat(5000).parse::<Expr>().is_err());
        assert!("-".repeat(5000).parse::<Expr>().is_err());
        assert!(
            format!("{}lap{}", "(".repeat(20), ")".repeat(20))
                .parse::<Expr>()
                .is_ok()
        );
    }

    #[test]
    fn keeps_what_was_written_and_what_it_reads() {
        let e: Expr = " abs(steer) > 45 or min(lap, 3) > -brake ".parse().unwrap();
        assert_eq!(e.to_string(), "abs(steer) > 45 or min(lap, 3) > -brake");
        assert_eq!(
            e.keys().into_iter().collect::<Vec<_>>(),
            ["brake", "lap", "steer"]
        );
    }
}
