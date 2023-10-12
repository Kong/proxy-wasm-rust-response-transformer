use crate::json::*;
use std::convert::TryFrom;
use std::fmt;

use serde::Deserialize;

use serde_json::Value as JsonValue;

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

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Json {
    pub(crate) remove: Vec<String>,
    pub(crate) replace: Vec<(String, JsonValue)>,
    pub(crate) add: Vec<(String, JsonValue)>,
    pub(crate) append: Vec<(String, JsonValue)>,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct Config {
    pub(crate) headers: Option<Headers>,
    pub(crate) json: Option<Json>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
