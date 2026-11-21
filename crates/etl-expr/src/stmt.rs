//! The statement subset of Java and JavaScript that code steps use (RFC 0203): declarations,
//! assignments, `if`/`else`, blocks, expression statements, `return`. Anything else is a
//! [`Stmt::Unsupported`] naming the construct, so the executor can stop with a precise reason.

use crate::java::{JExpr, Parser, Tok};
use crate::semantics::Gap;

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `var a = e, b;` / `String a = e;` — names and initializers.
    Decl(Vec<(String, Option<JExpr>)>),
    Expr(JExpr),
    If(JExpr, Vec<Stmt>, Vec<Stmt>),
    Return(Option<JExpr>),
    Block(Vec<Stmt>),
    /// A construct outside the subset, with its keyword.
    Unsupported(String),
}

const UNSUPPORTED: &[&str] = &[
    "for",
    "while",
    "do",
    "switch",
    "try",
    "throw",
    "function",
    "class",
    "break",
    "continue",
    "synchronized",
    "case",
    "default",
    "with",
    "label",
];
const PRIMITIVES: &[&str] = &[
    "int", "long", "double", "float", "short", "byte", "char", "boolean",
];
const MODIFIERS: &[&str] = &[
    "final",
    "static",
    "private",
    "public",
    "protected",
    "transient",
    "volatile",
];

/// Parses statements until the end of input or an unmatched `}`.
pub fn parse_statements(p: &mut Parser, js: bool) -> Result<Vec<Stmt>, Gap> {
    let mut out = Vec::new();
    while !p.at_end() && !p.is("}") {
        out.push(statement(p, js)?);
    }
    Ok(out)
}

/// Parses a whole script.
pub fn parse_script(src: &str, js: bool) -> Result<Vec<Stmt>, Gap> {
    let mut p = Parser::new_with(src, js)?;
    let s = parse_statements(&mut p, js)?;
    if !p.at_end() {
        return Err(Gap::new(format!("unexpected {}", p.describe())));
    }
    Ok(s)
}

/// Skips a balanced `(…)` or `{…}` starting at the current token.
fn skip_balanced(p: &mut Parser) {
    let (open, close) = if p.is("(") { ("(", ")") } else { ("{", "}") };
    let mut depth = 0;
    while !p.at_end() {
        if p.is(open) {
            depth += 1;
        } else if p.is(close) {
            depth -= 1;
            if depth == 0 {
                p.pos += 1;
                return;
            }
        }
        p.pos += 1;
    }
}

/// Whether the tokens at the cursor start a Java local declaration (`Type name …`).
fn java_decl_ahead(p: &Parser) -> bool {
    let mut i = 0;
    while matches!(p.peek_at(i), Some(Tok::Ident(m)) if MODIFIERS.contains(&m.as_str())) {
        i += 1;
    }
    let Some(Tok::Ident(t)) = p.peek_at(i) else {
        return false;
    };
    if PRIMITIVES.contains(&t.as_str()) {
        return true;
    }
    if matches!(
        t.as_str(),
        "return" | "new" | "this" | "super" | "null" | "true" | "false"
    ) {
        return false;
    }
    // Type[.Type]*[<…>][[]]* name
    i += 1;
    while let (Some(Tok::P(".")), Some(Tok::Ident(_))) = (p.peek_at(i), p.peek_at(i + 1)) {
        i += 2;
    }
    if matches!(p.peek_at(i), Some(Tok::P("<"))) {
        let mut depth = 0;
        while let Some(t) = p.peek_at(i) {
            match t {
                Tok::P("<") => depth += 1,
                Tok::P(">") => depth -= 1,
                Tok::P(">>") => depth -= 2,
                _ => {}
            }
            i += 1;
            if depth <= 0 {
                break;
            }
        }
    }
    while matches!(
        (p.peek_at(i), p.peek_at(i + 1)),
        (Some(Tok::P("[")), Some(Tok::P("]")))
    ) {
        i += 2;
    }
    matches!(p.peek_at(i), Some(Tok::Ident(_)))
        && matches!(p.peek_at(i + 1), Some(Tok::P("=" | ";" | ",")))
}

