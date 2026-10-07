use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A config as delivered by the Replane SDK API: a base value plus overrides
/// that are evaluated locally against the caller's context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub name: String,
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub overrides: Vec<Override>,
}

/// An override replaces the base value when all of its conditions match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Override {
    pub name: String,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub value: Value,
}

/// An override condition with config references already resolved by the server.
///
/// `value` is `None` when the server rendered a reference that points to a
/// missing value (it omits the field in that case); such a condition never matches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operator", rename_all = "snake_case")]
pub enum Condition {
    Equals {
        property: String,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        value: Option<Value>,
    },
    In {
        property: String,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        value: Option<Value>,
    },
    NotIn {
        property: String,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        value: Option<Value>,
    },
    LessThan {
        property: String,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        value: Option<Value>,
    },
    LessThanOrEqual {
        property: String,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        value: Option<Value>,
    },
    GreaterThan {
        property: String,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        value: Option<Value>,
    },
    GreaterThanOrEqual {
        property: String,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        value: Option<Value>,
    },
    #[serde(rename_all = "camelCase")]
    Segmentation {
        property: String,
        from_percentage: f64,
        to_percentage: f64,
        seed: String,
    },
    And {
        conditions: Vec<Condition>,
    },
    Or {
        conditions: Vec<Condition>,
    },
    Not {
        condition: Box<Condition>,
    },
    /// An operator this SDK version doesn't know about. Always evaluates to "unknown",
    /// so the override is skipped instead of the whole config failing to parse.
    #[serde(other)]
    Unsupported,
}

/// Keeps an explicit JSON `null` as `Some(Value::Null)`; only a missing field is `None`.
fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

/// A serializable copy of the client's configs, e.g. to hydrate another client.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub configs: Vec<Config>,
}

/// Notification passed to subscribers when a config changes on the server.
///
/// `value` is the base value; overrides are not applied because subscriptions
/// are not tied to a context. Call [`crate::Replane::get`] to get the evaluated value.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigChange {
    pub name: String,
    pub value: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartReplicationStreamBody<'a> {
    pub current_configs: &'a [Config],
    pub required_configs: &'a [String],
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ReplicationStreamRecord {
    Init { configs: Vec<Config> },
    ConfigChange { config: Config },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_server_payload() {
        let raw = json!({
            "name": "rate-limit",
            "value": 100,
            "overrides": [{
                "name": "vip",
                "value": 1000,
                "conditions": [
                    {"operator": "in", "property": "plan", "value": ["pro", "enterprise"]},
                    {"operator": "segmentation", "property": "userId", "fromPercentage": 0, "toPercentage": 50, "seed": "s"},
                    {"operator": "not", "__sentinel": null, "condition": {"operator": "equals", "property": "region"}},
                    {"operator": "regex_match", "property": "email", "value": ".*"}
                ]
            }]
        });
        let config: Config = serde_json::from_value(raw).unwrap();
        let conditions = &config.overrides[0].conditions;
        assert!(matches!(
            &conditions[0],
            Condition::In { value: Some(_), .. }
        ));
        assert!(matches!(
            &conditions[1],
            Condition::Segmentation { to_percentage, .. } if *to_percentage == 50.0
        ));
        assert!(matches!(
            &conditions[2],
            Condition::Not { condition } if matches!(**condition, Condition::Equals { value: None, .. })
        ));
        assert_eq!(conditions[3], Condition::Unsupported);
    }

    #[test]
    fn distinguishes_null_from_missing_value() {
        let null: Condition =
            serde_json::from_value(json!({"operator": "equals", "property": "p", "value": null}))
                .unwrap();
        assert_eq!(
            null,
            Condition::Equals {
                property: "p".into(),
                value: Some(Value::Null)
            }
        );
        let missing: Condition =
            serde_json::from_value(json!({"operator": "equals", "property": "p"})).unwrap();
        assert_eq!(
            missing,
            Condition::Equals {
                property: "p".into(),
                value: None
            }
        );
    }

    #[test]
    fn snapshot_round_trips() {
        let snapshot = Snapshot {
            configs: vec![Config {
                name: "flag".into(),
                value: json!(true),
                overrides: vec![Override {
                    name: "o".into(),
                    conditions: vec![Condition::Segmentation {
                        property: "userId".into(),
                        from_percentage: 0.0,
                        to_percentage: 10.0,
                        seed: "x".into(),
                    }],
                    value: json!(false),
                }],
            }],
        };
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            json["configs"][0]["overrides"][0]["conditions"][0]["fromPercentage"],
            json!(0.0)
        );
        let back: Snapshot = serde_json::from_value(json).unwrap();
        assert_eq!(back, snapshot);
    }
}
