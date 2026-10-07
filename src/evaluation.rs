//! Local override evaluation, matching the server's evaluator so that values
//! previewed in the Replane UI are the values the SDK returns.

use std::borrow::Cow;
use std::cmp::Ordering;

use serde_json::Value;

use crate::context::{Context, ContextValue};
use crate::js;
use crate::types::{Condition, Override};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationResult {
    Matched,
    NotMatched,
    /// The context lacks a property the condition needs.
    Unknown,
}

/// Returns the value of the first override whose conditions all match, or `base`.
pub fn evaluate_overrides<'a>(
    base: &'a Value,
    overrides: &'a [Override],
    context: &Context,
) -> &'a Value {
    overrides
        .iter()
        .find(|o| {
            o.conditions
                .iter()
                .all(|c| evaluate_condition(c, context) == EvaluationResult::Matched)
        })
        .map_or(base, |o| &o.value)
}

type Compare = fn(&Js, &Js) -> EvaluationResult;

pub fn evaluate_condition(condition: &Condition, context: &Context) -> EvaluationResult {
    use EvaluationResult::*;

    let (property, expected, compare): (&str, Option<&Value>, Compare) = match condition {
        Condition::And { conditions } => {
            let results: Vec<_> = conditions
                .iter()
                .map(|c| evaluate_condition(c, context))
                .collect();
            return if results.contains(&NotMatched) {
                NotMatched
            } else if results.contains(&Unknown) {
                Unknown
            } else {
                Matched
            };
        }
        Condition::Or { conditions } => {
            let results: Vec<_> = conditions
                .iter()
                .map(|c| evaluate_condition(c, context))
                .collect();
            return if results.contains(&Matched) {
                Matched
            } else if results.contains(&Unknown) {
                Unknown
            } else {
                NotMatched
            };
        }
        Condition::Not { condition } => {
            return match evaluate_condition(condition, context) {
                Matched => NotMatched,
                NotMatched => Matched,
                Unknown => Unknown,
            };
        }
        Condition::Segmentation {
            property,
            from_percentage,
            to_percentage,
            seed,
        } => {
            let value = match context.get(property) {
                None | Some(ContextValue::Null) => return Unknown,
                Some(v) => v,
            };
            let unit = js::fnv1a32_to_unit(&(js_string(value) + seed));
            return matched(unit >= from_percentage / 100.0 && unit < to_percentage / 100.0);
        }
        Condition::Unsupported => return Unknown,
        Condition::Equals { property, value } => (property, value.as_ref(), |c, e| {
            matched(strict_equals(c, e))
        }),
        Condition::In { property, value } => (property, value.as_ref(), |c, e| match e {
            Js::Array(items) => matched(
                items
                    .iter()
                    .any(|i| same_value_zero(c, &cast(Js::of(Some(i)), c))),
            ),
            _ => NotMatched,
        }),
        Condition::NotIn { property, value } => (property, value.as_ref(), |c, e| match e {
            Js::Array(items) => matched(
                !items
                    .iter()
                    .any(|i| same_value_zero(c, &cast(Js::of(Some(i)), c))),
            ),
            _ => NotMatched,
        }),
        Condition::LessThan { property, value } => (property, value.as_ref(), |c, e| {
            compare_ordered(c, e, Ordering::is_lt)
        }),
        Condition::LessThanOrEqual { property, value } => (property, value.as_ref(), |c, e| {
            compare_ordered(c, e, Ordering::is_le)
        }),
        Condition::GreaterThan { property, value } => (property, value.as_ref(), |c, e| {
            compare_ordered(c, e, Ordering::is_gt)
        }),
        Condition::GreaterThanOrEqual { property, value } => (property, value.as_ref(), |c, e| {
            compare_ordered(c, e, Ordering::is_ge)
        }),
    };

    let Some(context_value) = context.get(property) else {
        return Unknown;
    };
    let context_value = Js::from_context(context_value);
    let expected = cast(Js::of(expected), &context_value);
    compare(&context_value, &expected)
}

fn matched(b: bool) -> EvaluationResult {
    if b {
        EvaluationResult::Matched
    } else {
        EvaluationResult::NotMatched
    }
}

/// A JS value as seen by the server's evaluator.
#[derive(Debug, Clone)]
enum Js<'a> {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(Cow<'a, str>),
    Array(&'a [Value]),
    Object,
}

