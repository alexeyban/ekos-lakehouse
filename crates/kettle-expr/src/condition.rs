//! Kettle `<condition>` trees (Filter rows and every step that embeds one) → a two-valued
//! predicate. A condition list is evaluated **left to right with no precedence**
//! (`a OR b AND c` is `(a OR b) AND c`), and each atom is never null, so `NOT` behaves as in
//! Kettle.

use crate::expr::{BinOp, Expr, Func, Lit};
use crate::semantics::{
    Gap, Lowered, NOTE_NULL_COMPARE, NOTE_REGEX_FULL, full_match, kettle_compare, nullable,
};
use crate::types::KType;
use ekos_lakehouse_kettle::xml::Node;

/// Kettle's default date mask for a typed constant without one.
pub const DEFAULT_DATE_MASK: &str = "yyyy/MM/dd HH:mm:ss.SSS";

/// Lowers a `<condition>` element.
pub fn lower(cond: &Node, col_type: &dyn Fn(&str) -> KType) -> Result<Lowered, Gap> {
    let negated = cond.flag_at("negated", false);
    let children: Vec<&Node> = cond
        .child("conditions")
        .map(|c| c.children_named("condition").collect())
        .unwrap_or_default();
    let mut out = if children.is_empty() {
        atom(cond, col_type)?
    } else {
        let mut acc = lower(children[0], col_type)?;
        // Left to right, no precedence: the first condition's operator is ignored.
        for child in &children[1..] {
            let next = lower(child, col_type)?;
            let op = child.text_at("operator").unwrap_or("AND");
            acc.expr = combine(op, acc.expr, next.expr.clone())?;
            acc.absorb(&next);
        }
        acc
    };
    if negated {
        out.expr = Expr::negate(out.expr);
    }
    Ok(out)
}

fn combine(op: &str, l: Expr, r: Expr) -> Result<Expr, Gap> {
    Ok(match op.trim().to_ascii_uppercase().as_str() {
        "AND" | "" => Expr::and(l, r),
        "OR" => Expr::or(l, r),
        "AND NOT" => Expr::and(l, Expr::negate(r)),
        "OR NOT" => Expr::or(l, Expr::negate(r)),
        "XOR" => Expr::and(
            Expr::or(l.clone(), r.clone()),
            Expr::negate(Expr::and(l, r)),
        ),
        other => return Err(Gap::new(format!("condition operator `{other}`"))),
    })
}

/// A typed constant from a `<value>` element.
pub fn constant(value: &Node, as_type: KType) -> Result<Expr, Gap> {
    if value.flag_at("isnull", false) {
        return Ok(Expr::null());
    }
    let declared = value.text_at("type").map(KType::parse).unwrap_or(as_type);
    let text = value.at("text").map(|t| t.text.as_str()).unwrap_or("");
    literal(text, declared, value.text_at("mask"))
}

/// A literal of type `t` from Kettle's text form.
pub fn literal(text: &str, t: KType, mask: Option<&str>) -> Result<Expr, Gap> {
    let trimmed = text.trim();
    Ok(match t {
        KType::Integer => match trimmed.parse::<i64>() {
            Ok(i) => Expr::int(i),
            Err(_) => return Err(Gap::new(format!("integer constant `{text}`"))),
        },
        KType::Number | KType::BigNumber => {
            if trimmed.parse::<f64>().is_ok() && !trimmed.contains(['e', 'E']) {
                Expr::num(trimmed)
            } else {
                return Err(Gap::new(format!(
                    "number constant `{text}` is not a plain decimal (mask {})",
                    mask.unwrap_or("none")
                )));
            }
        }
        KType::Boolean => match trimmed {
            "Y" | "y" | "true" | "TRUE" | "True" => Expr::bool(true),
            "N" | "n" | "false" | "FALSE" | "False" => Expr::bool(false),
            _ => return Err(Gap::new(format!("boolean constant `{text}`"))),
        },
        KType::Date | KType::Timestamp => Expr::call(
            Func::ParseDatetime,
            vec![
                Expr::str(trimmed),
                Expr::str(mask.unwrap_or(DEFAULT_DATE_MASK)),
            ],
        ),
        // An empty string is null in Kettle.
        KType::String | KType::Unknown if text.is_empty() => Expr::null(),
        KType::String | KType::Unknown => Expr::str(text),
        KType::Binary => return Err(Gap::new("binary constant")),
    })
}

