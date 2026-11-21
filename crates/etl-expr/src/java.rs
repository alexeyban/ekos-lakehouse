//! A Java subset: the lexer and expression parser shared by User Defined Java Expression,
//! Java Filter and (RFC 0203) User Defined Java Class, and the lowering of an expression over
//! Kettle fields to SQL.
//!
//! Java's own semantics are kept where they differ from SQL: `==` on two objects compares
//! identity (a gap), `+` with a String concatenates and prints null as `"null"`, Integer
//! division truncates, `substring` is 0-based.

use crate::semantics::Gap;
use crate::types::KType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Float(String),
    Str(String),
    Char(char),
    /// Operators and punctuation, longest match first.
    P(&'static str),
}

const PUNCT: &[&str] = &[
    "===", "!==", ">>>=", "<<=", ">>=", ">>>", "...", "==", "!=", "<=", ">=", "&&", "||", "++",
    "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<", ">>", "->", "::", "+", "-", "*",
    "/", "%", "=", "<", ">", "!", "~", "?", ":", ".", ",", ";", "(", ")", "[", "]", "{", "}", "&",
    "|", "^", "@",
];

/// Splits Java source into tokens; comments are dropped.
pub fn lex(src: &str) -> Result<Vec<Tok>, Gap> {
    lex_with(src, false)
}

/// Splits Java (`js = false`) or JavaScript source into tokens. JavaScript single quotes are
/// strings, not characters.
pub fn lex_with(src: &str, js: bool) -> Result<Vec<Tok>, Gap> {
    let cs: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && cs.get(i + 1) == Some(&'/') {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && cs.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < cs.len() && !(cs[i] == '*' && cs[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let s = i;
            while i < cs.len() && (cs[i].is_ascii_alphanumeric() || cs[i] == '_' || cs[i] == '$') {
                i += 1;
            }
            out.push(Tok::Ident(cs[s..i].iter().collect()));
        } else if c.is_ascii_digit()
            || (c == '.' && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            let s = i;
            let mut float = false;
            while i < cs.len() && (cs[i].is_ascii_alphanumeric() || cs[i] == '.' || cs[i] == '_') {
                if cs[i] == '.'
                    || ((cs[i] == 'e' || cs[i] == 'E')
                        && !cs[s..i].contains(&'x')
                        && !cs[s..i].contains(&'X'))
                {
                    float = true;
                }
                i += 1;
            }
            let raw: String = cs[s..i].iter().filter(|c| **c != '_').collect();
            let lower = raw.to_ascii_lowercase();
            let hex = lower.starts_with("0x");
            if !hex && (float || lower.ends_with('d') || lower.ends_with('f')) {
                let t = lower.trim_end_matches(['d', 'f']);
                out.push(Tok::Float(t.to_string()));
            } else {
                let t = lower.trim_end_matches('l');
                let v = if let Some(h) = t.strip_prefix("0x") {
                    i64::from_str_radix(h, 16)
                } else {
                    t.parse::<i64>()
                };
                out.push(Tok::Int(
                    v.map_err(|_| Gap::new(format!("number `{raw}`")))?,
                ));
            }
        } else if c == '"' || c == '\'' {
            let q = c;
            let mut s = String::new();
            i += 1;
            loop {
                match cs.get(i) {
                    None => return Err(Gap::new("unclosed string or char literal")),
                    Some('\\') => {
                        let e = cs.get(i + 1).copied().unwrap_or('\\');
                        s.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            '0' => '\0',
                            other => other,
                        });
                        i += 2;
                    }
                    Some(ch) if *ch == q => {
                        i += 1;
                        break;
                    }
                    Some(ch) => {
                        s.push(*ch);
                        i += 1;
                    }
                }
            }
            if q == '"' || js {
                out.push(Tok::Str(s));
            } else {
                out.push(Tok::Char(s.chars().next().unwrap_or('\0')));
            }
        } else {
            let rest: String = cs[i..cs.len().min(i + 4)].iter().collect();
            let Some(p) = PUNCT.iter().find(|p| rest.starts_with(**p)) else {
                return Err(Gap::new(format!("unexpected character `{c}`")));
            };
            out.push(Tok::P(p));
            i += p.chars().count();
        }
    }
    Ok(out)
}