impl<'a> Js<'a> {
    fn of(value: Option<&'a Value>) -> Self {
        match value {
            None => Js::Undefined,
            Some(Value::Null) => Js::Null,
            Some(Value::Bool(b)) => Js::Bool(*b),
            Some(Value::Number(n)) => Js::Number(n.as_f64().unwrap_or(f64::NAN)),
            Some(Value::String(s)) => Js::String(Cow::Borrowed(s)),
            Some(Value::Array(items)) => Js::Array(items),
            Some(Value::Object(_)) => Js::Object,
        }
    }

    fn from_context(value: &'a ContextValue) -> Self {
        match value {
            ContextValue::Null => Js::Null,
            ContextValue::Bool(b) => Js::Bool(*b),
            ContextValue::Number(n) => Js::Number(*n),
            ContextValue::String(s) => Js::String(Cow::Borrowed(s)),
        }
    }
}

/// Casts a condition value to the type of the context value, e.g. `"25"` to `25`.
fn cast<'a>(expected: Js<'a>, context_value: &Js) -> Js<'a> {
    match (context_value, expected) {
        (Js::Number(_), Js::String(s)) => {
            let n = js::string_to_number(&s);
            if n.is_nan() {
                Js::String(s)
            } else {
                Js::Number(n)
            }
        }
        (Js::Bool(_), Js::String(s)) if s == "true" => Js::Bool(true),
        (Js::Bool(_), Js::String(s)) if s == "false" => Js::Bool(false),
        (Js::Bool(_), Js::Number(n)) => Js::Bool(n != 0.0),
        (Js::String(_), Js::Number(n)) => Js::String(Cow::Owned(js::number_to_string(n))),
        (Js::String(_), Js::Bool(b)) => Js::String(Cow::Borrowed(if b { "true" } else { "false" })),
        (_, expected) => expected,
    }
}

/// JS `===` on primitives; arrays and objects never compare equal.
fn strict_equals(a: &Js, b: &Js) -> bool {
    match (a, b) {
        (Js::Null, Js::Null) | (Js::Undefined, Js::Undefined) => true,
        (Js::Bool(a), Js::Bool(b)) => a == b,
        (Js::Number(a), Js::Number(b)) => a == b,
        (Js::String(a), Js::String(b)) => a == b,
        _ => false,
    }
}

/// JS `Array.prototype.includes` equality: like `===`, but NaN equals NaN.
fn same_value_zero(a: &Js, b: &Js) -> bool {
    match (a, b) {
        (Js::Number(a), Js::Number(b)) if a.is_nan() && b.is_nan() => true,
        _ => strict_equals(a, b),
    }
}

/// Numbers compare numerically, strings by UTF-16 code units (like JS); anything else doesn't match.
fn compare_ordered(
    context_value: &Js,
    expected: &Js,
    accept: fn(Ordering) -> bool,
) -> EvaluationResult {
    let ordering = match (context_value, expected) {
        (Js::Number(a), Js::Number(b)) => a.partial_cmp(b),
        (Js::String(a), Js::String(b)) => Some(a.encode_utf16().cmp(b.encode_utf16())),
        _ => None,
    };
    matched(ordering.is_some_and(accept))
}