fn declarators(p: &mut Parser) -> Result<Vec<(String, Option<JExpr>)>, Gap> {
    let mut out = Vec::new();
    loop {
        let name = p.ident()?;
        let init = if p.eat("=") { Some(p.expr()?) } else { None };
        out.push((name, init));
        if !p.eat(",") {
            break;
        }
    }
    p.eat(";");
    Ok(out)
}

fn block_or_statement(p: &mut Parser, js: bool) -> Result<Vec<Stmt>, Gap> {
    if p.eat("{") {
        let s = parse_statements(p, js)?;
        p.expect("}")?;
        Ok(s)
    } else {
        Ok(vec![statement(p, js)?])
    }
}

fn statement(p: &mut Parser, js: bool) -> Result<Stmt, Gap> {
    if p.eat(";") {
        return Ok(Stmt::Block(vec![]));
    }
    if p.eat("{") {
        let s = parse_statements(p, js)?;
        p.expect("}")?;
        return Ok(Stmt::Block(s));
    }
    if let Some(Tok::Ident(kw)) = p.peek().cloned() {
        if UNSUPPORTED.contains(&kw.as_str()) {
            // Consume it so the caller's position stays sane; the executor stops on it anyway.
            p.pos += 1;
            if p.is("(") {
                skip_balanced(p);
            }
            if p.is("{") {
                skip_balanced(p);
            }
            // The tails that belong to the construct: `catch`/`finally`, `do … while (…)`.
            loop {
                if p.is_ident("catch")
                    || p.is_ident("finally")
                    || (kw == "do" && p.is_ident("while"))
                {
                    p.pos += 1;
                    if p.is("(") {
                        skip_balanced(p);
                    }
                    if p.is("{") {
                        skip_balanced(p);
                    }
                    p.eat(";");
                } else {
                    break;
                }
            }
            return Ok(Stmt::Unsupported(kw));
        }
        match kw.as_str() {
            "if" => {
                p.pos += 1;
                p.expect("(")?;
                let c = p.expr()?;
                p.expect(")")?;
                let then = block_or_statement(p, js)?;
                let otherwise = if p.is_ident("else") {
                    p.pos += 1;
                    block_or_statement(p, js)?
                } else {
                    vec![]
                };
                return Ok(Stmt::If(c, then, otherwise));
            }
            "return" => {
                p.pos += 1;
                let e = if p.is(";") || p.is("}") || p.at_end() {
                    None
                } else {
                    Some(p.expr()?)
                };
                p.eat(";");
                return Ok(Stmt::Return(e));
            }
            "var" | "let" | "const" if js => {
                p.pos += 1;
                return Ok(Stmt::Decl(declarators(p)?));
            }
            _ if !js && java_decl_ahead(p) => {
                while matches!(p.peek(), Some(Tok::Ident(m)) if MODIFIERS.contains(&m.as_str())) {
                    p.pos += 1;
                }
                p.type_name()?;
                return Ok(Stmt::Decl(declarators(p)?));
            }
            _ => {}
        }
    }
    let e = p.expr()?;
    if !p.eat(";") && !(js && (p.is("}") || p.at_end() || starts_statement(p))) {
        return Err(Gap::new(format!("expected `;`, found {}", p.describe())));
    }
    Ok(Stmt::Expr(e))
}

/// JavaScript's automatic semicolon insertion, as far as scripts need it: a new statement on
/// the next token.
fn starts_statement(p: &Parser) -> bool {
    matches!(p.peek(), Some(Tok::Ident(k)) if matches!(k.as_str(), "var" | "let" | "const" | "if" | "return"))
        || matches!(p.peek(), Some(Tok::Ident(_)))
}

/// A method of a Java class body: its name and statements. Fields and other members are listed
/// by name so the executor can say a method relies on them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClassBody {
    pub methods: Vec<(String, Vec<Stmt>)>,
    pub fields: Vec<String>,
    pub imports: Vec<String>,
}

