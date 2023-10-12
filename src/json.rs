use std::convert::TryFrom;
use std::fmt;

use serde::Deserialize;
use serde_json::{self, json, Value};

//use serde_json::{Result as JsonResult, Value as JsonValue};

#[derive(Debug, Clone, Eq, PartialEq, Default, Deserialize)]
#[serde(try_from = "&str")]
pub(crate) enum Cast {
    Number,
    Boolean,
    #[default]
    String,
}

#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub(crate) struct InvalidCastType(String);

impl std::fmt::Display for InvalidCastType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid JSON type: {:?}", self.0)
    }
}

impl std::error::Error for InvalidCastType {}

impl TryFrom<&str> for Cast {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_lowercase().as_str() {
            "number" => Ok(Cast::Number),
            "boolean" => Ok(Cast::Boolean),
            "string" => Ok(Cast::String),
            _ => Err("no".into()),
        }
    }
}

impl Cast {
    pub(crate) fn convert(&self, s: String) -> Value {
        match self {
            Self::String => Value::String(escape(&s)),
            Self::Number => match s.parse::<serde_json::Number>() {
                Ok(n) => Value::Number(n),
                Err(_) => Value::String(s),
            },
            Self::Boolean => {
                if &s == "true" {
                    json!(true)
                } else {
                    json!(false)
                }
            }
        }
    }
}

pub(crate) fn type_name(v: Value) -> &'static str {
    match v {
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Bool(_) => "boolean",
        Value::Null => "null",
        Value::Number(_) => "number",
    }
}

/// This relates to the json_value() function in the response-transformer Lua
/// plugin source code:
///
/// https://github.com/Kong/kong/blob/1f886076b533f9e162b26dbc9ddf87e10be22fce/kong/plugins/response-transformer/body_transformer.lua#L51-L64
///
/// The code in question handles more than just strings and accounts for the
/// fact that lua-cjson escapes forward slash. Not sure if this is correct, but
/// I seem to be able to reproduce its behavior exactly by just double-escaping
/// backslashes.
fn escape(s: &str) -> String {
    s.replace('\\', r"\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape() {
        const A: u8 = 97;
        const QUOTE: u8 = 34;
        const SLASH: u8 = 47;
        const BSLASH: u8 = 92;

        let cases: Vec<(Vec<u8>, Vec<u8>)> = vec![
            // aaa => aaa
            (vec![A, A, A], vec![A, A, A]),
            // aa//a => aa//a
            (vec![A, A, SLASH, SLASH, A], vec![A, A, SLASH, SLASH, A]),
            // aa"a => aa"a
            (vec![A, A, QUOTE, A], vec![A, A, QUOTE, A]),
            // aa\"a => aa\\"a
            (
                vec![A, A, BSLASH, QUOTE, A],
                vec![A, A, BSLASH, BSLASH, QUOTE, A],
            ),
            // aa\/a => aa\/a
            (
                vec![A, A, BSLASH, SLASH, A],
                vec![A, A, BSLASH, BSLASH, SLASH, A],
            ),
            // aa\\/a => aa\\\/a
            (
                vec![A, A, BSLASH, BSLASH, SLASH, A],
                vec![A, A, BSLASH, BSLASH, BSLASH, BSLASH, SLASH, A],
            ),
        ];

        for (input, exp) in cases {
            let input = String::from_utf8(input).unwrap();
            assert_eq!(exp, escape(&input).into_bytes());
        }
    }
}
