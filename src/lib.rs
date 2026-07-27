//! Node-API bindings for the ferrosearch engine.
//!
//! The exposed class mirrors the MiniSearch API. Option values that are
//! JavaScript functions (custom tokenizers, term processors, filters, and
//! document boosters) cannot cross the JSON boundary and are not supported;
//! the README documents these deltas.

#![deny(clippy::all)]

mod engine;
mod js;
mod options;
mod radix;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value;

fn reason(error: String) -> Error {
    Error::from_reason(error)
}

#[napi(js_name = "MiniSearch")]
pub struct MiniSearch {
    engine: engine::Engine,
}

#[napi]
impl MiniSearch {
    #[napi(constructor)]
    pub fn new(options: Value) -> Result<Self> {
        engine::Engine::new(&options)
            .map(|engine| Self { engine })
            .map_err(reason)
    }

    /// Deserializes an index serialized with `toJSON`/`JSON.stringify`. The
    /// same options used when serializing must be provided.
    #[napi(factory)]
    pub fn load_json(json: String, options: Value) -> Result<Self> {
        engine::Engine::load_json(&json, &options)
            .map(|engine| Self { engine })
            .map_err(reason)
    }

    #[napi]
    pub fn add(&mut self, document: Value) -> Result<()> {
        self.engine.add(&document).map_err(reason)
    }

    #[napi]
    pub fn add_all(&mut self, documents: Vec<Value>) -> Result<()> {
        self.engine.add_all(&documents).map_err(reason)
    }

    #[napi]
    pub fn remove(&mut self, document: Value) -> Result<()> {
        self.engine.remove(&document).map_err(reason)
    }

    /// Removes the given documents. When called without arguments, removes
    /// all documents, resetting the index.
    #[napi]
    pub fn remove_all(&mut self, documents: Option<Vec<Value>>) -> Result<()> {
        match documents {
            Some(documents) => {
                for document in documents {
                    self.engine.remove(&document).map_err(reason)?;
                }
                Ok(())
            }
            None => {
                self.engine.remove_all();
                Ok(())
            }
        }
    }

    #[napi]
    pub fn discard(&mut self, id: Value) -> Result<()> {
        self.engine.discard(&id).map_err(reason)
    }

    #[napi]
    pub fn discard_all(&mut self, ids: Vec<Value>) -> Result<()> {
        self.engine.discard_all(&ids).map_err(reason)
    }

    #[napi]
    pub fn replace(&mut self, document: Value) -> Result<()> {
        self.engine.replace(&document).map_err(reason)
    }

    /// Cleans up references to discarded documents. Native vacuuming is
    /// synchronous and complete; there is no main thread to block.
    #[napi]
    pub fn vacuum(&mut self) {
        self.engine.vacuum();
    }

    #[napi]
    pub fn has(&self, id: Value) -> bool {
        self.engine.has(&id)
    }

    #[napi]
    pub fn get_stored_fields(&self, id: Value) -> Option<Value> {
        self.engine.get_stored_fields(&id)
    }

    /// Searches the index. Takes a query string or a combination query
    /// object with a `queries` array, and returns scored results sorted by
    /// descending score. Search mutates the index only to clean up stale
    /// references to discarded documents, exactly like the original.
    #[napi]
    pub fn search(&mut self, query: Value, options: Option<Value>) -> Result<Vec<Value>> {
        self.engine
            .search(&query, options.as_ref().unwrap_or(&Value::Null))
            .map_err(reason)
    }

    /// Same as `search`, returning the results as a JSON string. Crossing
    /// the native boundary once and parsing with `JSON.parse` is much faster
    /// than materializing result objects through the bindings.
    #[napi]
    pub fn search_json(&mut self, query: Value, options: Option<Value>) -> Result<String> {
        self.engine
            .search_to_json_string(&query, options.as_ref().unwrap_or(&Value::Null))
            .map_err(reason)
    }

    /// Returns results for all documents, equivalent to searching with the
    /// `MiniSearch.wildcard` symbol in the original API.
    #[napi]
    pub fn wildcard_search(&self, options: Option<Value>) -> Result<Vec<Value>> {
        self.engine
            .wildcard_search(options.as_ref().unwrap_or(&Value::Null))
            .map_err(reason)
    }

    #[napi]
    pub fn auto_suggest(&mut self, query: String, options: Option<Value>) -> Result<Vec<Value>> {
        self.engine
            .auto_suggest(&query, options.as_ref().unwrap_or(&Value::Null))
            .map_err(reason)
    }

    #[napi(getter)]
    pub fn document_count(&self) -> u32 {
        self.engine.document_count()
    }

    #[napi(getter)]
    pub fn term_count(&self) -> u32 {
        self.engine.term_count()
    }

    #[napi(getter)]
    pub fn dirt_count(&self) -> u32 {
        self.engine.dirt_count()
    }

    #[napi(getter)]
    pub fn dirt_factor(&self) -> f64 {
        self.engine.dirt_factor()
    }

    #[napi]
    pub fn to_json(&self) -> Value {
        self.engine.to_json()
    }
}