/// Parses a User Defined Java Class body (or a decompiled class): imports, fields, methods.
pub fn parse_class_body(src: &str) -> Result<ClassBody, Gap> {
    let mut p = Parser::new(src)?;
    let mut body = ClassBody::default();
    while !p.at_end() {
        if p.is_ident("import") || p.is_ident("package") {
            let start = p.pos;
            while !p.at_end() && !p.eat(";") {
                p.pos += 1;
            }
            let text: Vec<String> = p.toks[start + 1..p.pos.saturating_sub(1)]
                .iter()
                .map(|t| match t {
                    Tok::Ident(s) => s.clone(),
                    Tok::P(s) => s.to_string(),
                    _ => String::new(),
                })
                .collect();
            body.imports.push(text.concat());
            continue;
        }
        if p.eat("@") {
            p.ident()?;
            if p.is("(") {
                skip_balanced(&mut p);
            }
            continue;
        }
        // `[modifiers] Type name (` = a method; `[modifiers] Type name [= …];` = a field.
        while matches!(p.peek(), Some(Tok::Ident(m)) if MODIFIERS.contains(&m.as_str()) || m == "abstract")
        {
            p.pos += 1;
        }
        if p.is_ident("class") || p.is_ident("interface") || p.is_ident("enum") {
            return Err(Gap::new("a nested class in the step's code"));
        }
        if p.at_end() {
            break;
        }
        p.type_name()?;
        let name = p.ident()?;
        if p.is("(") {
            skip_balanced(&mut p);
            if p.is_ident("throws") {
                p.pos += 1;
                while !p.at_end() && !p.is("{") {
                    p.pos += 1;
                }
            }
            p.expect("{")?;
            let stmts = parse_statements(&mut p, false)?;
            p.expect("}")?;
            body.methods.push((name, stmts));
        } else {
            body.fields.push(name);
            while !p.at_end() && !p.eat(";") {
                if p.is("{") || p.is("(") {
                    skip_balanced(&mut p);
                } else {
                    p.pos += 1;
                }
            }
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_javascript_transform_script() {
        let s = parse_script(
            "var full = trim(first) + ' ' + trim(last);\nvar vip;\nif (amount > 1000) { vip = 'Y' } else vip = 'N'\nif (status === 'void') trans_Status = SKIP_TRANSFORMATION;",
            true,
        )
        .unwrap();
        assert_eq!(s.len(), 4);
        assert!(matches!(&s[2], Stmt::If(_, t, e) if t.len() == 1 && e.len() == 1));
        let s = parse_script("for (i = 0; i < 3; i++) { x += i; }\nvar y = 1;", true).unwrap();
        assert_eq!(s[0], Stmt::Unsupported("for".into()));
        let s = parse_script(
            "try { a = 1; } catch (e) { b = 2; } finally { c = 3; }\nvar y = 1;",
            true,
        )
        .unwrap();
        assert_eq!(
            (s[0].clone(), s.len()),
            (Stmt::Unsupported("try".into()), 2)
        );
    }

    #[test]
    fn parses_a_user_defined_java_class() {
        let src = r#"import org.pentaho.di.core.*;
            private int counter = 0;
            public boolean processRow(StepMetaInterface smi, StepDataInterface sdi) throws KettleException {
                Object[] r = getRow();
                if (r == null) { setOutputDone(); return false; }
                r = createOutputRow(r, data.outputRowMeta.size());
                String name = get(Fields.In, "name").getString(r);
                final Long qty = get(Fields.In, "qty").getInteger(r);
                java.util.List<String> xs = null;
                get(Fields.Out, "label").setValue(r, name + ":" + qty);
                putRow(data.outputRowMeta, r);
                return true;
            }"#;
        let b = parse_class_body(src).unwrap();
        assert_eq!(b.fields, vec!["counter".to_string()]);
        let (name, stmts) = &b.methods[0];
        assert_eq!(name, "processRow");
        assert!(matches!(&stmts[0], Stmt::Decl(d) if d[0].0 == "r"));
        assert!(matches!(&stmts[3], Stmt::Decl(d) if d[0].0 == "name"));
        assert!(matches!(&stmts[4], Stmt::Decl(d) if d[0].0 == "qty"));
        assert!(matches!(&stmts[5], Stmt::Decl(d) if d[0].0 == "xs"));
        assert_eq!(stmts.len(), 9);
        assert_eq!(b.imports, vec!["org.pentaho.di.core.*".to_string()]);
    }
}