fn atom(c: &Node, col_type: &dyn Fn(&str) -> KType) -> Result<Lowered, Gap> {
    let Some(left_name) = c.text_at("leftvalue") else {
        // An empty condition lets every row through.
        return Ok(Lowered::exact(Expr::bool(true)));
    };
    let left = Expr::col(left_name);
    let lt = col_type(left_name);
    let func = c.text_at("function").unwrap_or("=").to_ascii_uppercase();
    let right = || -> Result<Expr, Gap> {
        if let Some(r) = c.text_at("rightvalue") {
            Ok(Expr::col(r))
        } else if let Some(v) = c.child("value") {
            constant(v, lt)
        } else {
            Ok(Expr::null())
        }
    };
    let two = |e: Expr| Expr::two_valued(e);
    let mut out = match func.as_str() {
        "=" | "<>" | "<" | "<=" | ">" | ">=" => {
            let op = match func.as_str() {
                "=" => BinOp::Eq,
                "<>" => BinOp::NotEq,
                "<" => BinOp::Lt,
                "<=" => BinOp::LtEq,
                ">" => BinOp::Gt,
                _ => BinOp::GtEq,
            };
            let r = right()?;
            let r_null = nullable(&r);
            let e = kettle_compare(op, left, r, true, r_null);
            Lowered::rewritten(e, NOTE_NULL_COMPARE)
        }
        "IS NULL" => Lowered::exact(Expr::is_null(left)),
        "IS NOT NULL" => Lowered::exact(Expr::is_not_null(left)),
        "TRUE" => Lowered::exact(Expr::bool(true)),
        "IN LIST" => {
            let text = c
                .child("value")
                .and_then(|v| v.at("text"))
                .map(|t| t.text.clone())
                .or_else(|| c.text_at("rightvalue").map(str::to_string))
                .unwrap_or_default();
            let item_type = if lt == KType::Unknown {
                KType::String
            } else {
                lt
            };
            let list = text
                .split(';')
                .map(|item| literal(item, item_type, None))
                .collect::<Result<Vec<_>, _>>()?;
            Lowered::exact(two(Expr::InList {
                arg: Box::new(left),
                list,
                negated: false,
            }))
        }
        "CONTAINS" => Lowered::exact(two(Expr::bin(
            BinOp::Gt,
            Expr::call(Func::Strpos, vec![left, right()?]),
            Expr::int(0),
        ))),
        "STARTS WITH" => Lowered::exact(two(Expr::call(Func::StartsWith, vec![left, right()?]))),
        "ENDS WITH" => {
            let r = right()?;
            let tail = Expr::call(
                Func::Substr,
                vec![
                    left,
                    Expr::Unary {
                        op: crate::expr::UnOp::Neg,
                        arg: Box::new(Expr::call(Func::Length, vec![r.clone()])),
                    },
                ],
            );
            Lowered::exact(two(Expr::bin(BinOp::Eq, tail, r)))
        }
        "LIKE" => Lowered::exact(two(Expr::Like {
            arg: Box::new(left),
            pattern: Box::new(right()?),
        })),
        "REGEXP" => {
            let pattern = match right()? {
                Expr::Lit { value: Lit::Str(p) } => Expr::str(&full_match(&p)),
                other => Expr::call(
                    Func::Concat,
                    vec![Expr::str("^(?:"), other, Expr::str(")$")],
                ),
            };
            Lowered::rewritten(
                two(Expr::call(Func::RegexpLike, vec![left, pattern])),
                NOTE_REGEX_FULL,
            )
        }
        other => return Err(Gap::new(format!("condition function `{other}`"))),
    };
    if lt == KType::Unknown && !matches!(func.as_str(), "IS NULL" | "IS NOT NULL" | "TRUE") {
        out.note(format!(
            "the type of `{left_name}` is not known here; compared as written"
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> Node {
        let doc = roxmltree::Document::parse(xml).unwrap();
        Node::from_roxml(&doc, doc.root_element())
    }

    fn types(c: &str) -> KType {
        match c {
            "id" => KType::Integer,
            _ => KType::String,
        }
    }

    #[test]
    fn a_list_is_evaluated_left_to_right_without_precedence() {
        let xml = r#"<condition><negated>N</negated><conditions>
            <condition><negated>N</negated><leftvalue>id</leftvalue><function>&lt;=</function>
              <value><name>constant</name><type>Integer</type><text>5</text><isnull>N</isnull></value></condition>
            <condition><negated>N</negated><operator>OR</operator><leftvalue>name</leftvalue><function>IS NULL</function></condition>
            <condition><negated>Y</negated><operator>AND</operator><leftvalue>name</leftvalue><function>IN LIST</function>
              <value><type>String</type><text>a;b</text></value></condition>
          </conditions></condition>"#;
        let l = lower(&parse(xml), &types).unwrap();
        assert_eq!(
            l.expr.sql(),
            "((CASE WHEN (\"id\" IS NULL) THEN TRUE ELSE (\"id\" <= 5) END OR (\"name\" IS NULL)) \
             AND NOT (coalesce((\"name\" IN ('a', 'b')), FALSE)))"
        );
        assert_eq!(l.fidelity, crate::semantics::Fidelity::Rewritten);
    }

    #[test]
    fn regex_is_anchored_and_dates_keep_their_mask() {
        let xml = r#"<condition><leftvalue>code</leftvalue><function>REGEXP</function>
            <value><type>String</type><text>[A-Z]+</text></value></condition>"#;
        assert_eq!(
            lower(&parse(xml), &types).unwrap().expr.sql(),
            "coalesce(regexp_like(\"code\", '^(?:[A-Z]+)$'), FALSE)"
        );
        let d = literal("2020/01/31 00:00:00.000", KType::Date, None).unwrap();
        assert_eq!(
            d.sql(),
            "parse_datetime('2020/01/31 00:00:00.000', 'yyyy/MM/dd HH:mm:ss.SSS')"
        );
        assert_eq!(literal("", KType::String, None).unwrap(), Expr::null());
    }

    #[test]
    fn an_empty_condition_passes_everything_and_unknown_functions_are_gaps() {
        assert_eq!(
            lower(&parse("<condition/>"), &types).unwrap().expr,
            Expr::bool(true)
        );
        let bad =
            r#"<condition><leftvalue>a</leftvalue><function>SOUNDS LIKE</function></condition>"#;
        assert!(lower(&parse(bad), &types).is_err());
    }
}
