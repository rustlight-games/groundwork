//! Loosely-typed parameter bags for effect primitives.
//!
//! Each effect kind reads the parameters it cares about. A `damage` handler
//! wants `amount`; a `knockback` handler wants `force`. Rather than a giant
//! enum with a variant per kind — which would put every spell's schema in one
//! file and defeat the point of the plugin split — handlers declare their own
//! requirements by reading from a [`Params`] bag and reporting a typed error.
//!
//! Accessors return [`ContentResult`] rather than panicking or defaulting,
//! because these are read during validation at load time, where the useful
//! outcome is a message naming the file and key rather than a crash.

use bw_core::Real;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::error::{ContentError, ContentResult};

/// A single authored value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
    Int(i64),
    /// Authored as a decimal literal, converted to [`Real`] on read.
    Num(f64),
    Text(SmolStr),
    List(Vec<Value>),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "a bool",
            Value::Int(_) => "an integer",
            Value::Num(_) => "a number",
            Value::Text(_) => "a string",
            Value::List(_) => "a list",
        }
    }
}

/// An ordered map of parameters.
///
/// Ordered because validation errors should come out in the order the author
/// wrote them, and because iteration feeding into a content hash must be stable.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Params(IndexMap<SmolStr, Value>);

impl Params {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: impl Into<SmolStr>, value: Value) -> &mut Self {
        self.0.insert(key.into(), value);
        self
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    fn require<'a>(&'a self, context: &str, key: &str) -> ContentResult<&'a Value> {
        self.0.get(key).ok_or_else(|| ContentError::MissingParam {
            context: SmolStr::new(context),
            param: SmolStr::new(key),
        })
    }

    fn wrong(context: &str, key: &str, expected: &'static str, found: &Value) -> ContentError {
        ContentError::WrongParamType {
            context: SmolStr::new(context),
            param: SmolStr::new(key),
            expected,
            found: found.type_name(),
        }
    }

    /// A required numeric parameter, converted to fixed point.
    ///
    /// Accepts an integer too, so `amount: 10` and `amount: 10.0` both work —
    /// authors should not have to remember which one a given field wants.
    pub fn real(&self, context: &str, key: &str) -> ContentResult<Real> {
        match self.require(context, key)? {
            Value::Num(n) => Ok(Real::from_num(*n)),
            Value::Int(i) => Ok(Real::from_num(*i)),
            other => Err(Self::wrong(context, key, "a number", other)),
        }
    }

    pub fn real_or(&self, context: &str, key: &str, default: Real) -> ContentResult<Real> {
        if self.contains(key) {
            self.real(context, key)
        } else {
            Ok(default)
        }
    }

    pub fn int(&self, context: &str, key: &str) -> ContentResult<i64> {
        match self.require(context, key)? {
            Value::Int(i) => Ok(*i),
            other => Err(Self::wrong(context, key, "an integer", other)),
        }
    }

    pub fn int_or(&self, context: &str, key: &str, default: i64) -> ContentResult<i64> {
        if self.contains(key) {
            self.int(context, key)
        } else {
            Ok(default)
        }
    }

    pub fn bool_or(&self, context: &str, key: &str, default: bool) -> ContentResult<bool> {
        match self.0.get(key) {
            None => Ok(default),
            Some(Value::Bool(b)) => Ok(*b),
            Some(other) => Err(Self::wrong(context, key, "a bool", other)),
        }
    }

    pub fn text(&self, context: &str, key: &str) -> ContentResult<&str> {
        match self.require(context, key)? {
            Value::Text(s) => Ok(s.as_str()),
            other => Err(Self::wrong(context, key, "a string", other)),
        }
    }

    pub fn text_or<'a>(
        &'a self,
        context: &str,
        key: &str,
        default: &'a str,
    ) -> ContentResult<&'a str> {
        if self.contains(key) {
            self.text(context, key)
        } else {
            Ok(default)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Params {
        let mut p = Params::new();
        p.insert("amount", Value::Num(12.5))
            .insert("count", Value::Int(3))
            .insert("piercing", Value::Bool(true))
            .insert("school", Value::Text(SmolStr::new("fire")));
        p
    }

    #[test]
    fn reads_typed_values() {
        let p = params();
        assert_eq!(p.real("t", "amount").unwrap(), Real::from_num(12.5));
        assert_eq!(p.int("t", "count").unwrap(), 3);
        assert!(p.bool_or("t", "piercing", false).unwrap());
        assert_eq!(p.text("t", "school").unwrap(), "fire");
    }

    #[test]
    fn integers_are_accepted_where_a_number_is_wanted() {
        // Authors should not have to remember that `amount: 10` needs a `.0`.
        let mut p = Params::new();
        p.insert("amount", Value::Int(10));
        assert_eq!(p.real("t", "amount").unwrap(), Real::from_num(10));
    }

    #[test]
    fn missing_required_param_names_itself() {
        let e = Params::new().real("fireball", "amount").unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("fireball"), "{msg}");
        assert!(msg.contains("amount"), "{msg}");
    }

    #[test]
    fn wrong_type_reports_both_expected_and_found() {
        let e = params().int("t", "school").unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("an integer"), "{msg}");
        assert!(msg.contains("a string"), "{msg}");
    }

    #[test]
    fn defaults_apply_only_when_absent() {
        let p = params();
        assert_eq!(p.int_or("t", "count", 99).unwrap(), 3);
        assert_eq!(p.int_or("t", "absent", 99).unwrap(), 99);
    }

    #[test]
    fn insertion_order_is_preserved() {
        let keys: Vec<_> = params().iter().map(|(k, _)| k.to_string()).collect();
        assert_eq!(keys, ["amount", "count", "piercing", "school"]);
    }
}
