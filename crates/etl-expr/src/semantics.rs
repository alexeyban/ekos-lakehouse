//! Fidelity labels and the Kettle rules RFC 0202 makes explicit.

use crate::expr::Expr;
use serde::{Deserialize, Serialize};

/// How faithfully a step (or an expression) was carried over (RFC 0200 §2). Ordered from the
/// strongest to the weakest, so `max` is the weakest of several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fidelity {
    Exact,
    Rewritten,
    LlmDrafted,
    Gap,
}

impl Fidelity {
    pub fn as_str(self) -> &'static str {
        match self {
            Fidelity::Exact => "exact",
            Fidelity::Rewritten => "rewritten",
            Fidelity::LlmDrafted => "llm-drafted",
            Fidelity::Gap => "gap",
        }
    }
}

/// Why something could not be translated: the text that stopped it, in words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    pub reason: String,
}

impl Gap {
    pub fn new(reason: impl Into<String>) -> Gap {
        Gap {
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for Gap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

/// A translated expression with the rules applied to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lowered {
    pub expr: Expr,
    pub fidelity: Fidelity,
    /// Every rule below `exact`, in words a reviewer can check.
    pub notes: Vec<String>,
}

impl Lowered {
    pub fn exact(expr: Expr) -> Lowered {
        Lowered {
            expr,
            fidelity: Fidelity::Exact,
            notes: Vec::new(),
        }
    }

    pub fn rewritten(expr: Expr, note: impl Into<String>) -> Lowered {
        Lowered {
            expr,
            fidelity: Fidelity::Rewritten,
            notes: vec![note.into()],
        }
    }

    /// Records `note` and lowers the label to at least `rewritten`.
    pub fn note(&mut self, note: impl Into<String>) {
        let note = note.into();
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
        self.fidelity = self.fidelity.max(Fidelity::Rewritten);
    }

    /// Takes over `other`'s label and notes (for an expression built from sub-expressions).
    pub fn absorb(&mut self, other: &Lowered) {
        self.fidelity = self.fidelity.max(other.fidelity);
        for n in &other.notes {
            if !self.notes.contains(n) {
                self.notes.push(n.clone());
            }
        }
    }
}
