//! Search and engine option types, their defaults, and the overlay parsing
//! that mirrors MiniSearch's shallow option-object spreads.

use indexmap::IndexMap;
use serde_json::Value;

use crate::js::{as_f64, js_to_string};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy)]
pub struct Bm25 {
    pub k: f64,
    pub b: f64,
    pub d: f64,
}

pub const DEFAULT_BM25: Bm25 = Bm25 {
    k: 1.2,
    b: 0.7,
    d: 0.5,
};

#[derive(Clone, Copy)]
pub struct Weights {
    pub fuzzy: f64,
    pub prefix: f64,
}

pub const DEFAULT_WEIGHTS: Weights = Weights {
    fuzzy: 0.45,
    prefix: 0.375,
};

#[derive(Clone, Copy)]
pub enum Prefix {
    Enabled(bool),
    /// Prefix search only on the last query term: the default for
    /// auto-suggestions.
    LastTerm,
}

#[derive(Clone, Copy)]
pub enum Fuzzy {
    Off,
    /// `fuzzy: true`: a fraction of 0.2 of the term length.
    Auto,
    /// A number: below 1 it is a fraction of the term length, otherwise a
    /// maximum edit distance.
    Value(f64),
}

#[derive(Clone, Copy)]
pub enum Combine {
    Or,
    And,
    AndNot,
}

#[derive(Clone)]
pub struct SearchOptions {
    pub fields: Option<Vec<String>>,
    pub boost: IndexMap<String, f64>,
    pub weights: Weights,
    pub prefix: Prefix,
    pub fuzzy: Fuzzy,
    pub max_fuzzy: f64,
    pub combine_with: Combine,
    pub bm25: Bm25,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            fields: None,
            boost: IndexMap::new(),
            weights: DEFAULT_WEIGHTS,
            prefix: Prefix::Enabled(false),
            fuzzy: Fuzzy::Off,
            max_fuzzy: 6.0,
            combine_with: Combine::Or,
            bm25: DEFAULT_BM25,
        }
    }
}

impl SearchOptions {
    /// Applies the recognized keys of a plain-object overlay, mirroring the
    /// shallow object spreads used by MiniSearch when merging option layers.
    pub fn overlay(&self, overlay: &Value) -> Result<SearchOptions> {
        let mut options = self.clone();
        let Some(object) = overlay.as_object() else {
            return Ok(options);
        };

        if let Some(fields) = object.get("fields") {
            options.fields = Some(string_array(fields, "fields")?);
        }
        if let Some(boost) = object.get("boost").and_then(Value::as_object) {
            let mut boosts = IndexMap::new();
            for (field, factor) in boost {
                if let Some(factor) = as_f64(factor) {
                    boosts.insert(field.clone(), factor);
                }
            }
            options.boost = boosts;
        }
        if let Some(weights) = object.get("weights").and_then(Value::as_object) {
            if let Some(fuzzy) = weights.get("fuzzy").and_then(as_f64) {
                options.weights.fuzzy = fuzzy;
            }
            if let Some(prefix) = weights.get("prefix").and_then(as_f64) {
                options.weights.prefix = prefix;
            }
        }
        if let Some(prefix) = object.get("prefix") {
            if let Some(enabled) = prefix.as_bool() {
                options.prefix = Prefix::Enabled(enabled);
            }
        }
        if let Some(fuzzy) = object.get("fuzzy") {
            options.fuzzy = match fuzzy {
                Value::Bool(true) => Fuzzy::Auto,
                Value::Bool(false) => Fuzzy::Off,
                Value::Number(_) => Fuzzy::Value(as_f64(fuzzy).expect("checked number")),
                _ => options.fuzzy,
            };
        }
        if let Some(max_fuzzy) = object.get("maxFuzzy").and_then(as_f64) {
            options.max_fuzzy = max_fuzzy;
        }
        if let Some(combine) = object.get("combineWith") {
            options.combine_with = parse_combine(combine)?;
        }
        if let Some(bm25) = object.get("bm25").and_then(Value::as_object) {
            // A bm25 overlay replaces the whole parameter object, like the
            // original's shallow spread. Missing keys become NaN, matching
            // the undefined arithmetic of the original; NaN scores surface
            // as null through JSON, like `JSON.stringify(NaN)`.
            options.bm25 = Bm25 {
                k: bm25.get("k").and_then(as_f64).unwrap_or(f64::NAN),
                b: bm25.get("b").and_then(as_f64).unwrap_or(f64::NAN),
                d: bm25.get("d").and_then(as_f64).unwrap_or(f64::NAN),
            };
        }
        Ok(options)
    }
}

pub fn parse_combine(value: &Value) -> Result<Combine> {
    let Some(text) = value.as_str() else {
        return Err(format!(
            "Invalid combination operator: {}",
            js_to_string(value)
        ));
    };
    match text.to_lowercase().as_str() {
        "or" => Ok(Combine::Or),
        "and" => Ok(Combine::And),
        "and_not" => Ok(Combine::AndNot),
        _ => Err(format!("Invalid combination operator: {text}")),
    }
}

pub fn string_array(value: &Value, name: &str) -> Result<Vec<String>> {
    let Some(items) = value.as_array() else {
        return Err(format!(
            "MiniSearch: option \"{name}\" must be an array of strings"
        ));
    };
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let Some(text) = item.as_str() else {
            return Err(format!(
                "MiniSearch: option \"{name}\" must be an array of strings"
            ));
        };
        result.push(text.to_string());
    }
    Ok(result)
}

pub struct AutoVacuum {
    pub min_dirt_count: u32,
    pub min_dirt_factor: f64,
}

pub const DEFAULT_AUTO_VACUUM: AutoVacuum = AutoVacuum {
    min_dirt_count: 20,
    min_dirt_factor: 0.1,
};

/// Parses the `autoVacuum` constructor option: `true`/absent enables the
/// defaults, `false` disables, and an object customizes the thresholds.
/// Falsy thresholds (0) fall back to the defaults, matching the original's
/// `minDirtCount || defaultAutoVacuumOptions.minDirtCount`.
pub fn parse_auto_vacuum(value: Option<&Value>) -> Option<AutoVacuum> {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(true)) => Some(DEFAULT_AUTO_VACUUM),
        Some(Value::Bool(false)) => None,
        Some(Value::Object(settings)) => Some(AutoVacuum {
            min_dirt_count: settings
                .get("minDirtCount")
                .and_then(as_f64)
                .map(|value| value as u32)
                .filter(|&value| value != 0)
                .unwrap_or(DEFAULT_AUTO_VACUUM.min_dirt_count),
            min_dirt_factor: settings
                .get("minDirtFactor")
                .and_then(as_f64)
                .filter(|&value| value != 0.0)
                .unwrap_or(DEFAULT_AUTO_VACUUM.min_dirt_factor),
        }),
        Some(_) => Some(DEFAULT_AUTO_VACUUM),
    }
}
