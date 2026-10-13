//! Parameter panel model (upstream Project.swift `setParameter` +
//! ProjectOutput.swift `orderedParameters`): the manifest's `parameters`
//! map ordered by first appearance in model.py, and the surgical rewrite
//! of one parameter's numeric literal in the script, written back with
//! [`super::store::atomic_write`].
//!
//! One deliberate divergence from Swift's `String(Double)`: beyond ~1e16
//! Swift switches to exponent notation ("1e+16") while Rust's `{}` always
//! writes the number out. A written-out literal is still valid Python, and
//! slider commits (3-decimal rounding) cannot reach that magnitude. The
//! same reasoning covers `value as i64` for integral values beyond i64
//! range: unreachable from the UI, and Rust's saturating cast at least
//! never traps the way Swift's `Int(Double)` would.

use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;

/// One row of the parameter panel: a manifest parameter with the value
/// the slider currently shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelParameter {
    pub name: String,
    pub value: f64,
}

/// Manifest parameters ordered by the first index of the name as a plain
/// substring in model.py (missing → end of script, i.e. last), ties broken
/// alphabetically — the order the parameters appear in the user's script.
pub fn ordered_parameters(
    manifest: &BTreeMap<String, f64>,
    script: &str,
) -> Vec<ModelParameter> {
    let mut rows: Vec<ModelParameter> = manifest
        .iter()
        .map(|(name, value)| ModelParameter { name: name.clone(), value: *value })
        .collect();
    rows.sort_by(|a, b| {
        let a_at = script.find(a.name.as_str()).unwrap_or(script.len());
        let b_at = script.find(b.name.as_str()).unwrap_or(script.len());
        a_at.cmp(&b_at).then_with(|| a.name.cmp(&b.name))
    });
    rows
}

#[derive(Debug, thiserror::Error)]
pub enum ParameterError {
    #[error("parameter {name:?} not found in model.py")]
    NotFound { name: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The result of a rewrite: the full new script and the exact literal
/// written in place of the captured number (the caller builds the version
/// prompt `"{name} = {literal}"`).
#[derive(Debug, Clone, PartialEq)]
pub struct Rewrite {
    pub script: String,
    pub literal: String,
}

/// Integer values keep the integer look when the original literal had no
/// decimal point (upstream `Int(value)`, which truncates toward zero —
/// equivalent here because the value is already integral); anything else
/// gets Rust's shortest round-trip form.
pub fn format_number(value: f64, original: &str) -> String {
    if value == value.trunc() && !original.contains('.') {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Rewrite the first assignment to `name` in `script`, replacing only the
/// captured numeric literal. The pattern is upstream Project.swift:346
/// verbatim; `regex::escape` stands in for
/// `NSRegularExpression.escapedPattern`.
pub fn rewrite_value(script: &str, name: &str, value: f64) -> Result<Rewrite, ParameterError> {
    let pattern = format!(
        r"(?m)^\s*{}\s*(?::[^=\n]+)?=\s*([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)",
        regex::escape(name)
    );
    // Escaping makes every name a valid pattern, so construction failure is
    // as unreachable as upstream's `try?` path — and lands the same way:
    // the parameter reads as not found.
    let re = Regex::new(&pattern).map_err(|_| ParameterError::NotFound { name: name.to_owned() })?;
    let number = re
        .captures(script)
        .and_then(|caps| caps.get(1))
        .ok_or_else(|| ParameterError::NotFound { name: name.to_owned() })?;
    let literal = format_number(value, number.as_str());
    let mut rewritten = String::with_capacity(script.len() + literal.len());
    rewritten.push_str(&script[..number.start()]);
    rewritten.push_str(&literal);
    rewritten.push_str(&script[number.end()..]);
    Ok(Rewrite { script: rewritten, literal })
}

/// Read model.py, rewrite one parameter, and write the script back
/// atomically. Nothing is written when the parameter is not found.
pub fn set_parameter(project_root: &Path, name: &str, value: f64) -> Result<Rewrite, ParameterError> {
    let script = std::fs::read_to_string(project_root.join("model.py"))?;
    let rewrite = rewrite_value(&script, name, value)?;
    super::store::atomic_write(&project_root.join("model.py"), rewrite.script.as_bytes())?;
    Ok(rewrite)
}