fn js_string(value: &ContextValue) -> String {
    match value {
        ContextValue::Null => "null".into(),
        ContextValue::Bool(b) => b.to_string(),
        ContextValue::Number(n) => js::number_to_string(*n),
        ContextValue::String(s) => s.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::EvaluationResult::*;
    use super::*;
    use serde_json::json;

    fn cond(raw: Value) -> Condition {
        serde_json::from_value(raw).unwrap()
    }

    fn eval(raw: Value, ctx: &Context) -> EvaluationResult {
        evaluate_condition(&cond(raw), ctx)
    }

    #[test]
    fn equals() {
        let c = json!({"operator": "equals", "property": "env", "value": "production"});
        assert_eq!(
            eval(c.clone(), &Context::from([("env", "production")])),
            Matched
        );
        assert_eq!(
            eval(c.clone(), &Context::from([("env", "staging")])),
            NotMatched
        );
        assert_eq!(eval(c, &Context::new()), Unknown);
    }

    #[test]
    fn equals_casts_to_context_type() {
        let c = |v: Value| json!({"operator": "equals", "property": "p", "value": v});
        assert_eq!(eval(c(json!("42")), &Context::from([("p", 42)])), Matched);
        assert_eq!(eval(c(json!(" 42 ")), &Context::from([("p", 42)])), Matched);
        assert_eq!(eval(c(json!(42)), &Context::from([("p", "42")])), Matched);
        assert_eq!(eval(c(json!(1.5)), &Context::from([("p", "1.5")])), Matched);
        assert_eq!(
            eval(c(json!("true")), &Context::from([("p", true)])),
            Matched
        );
        assert_eq!(eval(c(json!(1)), &Context::from([("p", true)])), Matched);
        assert_eq!(eval(c(json!(0)), &Context::from([("p", false)])), Matched);
        assert_eq!(
            eval(c(json!(true)), &Context::from([("p", "true")])),
            Matched
        );
        assert_eq!(
            eval(c(json!("yes")), &Context::from([("p", true)])),
            NotMatched
        );
        assert_eq!(
            eval(c(json!("abc")), &Context::from([("p", 1)])),
            NotMatched
        );
    }

    #[test]
    fn null_context_value_is_a_value_for_property_conditions() {
        let ctx = Context::new().with("p", None::<String>);
        assert_eq!(
            eval(
                json!({"operator": "equals", "property": "p", "value": null}),
                &ctx
            ),
            Matched
        );
        assert_eq!(
            eval(
                json!({"operator": "equals", "property": "p", "value": "x"}),
                &ctx
            ),
            NotMatched
        );
    }

    #[test]
    fn unresolved_reference_never_matches() {
        let ctx = Context::from([("p", "x")]);
        for op in [
            "equals",
            "in",
            "not_in",
            "less_than",
            "greater_than_or_equal",
        ] {
            assert_eq!(
                eval(json!({"operator": op, "property": "p"}), &ctx),
                NotMatched,
                "{op}"
            );
        }
    }

    #[test]
    fn in_and_not_in() {
        let ctx = |c: &str| Context::from([("country", c)]);
        let in_c = json!({"operator": "in", "property": "country", "value": ["US", "CA", "MX"]});
        assert_eq!(eval(in_c.clone(), &ctx("US")), Matched);
        assert_eq!(eval(in_c, &ctx("UK")), NotMatched);

        let not_in = json!({"operator": "not_in", "property": "country", "value": ["US", "CA"]});
        assert_eq!(eval(not_in.clone(), &ctx("UK")), Matched);
        assert_eq!(eval(not_in, &ctx("US")), NotMatched);
    }

    #[test]
    fn in_casts_each_element() {
        let c = json!({"operator": "in", "property": "tier", "value": ["1", "2"]});
        assert_eq!(eval(c, &Context::from([("tier", 2)])), Matched);
        let c = json!({"operator": "not_in", "property": "id", "value": [1, 2]});
        assert_eq!(eval(c, &Context::from([("id", "2")])), NotMatched);
    }

    #[test]
    fn in_with_non_array_does_not_match() {
        let ctx = Context::from([("p", "a")]);
        assert_eq!(
            eval(
                json!({"operator": "in", "property": "p", "value": "a"}),
                &ctx
            ),
            NotMatched
        );
        assert_eq!(
            eval(
                json!({"operator": "not_in", "property": "p", "value": "a"}),
                &ctx
            ),
            NotMatched
        );
    }

    #[test]
    fn comparisons() {
        let c = |op: &str, v: Value| json!({"operator": op, "property": "n", "value": v});
        let n = |v: i32| Context::from([("n", v)]);
        assert_eq!(eval(c("less_than", json!(18)), &n(16)), Matched);
        assert_eq!(eval(c("less_than", json!(18)), &n(18)), NotMatched);
        assert_eq!(eval(c("less_than_or_equal", json!(18)), &n(18)), Matched);
        assert_eq!(eval(c("greater_than", json!(100)), &n(150)), Matched);
        assert_eq!(eval(c("greater_than", json!(100)), &n(100)), NotMatched);
        assert_eq!(
            eval(c("greater_than_or_equal", json!("100")), &n(100)),
            Matched
        );
        assert_eq!(eval(c("greater_than", json!(true)), &n(1)), NotMatched);
    }

    #[test]
    fn string_comparisons_are_lexicographic() {
        let c = json!({"operator": "less_than", "property": "version", "value": "2.0.0"});
        let v = |s: &str| Context::from([("version", s)]);
        assert_eq!(eval(c.clone(), &v("1.9.0")), Matched);
        assert_eq!(eval(c.clone(), &v("2.0.0")), NotMatched);
        assert_eq!(eval(c, &v("10.0.0")), Matched);
    }

    #[test]
    fn string_comparisons_use_utf16_order() {
        // U+FF61 sorts before U+1F600 by code point, but after its surrogate pair in UTF-16.
        let c = json!({"operator": "less_than", "property": "s", "value": "\u{1F600}"});
        assert_eq!(eval(c, &Context::from([("s", "\u{FF61}")])), NotMatched);
    }

    #[test]
    fn composites() {
        let and = json!({"operator": "and", "conditions": [
            {"operator": "equals", "property": "env", "value": "production"},
            {"operator": "equals", "property": "missing", "value": "value"},
        ]});
        assert_eq!(
            eval(and.clone(), &Context::from([("env", "production")])),
            Unknown
        );
        assert_eq!(eval(and, &Context::from([("env", "staging")])), NotMatched);

        let or = json!({"operator": "or", "conditions": [
            {"operator": "equals", "property": "env", "value": "production"},
            {"operator": "equals", "property": "missing", "value": "value"},
        ]});
        assert_eq!(
            eval(or.clone(), &Context::from([("env", "production")])),
            Matched
        );
        assert_eq!(eval(or, &Context::from([("env", "staging")])), Unknown);

        let not = json!({"operator": "not", "condition": {"operator": "equals", "property": "env", "value": "production"}});
        assert_eq!(
            eval(not.clone(), &Context::from([("env", "production")])),
            NotMatched
        );
        assert_eq!(
            eval(not.clone(), &Context::from([("env", "staging")])),
            Matched
        );
        assert_eq!(eval(not, &Context::new()), Unknown);
    }

    #[test]
    fn segmentation() {
        let seg = |from: f64, to: f64| json!({"operator": "segmentation", "property": "userId", "fromPercentage": from, "toPercentage": to, "seed": "test-seed"});
        let user = Context::from([("userId", "user-123")]);
        assert_eq!(eval(seg(0.0, 100.0), &user), Matched);
        assert_eq!(eval(seg(0.0, 0.0), &user), NotMatched);
        assert_eq!(eval(seg(0.0, 50.0), &Context::new()), Unknown);
        assert_eq!(
            eval(seg(0.0, 50.0), &Context::new().with("userId", None::<&str>)),
            Unknown
        );

        let matched = (0..1000)
            .filter(|i| {
                eval(
                    seg(0.0, 50.0),
                    &Context::from([("userId", format!("user-{i}"))]),
                ) == Matched
            })
            .count();
        assert!((400..600).contains(&matched), "{matched}");
    }

    #[test]
    fn segmentation_buckets_match_server() {
        // unit = fnv1a32("user-1" + "seed") / 2^32 = 0x08e5c021 / 2^32 ≈ 0.0348
        let seg = |from: f64, to: f64| json!({"operator": "segmentation", "property": "id", "fromPercentage": from, "toPercentage": to, "seed": "seed"});
        assert_eq!(js::fnv1a32("user-1seed"), 0x08e5_c021);
        let ctx = Context::from([("id", "user-1")]);
        assert_eq!(eval(seg(3.0, 4.0), &ctx), Matched);
        assert_eq!(eval(seg(0.0, 3.0), &ctx), NotMatched);
        assert_eq!(eval(seg(4.0, 100.0), &ctx), NotMatched);
        // Numbers are stringified the JS way before hashing: 1.0 hashes as "1".
        assert_eq!(
            eval(seg(0.0, 100.0), &Context::from([("id", 1.0)])),
            eval(seg(0.0, 100.0), &Context::from([("id", 1)]))
        );
    }

    #[test]
    fn unsupported_operator_is_unknown() {
        assert_eq!(
            eval(
                json!({"operator": "regex", "property": "p", "value": "x"}),
                &Context::from([("p", "x")])
            ),
            Unknown
        );
    }

    #[test]
    fn first_matching_override_wins_and_unknown_skips() {
        let overrides: Vec<Override> = serde_json::from_value(json!([
            {"name": "with-unknown", "value": "unknown-override", "conditions": [
                {"operator": "equals", "property": "env", "value": "production"},
                {"operator": "equals", "property": "missing", "value": "value"},
            ]},
            {"name": "first", "value": "first", "conditions": [{"operator": "equals", "property": "env", "value": "production"}]},
            {"name": "second", "value": "second", "conditions": [{"operator": "equals", "property": "env", "value": "production"}]},
        ]))
        .unwrap();
        let base = json!("default");
        assert_eq!(
            evaluate_overrides(&base, &overrides, &Context::from([("env", "production")])),
            &json!("first")
        );
        assert_eq!(
            evaluate_overrides(&base, &overrides, &Context::from([("env", "dev")])),
            &base
        );
        assert_eq!(evaluate_overrides(&base, &[], &Context::new()), &base);
    }
}
