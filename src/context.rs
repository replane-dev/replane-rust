use std::collections::BTreeMap;

/// A primitive value used for override evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum ContextValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
}

impl From<bool> for ContextValue {
    fn from(v: bool) -> Self {
        ContextValue::Bool(v)
    }
}

impl From<&str> for ContextValue {
    fn from(v: &str) -> Self {
        ContextValue::String(v.to_owned())
    }
}

impl From<String> for ContextValue {
    fn from(v: String) -> Self {
        ContextValue::String(v)
    }
}

impl From<&String> for ContextValue {
    fn from(v: &String) -> Self {
        ContextValue::String(v.clone())
    }
}

macro_rules! impl_from_number {
    ($($t:ty),*) => {
        $(impl From<$t> for ContextValue {
            fn from(v: $t) -> Self {
                ContextValue::Number(v as f64)
            }
        })*
    };
}

impl_from_number!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64);

impl<T: Into<ContextValue>> From<Option<T>> for ContextValue {
    fn from(v: Option<T>) -> Self {
        v.map_or(ContextValue::Null, Into::into)
    }
}

/// Properties that overrides are evaluated against, e.g. user ID, plan or region.
///
/// The context never leaves the application: all evaluation happens locally.
///
/// ```
/// use replane::Context;
///
/// let ctx = Context::new().with("userId", "u-42").with("plan", "pro").with("age", 30);
/// assert!(ctx.get("plan").is_some());
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Context(BTreeMap<String, ContextValue>);

impl Context {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, key: impl Into<String>, value: impl Into<ContextValue>) -> Self {
        self.insert(key, value);
        self
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<ContextValue>) {
        self.0.insert(key.into(), value.into());
    }

    pub fn remove(&mut self, key: &str) -> Option<ContextValue> {
        self.0.remove(key)
    }

    pub fn get(&self, key: &str) -> Option<&ContextValue> {
        self.0.get(key)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ContextValue)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Returns a new context with `other`'s values taking precedence.
    pub fn merged(&self, other: &Context) -> Context {
        let mut merged = self.clone();
        merged.extend(other.0.iter().map(|(k, v)| (k.clone(), v.clone())));
        merged
    }
}

impl<K: Into<String>, V: Into<ContextValue>> Extend<(K, V)> for Context {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

impl<K: Into<String>, V: Into<ContextValue>> FromIterator<(K, V)> for Context {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut ctx = Context::new();
        ctx.extend(iter);
        ctx
    }
}

impl<K: Into<String>, V: Into<ContextValue>, const N: usize> From<[(K, V); N]> for Context {
    fn from(entries: [(K, V); N]) -> Self {
        entries.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_prefers_other() {
        let base = Context::from([("a", 1), ("b", 2)]);
        let merged = base.merged(&Context::from([("b", 3)]));
        assert_eq!(merged.get("a"), Some(&ContextValue::Number(1.0)));
        assert_eq!(merged.get("b"), Some(&ContextValue::Number(3.0)));
    }

    #[test]
    fn option_maps_to_null() {
        let ctx = Context::new().with("x", None::<&str>).with("y", Some("v"));
        assert_eq!(ctx.get("x"), Some(&ContextValue::Null));
        assert_eq!(ctx.get("y"), Some(&ContextValue::String("v".into())));
    }
}
