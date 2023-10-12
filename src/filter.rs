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
const JSON_CONTENT_TYPE: &str = "application/json";

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
            .map_or(false, |ct| ct.eq_ignore_ascii_case(JSON_CONTENT_TYPE))
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
        let mut changed = false;

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

        tx.remove.iter().for_each(|field| {
            if json.remove(field).is_some() {
                info!("removed field {:?}", field);
                changed = true;
            }
        });

        tx.replace.iter().for_each(|(field, value)| {
            if let Some(found) = json.get_mut(field) {
                info!("replacing field {:?} {:?} => {:?}", field, found, value);
                *found = value.clone();
                changed = true;
            }
        });

        tx.add.iter().for_each(|(field, value)| {
            if !json.contains_key(field) {
                info!("adding field {:?} {:?}", field, value);
                json.insert(field.to_owned(), value.clone());
                changed = true;
            }
        });

        tx.append.iter().for_each(|(field, value)| {
            json.entry(field)
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
        });

        if !changed {
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
