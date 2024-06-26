mod json;
mod types;

use std::rc::Rc;

use crate::types::*;
use log::*;

use proxy_wasm::traits::{Context, HttpContext, RootContext};
use proxy_wasm::types::{Action, ContextType, LogLevel};
use serde_json::{self, Value as JsonValue};

proxy_wasm::main! {{
   proxy_wasm::set_log_level(LogLevel::Info);
   proxy_wasm::set_root_context(|_| -> Box<dyn RootContext> {
       Box::new(ResponseTransformerRoot { config: None, id: 0 } )
   });
}}

const CONTENT_LENGTH: &str = "content-length";
const CONTENT_TYPE: &str = "content-type";

fn is_json_mime_type<T: AsRef<str>>(ct: T) -> bool {
    let Ok(mt) = ct.as_ref().parse::<mime::Mime>() else {
        return false;
    };

    matches!(
        (mt.type_(), mt.subtype(), mt.suffix()),
        (mime::APPLICATION, mime::JSON, _) | (mime::APPLICATION, _, Some(mime::JSON))
    )
}

struct ResponseTransformerRoot {
    config: Option<Rc<Config>>,
    id: u32,
}

impl Context for ResponseTransformerRoot {}

impl ResponseTransformerRoot {}

impl RootContext for ResponseTransformerRoot {
    fn on_configure(&mut self, _: usize) -> bool {
        info!("ID: {}, existing config: {:?}", self.id, self.config);

        let Some(bytes) = self.get_plugin_configuration() else {
            warn!("no configuration provided");
            return false;
        };

        match serde_json::from_slice::<ConfigInput>(bytes.as_slice()) {
            Ok(user_config) => {
                self.config = Some(Rc::new(user_config.into()));

                info!("new configuration: {:#?}", &self.config);

                true
            }
            Err(e) => {
                error!("failed to parse configuration: {:?}", e);
                false
            }
        }
    }

    fn create_http_context(&self, id: u32) -> Option<Box<dyn HttpContext>> {
        info!("create_http_context ID: {id}");

        let Some(config) = &self.config else {
            warn!("called create_http_context() with no root context config");
            return None;
        };

        let config = config.clone();

        Some(Box::new(ResponseTransformerHttp { config, id }))
    }

    fn get_type(&self) -> Option<ContextType> {
        Some(ContextType::HttpContext)
    }
}

struct ResponseTransformerHttp {
    config: Rc<Config>,
    id: u32,
}

impl Context for ResponseTransformerHttp {}

impl HttpContext for ResponseTransformerHttp {
    fn on_http_response_headers(&mut self, num_headers: usize, end_of_stream: bool) -> Action {
        info!(
            "{} on_http_response_headers, num_headers: {}, eof: {}",
            self.id, num_headers, end_of_stream
        );

        if self.config.json.is_some() && self.is_json_response() {
            info!(
                "removing {} header for body transformations",
                CONTENT_LENGTH
            );
            self.set_http_response_header(CONTENT_LENGTH, None);
        }

        if let Some(header_tx) = &self.config.headers {
            self.transform_headers(header_tx);
        };

        Action::Continue
    }

    fn on_http_response_body(&mut self, body_size: usize, end_of_stream: bool) -> Action {
        info!(
            "{} on_http_response_body, body_size: {}, eof: {}",
            self.id, body_size, end_of_stream
        );

        if let Some(json_tx) = &self.config.json {
            if !self.is_json_response() {
                info!("response is not JSON, exiting");
                return Action::Continue;
            }

            if !end_of_stream {
                return Action::Pause;
            }

            let Some(body) = self.get_http_response_body(0, body_size) else {
                info!("empty response body, exiting");
                return Action::Continue;
            };

            self.transform_body(json_tx, body);
        }

        Action::Continue
    }
}

impl ResponseTransformerHttp {
    fn is_json_response(&self) -> bool {
        self.get_http_response_header(CONTENT_TYPE)
            .map_or(false, is_json_mime_type)
    }

    fn transform_headers(&self, tx: &Headers) {
        // https://docs.konghq.com/hub/kong-inc/response-transformer/#order-of-execution

        tx.remove.iter().for_each(|name| {
            if self.get_http_response_header(name).is_some() {
                info!("removing header: {}", name);
                self.set_http_response_header(name, None);
            }
        });

        tx.rename.iter().for_each(|KeyValue(from, to)| {
            if let Some(value) = self.get_http_response_header(from) {
                info!("renaming header {} => {}", from, to);
                self.set_http_response_header(from, None);
                self.set_http_response_header(to, Some(value.as_ref()));
            }
        });

        tx.replace.iter().for_each(|KeyValue(name, value)| {
            if self.get_http_response_header(name).is_some() {
                info!("updating header {} value to {}", name, value);
                self.set_http_response_header(name, Some(value));
            }
        });

        tx.add.iter().for_each(|KeyValue(name, value)| {
            if self.get_http_response_header(name).is_none() {
                info!("adding header {} => {}", name, value);
                self.set_http_response_header(name, Some(value));
            }
        });

        tx.append.iter().for_each(|KeyValue(name, value)| {
            info!("appending header {} => {}", name, value);
            self.add_http_response_header(name, value);
        });
    }

    fn transform_body(&self, tx: &Json, body: Vec<u8>) {
        let mut json = match serde_json::from_slice(&body) {
            Ok(JsonValue::Object(value)) => value,
            Ok(other) => {
                warn!(
                    "invalid response body type (expected: object, got: {}), exiting",
                    json::type_name(other)
                );
                return;
            }
            Err(e) => {
                warn!("response body was invalid JSON ({}), exiting", e);
                return;
            }
        };

        if !tx.transform_body(&mut json) {
            info!("no response body changes were applied");
            return;
        }

        let body = match serde_json::to_vec(&json) {
            Ok(b) => b,
            Err(e) => {
                error!("failed to re-serialize JSON response body ({}), exiting", e);
                return;
            }
        };

        self.set_http_response_body(0, body.len(), body.as_slice());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_mime_type_detection() {
        assert!(is_json_mime_type("application/json"));
        assert!(is_json_mime_type("APPLICATION/json"));
        assert!(is_json_mime_type("APPLICATION/JSON"));
        assert!(is_json_mime_type("application/JSON"));
        assert!(is_json_mime_type("application/json; charset=utf-8"));
        assert!(is_json_mime_type("application/problem+json"));
        assert!(is_json_mime_type("application/problem+JSON"));
        assert!(is_json_mime_type("application/problem+json; charset=utf-8"));

        assert!(!is_json_mime_type("text/plain"));
        assert!(!is_json_mime_type("application/not-json"));
        assert!(!is_json_mime_type("nope/json"));
    }
}