/// A Java expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JExpr {
    Name(String),
    Int(i64),
    Float(String),
    Str(String),
    Char(char),
    Bool(bool),
    Null,
    /// `target.name`
    Field(Box<JExpr>, String),
    /// `target.name(args)` or `name(args)`.
    Call(Option<Box<JExpr>>, String, Vec<JExpr>),
    New(String, Vec<JExpr>),
    Index(Box<JExpr>, Box<JExpr>),
    Unary(&'static str, Box<JExpr>),
    /// `x++` / `x--`.
    Postfix(&'static str, Box<JExpr>),
    Binary(&'static str, Box<JExpr>, Box<JExpr>),
    Ternary(Box<JExpr>, Box<JExpr>, Box<JExpr>),
    Cast(String, Box<JExpr>),
    InstanceOf(Box<JExpr>, String),
    /// `target op= value` (`=` included).
    Assign(&'static str, Box<JExpr>, Box<JExpr>),
}

/// A recursive-descent parser over [`Tok`]s; the statement parser of RFC 0203 extends it.
pub struct Parser {
    pub toks: Vec<Tok>,
    pub pos: usize,
}

const PRIMITIVES: &[&str] = &[
    "int", "long", "double", "float", "short", "byte", "char", "boolean",
];
const BOXED: &[&str] = &[
    "String",
    "Integer",
    "Long",
    "Double",
    "Float",
    "Short",
    "Byte",
    "Character",
    "Boolean",
    "Object",
    "BigDecimal",
    "Date",
    "Number",
];

fn binary_power(op: &str) -> Option<u8> {
    Some(match op {
        "||" => 3,
        "&&" => 4,
        "|" => 5,
        "^" => 6,
        "&" => 7,
        "==" | "!=" | "===" | "!==" => 8,
        "<" | ">" | "<=" | ">=" => 9,
        "<<" | ">>" | ">>>" => 10,
        "+" | "-" => 11,
        "*" | "/" | "%" => 12,
        _ => return None,
    })
}

impl Parser {
    pub fn new(src: &str) -> Result<Parser, Gap> {
        Ok(Parser {
            toks: lex(src)?,
            pos: 0,
        })
    }

    /// A parser over JavaScript (`js = true`) or Java source.
    pub fn new_with(src: &str, js: bool) -> Result<Parser, Gap> {
        Ok(Parser {
            toks: lex_with(src, js)?,
            pos: 0,
        })
    }

    pub fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    pub fn peek_at(&self, n: usize) -> Option<&Tok> {
        self.toks.get(self.pos + n)
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }

    pub fn is(&self, p: &str) -> bool {
        matches!(self.peek(), Some(Tok::P(x)) if *x == p)
    }

    pub fn is_ident(&self, name: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(x)) if x == name)
    }

    pub fn eat(&mut self, p: &str) -> bool {
        if self.is(p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub fn expect(&mut self, p: &str) -> Result<(), Gap> {
        if self.eat(p) {
            Ok(())
        } else {
            Err(Gap::new(format!(
                "expected `{p}`, found {}",
                self.describe()
            )))
        }
    }

    pub fn describe(&self) -> String {
        match self.peek() {
            None => "the end".into(),
            Some(Tok::Ident(s)) => format!("`{s}`"),
            Some(Tok::P(p)) => format!("`{p}`"),
            Some(Tok::Str(s)) => format!("\"{s}\""),
            Some(t) => format!("{t:?}"),
        }
    }

    pub fn ident(&mut self) -> Result<String, Gap> {
        match self.peek().cloned() {
            Some(Tok::Ident(s)) => {
                self.pos += 1;
                Ok(s)
            }
            _ => Err(Gap::new(format!(
                "expected a name, found {}",
                self.describe()
            ))),
        }
    }

    /// A dotted type name with optional generics and array brackets, e.g. `java.util.List<String>[]`.
    pub fn type_name(&mut self) -> Result<String, Gap> {
        let mut name = self.ident()?;
        while self.is(".") && matches!(self.peek_at(1), Some(Tok::Ident(_))) {
            self.pos += 1;
            name.push('.');
            name.push_str(&self.ident()?);
        }
        if self.eat("<") {
            let mut depth = 1;
            while depth > 0 && !self.at_end() {
                if self.eat("<") {
                    depth += 1;
                } else if self.eat(">") {
                    depth -= 1;
                } else if self.eat(">>") {
                    depth -= 2;
                } else {
                    self.pos += 1;
                }
            }
        }
        while self.is("[") && matches!(self.peek_at(1), Some(Tok::P("]"))) {
            self.pos += 2;
            name.push_str("[]");
        }
        Ok(name)
    }

    /// One expression (assignment included).
    pub fn expr(&mut self) -> Result<JExpr, Gap> {
        let lhs = self.ternary()?;
        for op in ["=", "+=", "-=", "*=", "/=", "%="] {
            if self.eat(op) {
                let rhs = self.expr()?;
                let op: &'static str = PUNCT.iter().find(|p| **p == op).expect("listed");
                return Ok(JExpr::Assign(op, Box::new(lhs), Box::new(rhs)));
            }
        }
        Ok(lhs)
    }

    fn ternary(&mut self) -> Result<JExpr, Gap> {
        let c = self.binary(0)?;
        if self.eat("?") {
            let a = self.expr()?;
            self.expect(":")?;
            let b = self.ternary()?;
            return Ok(JExpr::Ternary(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }

    fn binary(&mut self, min: u8) -> Result<JExpr, Gap> {
        let mut lhs = self.unary()?;
        loop {
            if self.is_ident("instanceof") {
                self.pos += 1;
                let t = self.type_name()?;
                lhs = JExpr::InstanceOf(Box::new(lhs), t);
                continue;
            }
            let Some(Tok::P(op)) = self.peek().cloned() else {
                break;
            };
            let Some(p) = binary_power(op) else { break };
            if p < min {
                break;
            }
            self.pos += 1;
            let rhs = self.binary(p + 1)?;
            lhs = JExpr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<JExpr, Gap> {
        for op in ["!", "-", "+", "~", "++", "--"] {
            if self.is(op) {
                self.pos += 1;
                let e = self.unary()?;
                let op: &'static str = PUNCT.iter().find(|p| **p == op).expect("listed");
                return Ok(JExpr::Unary(op, Box::new(e)));
            }
        }
        // A cast: `(type) expr`, where type is a primitive or a known boxed class.
        if self.is("(")
            && let Some(Tok::Ident(t)) = self.peek_at(1)
            && (PRIMITIVES.contains(&t.as_str()) || BOXED.contains(&t.as_str()))
            && matches!(self.peek_at(2), Some(Tok::P(")")))
        {
            let t = t.clone();
            self.pos += 3;
            let e = self.unary()?;
            return Ok(JExpr::Cast(t, Box::new(e)));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<JExpr, Gap> {
        let mut e = self.primary()?;
        loop {
            if self.eat(".") {
                let name = self.ident()?;
                if self.is("(") {
                    let args = self.args()?;
                    e = JExpr::Call(Some(Box::new(e)), name, args);
                } else {
                    e = JExpr::Field(Box::new(e), name);
                }
            } else if self.eat("[") {
                let i = self.expr()?;
                self.expect("]")?;
                e = JExpr::Index(Box::new(e), Box::new(i));
            } else if self.eat("++") {
                e = JExpr::Postfix("++", Box::new(e));
            } else if self.eat("--") {
                e = JExpr::Postfix("--", Box::new(e));
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn args(&mut self) -> Result<Vec<JExpr>, Gap> {
        self.expect("(")?;
        let mut out = Vec::new();
        if self.eat(")") {
            return Ok(out);
        }
        loop {
            out.push(self.expr()?);
            if self.eat(")") {
                return Ok(out);
            }
            self.expect(",")?;
        }
    }

    fn primary(&mut self) -> Result<JExpr, Gap> {
        match self.peek().cloned() {
            Some(Tok::Int(i)) => {
                self.pos += 1;
                Ok(JExpr::Int(i))
            }
            Some(Tok::Float(f)) => {
                self.pos += 1;
                Ok(JExpr::Float(f))
            }
            Some(Tok::Str(s)) => {
                self.pos += 1;
                Ok(JExpr::Str(s))
            }
            Some(Tok::Char(c)) => {
                self.pos += 1;
                Ok(JExpr::Char(c))
            }
            Some(Tok::P("(")) => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                match name.as_str() {
                    "true" => Ok(JExpr::Bool(true)),
                    "false" => Ok(JExpr::Bool(false)),
                    "null" => Ok(JExpr::Null),
                    "new" => {
                        let t = self.type_name()?;
                        if self.is("{") {
                            return Err(Gap::new(format!("array initializer of {t}")));
                        }
                        let args = if self.is("(") {
                            self.args()?
                        } else {
                            Vec::new()
                        };
                        if self.is("{") {
                            return Err(Gap::new(format!("anonymous class of {t}")));
                        }
                        Ok(JExpr::New(t, args))
                    }
                    _ if self.is("(") => {
                        let args = self.args()?;
                        Ok(JExpr::Call(None, name, args))
                    }
                    _ => Ok(JExpr::Name(name)),
                }
            }
            _ => Err(Gap::new(format!("unexpected {}", self.describe()))),
        }
    }
}

/// Parses one complete expression.
pub fn parse_expr(src: &str) -> Result<JExpr, Gap> {
    let mut p = Parser::new(src)?;
    let e = p.expr()?;
    if !p.at_end() {
        return Err(Gap::new(format!(
            "unexpected {} after the expression",
            p.describe()
        )));
    }
    Ok(e)
}

/// The Java static type of a lowered expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JType {
    Str,
    /// A primitive or boxed long/int.
    Long {
        boxed: bool,
    },
    Double {
        boxed: bool,
    },
    BigDecimal,
    Date,
    Bool {
        boxed: bool,
    },
    Null,
    Unknown,
}

impl JType {
    /// The Java type Kettle hands Java code for a field of type `t`.
    pub fn of_field(t: KType) -> JType {
        match t {
            KType::String => JType::Str,
            KType::Integer => JType::Long { boxed: true },
            KType::Number => JType::Double { boxed: true },
            KType::BigNumber => JType::BigDecimal,
            KType::Date | KType::Timestamp => JType::Date,
            KType::Boolean => JType::Bool { boxed: true },
            KType::Binary | KType::Unknown => JType::Unknown,
        }
    }

    pub fn numeric(self) -> bool {
        matches!(self, JType::Long { .. } | JType::Double { .. })
    }

    pub fn boxed(self) -> bool {
        matches!(
            self,
            JType::Long { boxed: true }
                | JType::Double { boxed: true }
                | JType::Bool { boxed: true }
                | JType::Str
                | JType::BigDecimal
                | JType::Date
        )
    }

    pub fn ktype(self) -> KType {
        match self {
            JType::Str => KType::String,
            JType::Long { .. } => KType::Integer,
            JType::Double { .. } => KType::Number,
            JType::BigDecimal => KType::BigNumber,
            JType::Date => KType::Date,
            JType::Bool { .. } => KType::Boolean,
            JType::Null | JType::Unknown => KType::Unknown,
        }
    }
}
