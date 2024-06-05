use crate::json::*;
use log::*;
use std::convert::TryFrom;
use std::fmt;

use serde::Deserialize;

use serde_json::Value as JsonValue;
type JsonMap = serde_json::Map<String, JsonValue>;

fn split_str(input: &str) -> Result<(&str, &str), InvalidKeyValue> {
    input
        .split_once(':')
        .filter(|(name, value)| !name.is_empty() && !value.is_empty())
        .ok_or_else(|| InvalidKeyValue(input.into()))
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InvalidKeyValue(String);

impl fmt::Display for InvalidKeyValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Invalid <key>:<value> => {:?}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct KeyValue(pub(crate) String, pub(crate) String);

impl TryFrom<String> for KeyValue {
    type Error = InvalidKeyValue;

    fn try_from(input: String) -> std::result::Result<Self, Self::Error> {
        Ok(split_str(&input)?.into())
    }
}

impl From<(&str, &str)> for KeyValue {
    fn from(value: (&str, &str)) -> Self {
        KeyValue(value.0.to_owned(), value.1.to_owned())
    }
}

impl TryFrom<&str> for KeyValue {
    type Error = InvalidKeyValue;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        KeyValue::try_from(value.to_string())
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(default)]
pub(crate) struct TransformationsConfig<T = KeyValue> {
    pub(crate) headers: Vec<T>,
    pub(crate) json: Vec<T>,
    pub(crate) json_types: Vec<Cast>,
}

impl<T> Default for TransformationsConfig<T> {
    fn default() -> Self {
        TransformationsConfig {
            headers: vec![],
            json: vec![],
            json_types: vec![],
        }
    }
}

impl TransformationsConfig {
    fn cast_json(mut self) -> Vec<(String, JsonValue)> {
        let mut json_values = Vec::with_capacity(self.json.len());
        let json = self.json.drain(..);

        for (i, kv) in json.enumerate() {
            if let Some(typ) = self.json_types.get(i) {
                json_values.push((kv.0, typ.convert(kv.1)));
            } else {
                json_values.push((kv.0, Cast::String.convert(kv.1)));
            }
        }

        json_values
    }
}

#[derive(Deserialize, Default, PartialEq, Eq, Debug, Clone)]
#[serde(default)]
pub(crate) struct ConfigInput {
    remove: TransformationsConfig<String>,
    rename: TransformationsConfig,
    replace: TransformationsConfig,
    add: TransformationsConfig,
    append: TransformationsConfig,
}

impl From<ConfigInput> for Config {
    fn from(val: ConfigInput) -> Self {
        let mut config: Config = Default::default();

        if !val.remove.headers.is_empty()
            || !val.rename.headers.is_empty()
            || !val.replace.headers.is_empty()
            || !val.add.headers.is_empty()
            || !val.append.headers.is_empty()
        {
            config.headers = Some(Headers {
                remove: val.remove.headers,
                rename: val.rename.headers,
                replace: val.replace.headers.clone(),
                add: val.add.headers.clone(),
                append: val.append.headers.clone(),
            });
        }

        if !val.remove.json.is_empty()
            || !val.rename.json.is_empty()
            || !val.replace.json.is_empty()
            || !val.add.json.is_empty()
            || !val.append.json.is_empty()
        {
            config.json = Some(Json {
                remove: val.remove.json,
                rename: val.rename.json,
                replace: val.replace.cast_json(),
                add: val.add.cast_json(),
                append: val.append.cast_json(),
            });
        }

        config
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Headers {
    pub(crate) remove: Vec<String>,
    pub(crate) rename: Vec<KeyValue>,
    pub(crate) replace: Vec<KeyValue>,
    pub(crate) add: Vec<KeyValue>,
    pub(crate) append: Vec<KeyValue>,
}

#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub(crate) struct Json {
    pub(crate) remove: Vec<String>,
    pub(crate) rename: Vec<KeyValue>,
    pub(crate) replace: Vec<(String, JsonValue)>,
    pub(crate) add: Vec<(String, JsonValue)>,
    pub(crate) append: Vec<(String, JsonValue)>,
}

impl Json {
    pub(crate) fn transform_body(&self, body: &mut JsonMap) -> bool {
        // https://docs.konghq.com/hub/kong-inc/response-transformer/#order-of-execution

        let mut changed = false;

        for field in &self.remove {
            if body.remove(field).is_some() {
                info!("removed field {:?}", field);
                changed = true;
            }
        }

        for KeyValue(from, to) in &self.rename {
            if let Some(v) = body.remove(from) {
                let _ = body.insert(to.clone(), v);
                info!("renamed {} => {}", from, to);
                changed = true;
            }
        }

        for (field, value) in &self.replace {
            if let Some(found) = body.get_mut(field) {
                if found != value {
                    info!("replacing field {:?} {:?} => {:?}", field, found, value);
                    *found = value.clone();
                    changed = true;
                }
            }
        }

        for (field, value) in &self.add {
            if !body.contains_key(field) {
                info!("adding field {:?} {:?}", field, value);
                body.insert(field.to_owned(), value.clone());
                changed = true;
            }
        }

        for (field, value) in &self.append {
            body.entry(field)
                .and_modify(|found| {
                    let current = found.take();
                    let mut appended = false;

                    *found = match current {
                        JsonValue::String(_) => {
                            appended = true;
                            serde_json::json!([current, value.clone()])
                        }
                        JsonValue::Array(mut arr) => {
                            appended = true;
                            arr.push(value.clone());
                            arr.into()
                        }
                        // XXX: this branch is not fully compatible with the Lua plugin
                        //
                        // The lua plugin doesn't attempt to disambiguate between an
                        // array-like table and a map-like table. It just blindly calls
                        // the `table.insert()` function.
                        _ => current,
                    };

                    if appended {
                        changed = true;
                        info!("appended {:?} to {:?}", value, field);
                    }
                })
                .or_insert_with(|| {
                    changed = true;
                    let new = serde_json::json!([value]);
                    info!("inserted {:?} to {:?}", new, field);
                    new
                });
        }

        changed
    }
}

#[derive(Default, Debug, Clone)]
pub(crate) struct Config {
    pub(crate) headers: Option<Headers>,
    pub(crate) json: Option<Json>,
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! map {
        ($x:tt) => {{
            let json = serde_json::json!($x);

            let serde_json::Value::Object(map) = json else {
                panic!("not a json object");
            };
            map
        }};
    }

    impl KeyValue {
        #[warn(unused)]
        pub(crate) fn new<T: std::string::ToString>(name: T, value: T) -> Self {
            KeyValue(name.to_string(), value.to_string())
        }
    }

    #[test]
    fn test_header_try_from_valid() {
        assert_eq!(Ok(KeyValue::new("a", "b")), KeyValue::try_from("a:b"));
    }

    #[test]
    fn test_header_try_from_invalid() {
        assert_eq!(
            Err(InvalidKeyValue("a".to_string())),
            KeyValue::try_from("a")
        );
        assert_eq!(
            Err(InvalidKeyValue("a:".to_string())),
            KeyValue::try_from("a:")
        );
        assert_eq!(
            Err(InvalidKeyValue(":b".to_string())),
            KeyValue::try_from(":b")
        );
    }

    #[test]
    fn test_json_deserialize_transformations() {
        assert_eq!(
            TransformationsConfig {
                headers: vec![KeyValue::new("a", "b"), KeyValue::new("c", "d")],
                ..Default::default()
            },
            serde_json::from_str(r#"{ "headers": ["a:b", "c:d"] }"#).unwrap()
        );
    }

    #[test]
    fn test_json_transform_remove() {
        let tx = Json {
            remove: vec!["remove_me".to_string()],
            ..Default::default()
        };

        let mut body = map!({
            "remove_me": "goodbye",
            "unchanged": true
        });

        assert!(tx.transform_body(&mut body));

        assert_eq!(body, map!({ "unchanged": true }));

        // no more changes
        assert!(!tx.transform_body(&mut body));
    }

    #[test]
    fn test_json_transform_rename() {
        let tx = Json {
            rename: vec![KeyValue::from(("rename_me", "renamed"))],
            ..Default::default()
        };

        let mut body = map!({
            "rename_me": "test",
            "unchanged": true
        });

        assert!(tx.transform_body(&mut body));

        assert_eq!(
            body,
            map!({
                "renamed": "test",
                "unchanged": true
            })
        );

        // no more changes
        assert!(!tx.transform_body(&mut body));
    }

    #[test]
    fn test_json_transform_replace() {
        let tx = Json {
            replace: vec![(
                "replace_me".to_string(),
                JsonValue::String("replacement".to_string()),
            )],
            ..Default::default()
        };

        let mut body = map!({
            "replace_me": "test",
            "unchanged": true
        });

        assert!(tx.transform_body(&mut body));

        assert_eq!(
            body,
            map!({
                "replace_me": "replacement",
                "unchanged": true
            })
        );

        // no more changes
        assert!(!tx.transform_body(&mut body));
    }

    #[test]
    fn test_json_transform_add() {
        let tx = Json {
            add: vec![("add_me".to_string(), JsonValue::String("added".to_string()))],
            ..Default::default()
        };

        let mut body = map!({ "unchanged": true });

        assert!(tx.transform_body(&mut body));

        assert_eq!(
            body,
            map!({
                "add_me": "added",
                "unchanged": true
            })
        );

        // no more changes
        assert!(!tx.transform_body(&mut body));
    }

    #[test]
    fn test_json_transform_append_absent() {
        let tx = Json {
            append: vec![(
                "append_me".to_string(),
                JsonValue::String("appended".to_string()),
            )],
            ..Default::default()
        };

        let mut body = map!({ "unchanged": true });

        assert!(tx.transform_body(&mut body));

        assert_eq!(
            body,
            map!({
                "append_me": [
                    "appended"
                ],
                "unchanged": true
            })
        );
    }

    #[test]
    fn test_json_transform_append_array() {
        let tx = Json {
            append: vec![(
                "append_me".to_string(),
                JsonValue::String("appended".to_string()),
            )],
            ..Default::default()
        };

        let mut body = map!({
            "append_me": [
                "current value"
            ],
            "unchanged": true
        });

        assert!(tx.transform_body(&mut body));

        assert_eq!(
            body,
            map!({
                "append_me": [
                    "current value",
                    "appended"
                ],
                "unchanged": true
            })
        );
    }

    #[test]
    fn test_json_transform_append_string() {
        let tx = Json {
            append: vec![(
                "append_me".to_string(),
                JsonValue::String("appended".to_string()),
            )],
            ..Default::default()
        };

        let mut body = map!({
            "append_me": "current value",
            "unchanged": true
        });

        assert!(tx.transform_body(&mut body));

        assert_eq!(
            body,
            map!({
                "append_me": [
                    "current value",
                    "appended"
                ],
                "unchanged": true
            })
        );
    }
}
