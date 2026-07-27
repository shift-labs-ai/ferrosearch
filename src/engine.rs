//! The ferrosearch engine: a Rust port of MiniSearch's core.
//!
//! The scoring model (BM25+), query combination semantics, serialization
//! format (version 2), and self-healing cleanup of discarded documents follow
//! the original TypeScript implementation. Insertion-ordered maps are used
//! throughout so that iteration order — and therefore tie ordering and the
//! order-sensitive `matching_fields` bookkeeping — matches JavaScript `Map`
//! behavior.

use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::Regex;
use rustc_hash::{FxBuildHasher, FxHashSet};
use serde_json::{json, Map as JsonMap, Value};

use crate::js::{
    as_f64, id_key, js_text, js_to_string, json_number_to, json_string_to, json_value_to,
    shallow_merge, without_key, IdKey,
};
use crate::options::{
    parse_auto_vacuum, parse_combine, string_array, AutoVacuum, Bm25, Combine, Fuzzy, Prefix,
    Result, SearchOptions,
};
use crate::radix::RadixTree;
use crate::vecmap::VecMap;

/// Insertion-ordered map with a fast non-cryptographic hasher, used for the
/// document-keyed engine maps.
type FxIndexMap<K, V> = IndexMap<K, V, FxBuildHasher>;

/// For each field, the number of occurrences of a term per document. Posting
/// lists are flat pair vectors: see the `vecmap` module for why.
type DocFreqs = VecMap<u32, u32>;
type FieldTermData = VecMap<u32, DocFreqs>;

const SERIALIZATION_VERSION: u64 = 2;

/// Matches any Unicode space, newline, or punctuation character.
fn tokenizer() -> &'static Regex {
    static TOKENIZER: OnceLock<Regex> = OnceLock::new();
    TOKENIZER.get_or_init(|| Regex::new(r"[\n\r\p{Z}\p{P}]+").expect("valid pattern"))
}

/// Interns the small set of terms touched by one query execution, so that
/// per-document bookkeeping works with integer IDs instead of heap-allocated
/// strings. This keeps the scoring loops allocation-free.
#[derive(Default)]
struct Interner {
    table: IndexMap<String, u16>,
}

impl Interner {
    fn intern(&mut self, term: &str) -> u16 {
        if let Some(&id) = self.table.get(term) {
            return id;
        }
        let id = self.table.len() as u16;
        self.table.insert(term.to_string(), id);
        id
    }

    fn resolve(&self, id: u16) -> &str {
        self.table
            .get_index(id as usize)
            .map(|(term, _)| term.as_str())
            .expect("interned ID is valid")
    }
}

struct RawScore {
    score: f64,
    /// Query terms that matched, deduplicated, as interned IDs. Kept as a
    /// Vec because the number of elements is small, exactly as in the
    /// original.
    terms: Vec<u16>,
    /// Matched document terms mapped to the fields they were found in:
    /// `(derived term ID, field IDs)` pairs in first-match order. Linear
    /// search is faster than hashing for these few entries.
    matches: Vec<(u16, Vec<u32>)>,
}

impl RawScore {
    fn add_term(&mut self, term: u16) {
        if !self.terms.contains(&term) {
            self.terms.push(term);
        }
    }

    fn add_match(&mut self, derived: u16, field_id: u32) {
        match self.matches.iter_mut().find(|(term, _)| *term == derived) {
            Some((_, fields)) => fields.push(field_id),
            None => self.matches.push((derived, vec![field_id])),
        }
    }

    /// `Object.assign` semantics: the other entry replaces the field list of
    /// an existing derived term.
    fn assign_match(&mut self, derived: u16, fields: Vec<u32>) {
        match self.matches.iter_mut().find(|(term, _)| *term == derived) {
            Some((_, existing)) => *existing = fields,
            None => self.matches.push((derived, fields)),
        }
    }
}

type RawResult = FxIndexMap<u32, RawScore>;

/// One scored search result, referencing interned terms. The final JSON is
/// produced directly from this without intermediate `Value` trees.
struct Hit {
    doc_id: u32,
    score: f64,
    terms: Vec<u16>,
    matches: Vec<(u16, Vec<u32>)>,
}

/// The processed content of one document field, shared by indexing and
/// removal.
struct FieldTokens {
    field_id: u32,
    /// Number of unique raw tokens, counted before term processing, exactly
    /// like the original's `new Set(tokens).size`.
    unique_tokens: u32,
    /// Tokens after term processing; empty tokens are dropped.
    terms: Vec<String>,
}

/// Per-query-term scoring inputs shared by the exact, prefix, and fuzzy
/// passes of `execute_query_spec`.
struct Scoring<'a> {
    source_term: u16,
    /// `(field name, boost)` pairs in search-field order. A falsy boost (0)
    /// has already been replaced by 1.
    field_boosts: &'a [(String, f64)],
    bm25: Bm25,
}

pub struct Engine {
    fields: Vec<String>,
    field_ids: IndexMap<String, u32>,
    store_fields: Vec<String>,
    id_field: String,
    default_search: SearchOptions,
    default_auto_suggest: SearchOptions,
    auto_vacuum: Option<AutoVacuum>,

    index: RadixTree<FieldTermData>,
    document_count: u32,
    document_ids: FxIndexMap<u32, Value>,
    id_to_short: FxIndexMap<IdKey, u32>,
    field_length: FxIndexMap<u32, Vec<Option<u32>>>,
    avg_field_length: Vec<Option<f64>>,
    next_id: u32,
    /// Stored fields per document as `(name ID, value)` pairs; names are
    /// interned in `stored_field_names` instead of cloning a keyed map per
    /// document.
    stored_fields: FxIndexMap<u32, Vec<(u16, Value)>>,
    stored_field_names: Vec<String>,
    dirt_count: u32,
}

impl Engine {
    pub fn new(options: &Value) -> Result<Engine> {
        let Some(object) = options.as_object() else {
            return Err("MiniSearch: option \"fields\" must be provided".to_string());
        };
        let Some(fields_value) = object.get("fields") else {
            return Err("MiniSearch: option \"fields\" must be provided".to_string());
        };
        let fields = string_array(fields_value, "fields")?;

        let store_fields = match object.get("storeFields") {
            Some(value) => string_array(value, "storeFields")?,
            None => Vec::new(),
        };
        let id_field = object
            .get("idField")
            .and_then(Value::as_str)
            .unwrap_or("id")
            .to_string();

        let default_search = SearchOptions::default()
            .overlay(object.get("searchOptions").unwrap_or(&Value::Null))?;

        let mut auto_suggest_base = default_search.clone();
        auto_suggest_base.combine_with = Combine::And;
        auto_suggest_base.prefix = Prefix::LastTerm;
        let default_auto_suggest =
            auto_suggest_base.overlay(object.get("autoSuggestOptions").unwrap_or(&Value::Null))?;

        let field_ids = fields
            .iter()
            .enumerate()
            .map(|(position, field)| (field.clone(), position as u32))
            .collect();

        Ok(Engine {
            fields,
            field_ids,
            stored_field_names: store_fields.clone(),
            store_fields,
            id_field,
            default_search,
            default_auto_suggest,
            auto_vacuum: parse_auto_vacuum(object.get("autoVacuum")),
            index: RadixTree::new(),
            document_count: 0,
            document_ids: FxIndexMap::default(),
            id_to_short: FxIndexMap::default(),
            field_length: FxIndexMap::default(),
            avg_field_length: Vec::new(),
            next_id: 0,
            stored_fields: FxIndexMap::default(),
            dirt_count: 0,
        })
    }

    // -- Indexing ------------------------------------------------------------

    fn tokenize(text: &str) -> Vec<String> {
        tokenizer().split(text).map(str::to_string).collect()
    }

    fn process_term(term: &str) -> Option<String> {
        if term.is_empty() {
            return None;
        }
        // Fast path: lowercasing is the identity for ASCII text without
        // uppercase letters, the common case for indexed prose.
        if term
            .bytes()
            .all(|b| b.is_ascii() && !b.is_ascii_uppercase())
        {
            Some(term.to_string())
        } else {
            Some(term.to_lowercase())
        }
    }

    /// Extracts the document ID, mirroring the original's `extractField` +
    /// null check.
    fn document_id(&self, document: &Value) -> Result<Value> {
        match document.get(&self.id_field) {
            Some(value) if !value.is_null() => Ok(value.clone()),
            _ => Err(format!(
                "MiniSearch: document does not have ID field \"{}\"",
                self.id_field
            )),
        }
    }

    /// Tokenizes every indexed field of a document. Fields that are missing
    /// or null are skipped, and other values are coerced to strings with
    /// JavaScript semantics. Tokens are streamed straight from the splitter:
    /// raw tokens are counted for uniqueness while processed terms are
    /// collected, without an intermediate token list.
    fn tokenized_fields(&self, document: &Value) -> Vec<FieldTokens> {
        self.fields
            .iter()
            .filter_map(|field| {
                let value = document.get(field).filter(|value| !value.is_null())?;
                let text = js_text(value);

                let mut unique = FxHashSet::default();
                let mut terms = Vec::new();
                for token in tokenizer().split(&text) {
                    unique.insert(token);
                    if let Some(term) = Self::process_term(token) {
                        terms.push(term);
                    }
                }

                Some(FieldTokens {
                    field_id: self.field_ids[field],
                    unique_tokens: unique.len() as u32,
                    terms,
                })
            })
            .collect()
    }

    pub fn add(&mut self, document: &Value) -> Result<()> {
        let id = self.document_id(document)?;
        let key = id_key(&id);
        if self.id_to_short.contains_key(&key) {
            return Err(format!("MiniSearch: duplicate ID {}", js_to_string(&id)));
        }

        let short_id = self.next_id;
        self.next_id += 1;
        self.id_to_short.insert(key, short_id);
        self.document_ids.insert(short_id, id);
        self.document_count += 1;

        self.save_stored_fields(short_id, document);

        for field in self.tokenized_fields(document) {
            self.add_field_length(
                short_id,
                field.field_id,
                self.document_count - 1,
                field.unique_tokens,
            );
            for term in &field.terms {
                self.add_term(field.field_id, short_id, term);
            }
        }
        Ok(())
    }

    pub fn add_all(&mut self, documents: &[Value]) -> Result<()> {
        for document in documents {
            self.add(document)?;
        }
        Ok(())
    }

    /// Adds all documents from a JSON array string. Parsing in native code is
    /// much cheaper than converting an array of JavaScript objects through
    /// the bindings.
    pub fn add_all_json(&mut self, json_text: &str) -> Result<()> {
        let documents: Vec<Value> = serde_json::from_str(json_text)
            .map_err(|error| format!("MiniSearch: invalid JSON documents: {error}"))?;
        self.add_all(&documents)
    }

    pub fn remove(&mut self, document: &Value) -> Result<()> {
        let id = self.document_id(document)?;
        let key = id_key(&id);
        let Some(&short_id) = self.id_to_short.get(&key) else {
            return Err(format!(
                "MiniSearch: cannot remove document with ID {}: it is not in the index",
                js_to_string(&id)
            ));
        };

        for field in self.tokenized_fields(document) {
            self.remove_field_length(field.field_id, self.document_count, field.unique_tokens);
            for term in &field.terms {
                self.remove_term(field.field_id, short_id, term);
            }
        }

        self.stored_fields.shift_remove(&short_id);
        self.document_ids.shift_remove(&short_id);
        self.id_to_short.shift_remove(&key);
        self.field_length.shift_remove(&short_id);
        self.document_count -= 1;
        Ok(())
    }

    pub fn remove_all(&mut self) {
        self.index.clear();
        self.document_count = 0;
        self.document_ids.clear();
        self.id_to_short.clear();
        self.field_length.clear();
        self.avg_field_length.clear();
        self.stored_fields.clear();
        self.next_id = 0;
    }

    pub fn discard(&mut self, id: &Value) -> Result<()> {
        let key = id_key(id);
        let Some(&short_id) = self.id_to_short.get(&key) else {
            return Err(format!(
                "MiniSearch: cannot discard document with ID {}: it is not in the index",
                js_to_string(id)
            ));
        };

        self.id_to_short.shift_remove(&key);
        self.document_ids.shift_remove(&short_id);
        self.stored_fields.shift_remove(&short_id);

        if let Some(lengths) = self.field_length.shift_remove(&short_id) {
            for (position, length) in lengths.into_iter().enumerate() {
                if let Some(length) = length {
                    self.remove_field_length(position as u32, self.document_count, length);
                }
            }
        }

        self.document_count -= 1;
        self.dirt_count += 1;
        self.maybe_auto_vacuum();
        Ok(())
    }

    pub fn discard_all(&mut self, ids: &[Value]) -> Result<()> {
        let auto_vacuum = self.auto_vacuum.take();
        let result = ids.iter().try_for_each(|id| self.discard(id));
        self.auto_vacuum = auto_vacuum;
        result?;
        self.maybe_auto_vacuum();
        Ok(())
    }

    pub fn replace(&mut self, document: &Value) -> Result<()> {
        let id = document.get(&self.id_field).cloned().unwrap_or(Value::Null);
        self.discard(&id)?;
        self.add(document)
    }

    fn maybe_auto_vacuum(&mut self) {
        let Some(conditions) = &self.auto_vacuum else {
            return;
        };
        if self.dirt_count >= conditions.min_dirt_count
            && self.dirt_factor() >= conditions.min_dirt_factor
        {
            self.vacuum();
        }
    }

    /// Removes all references to discarded documents from the inverted index.
    /// The original performs this in timed batches to avoid blocking the
    /// JavaScript main thread; native code has no such constraint, so
    /// vacuuming is synchronous and complete.
    pub fn vacuum(&mut self) {
        let initial_dirt_count = self.dirt_count;

        let mut stale: Vec<(String, Vec<(u32, u32)>)> = Vec::new();
        self.index.for_each(&mut |term, fields_data| {
            let mut term_stale = Vec::new();
            for (field_id, doc_freqs) in fields_data.iter() {
                for (doc_id, _) in doc_freqs.iter() {
                    if !self.document_ids.contains_key(doc_id) {
                        term_stale.push((*field_id, *doc_id));
                    }
                }
            }
            if !term_stale.is_empty() {
                stale.push((term.to_string(), term_stale));
            }
        });

        for (term, refs) in stale {
            let Some(fields_data) = self.index.get_mut(&term) else {
                continue;
            };
            for (field_id, doc_id) in refs {
                let remove_field = fields_data
                    .get(field_id)
                    .is_some_and(|doc_freqs| doc_freqs.len() <= 1);
                if remove_field {
                    fields_data.remove(field_id);
                } else if let Some(doc_freqs) = fields_data.get_mut(field_id) {
                    doc_freqs.remove(doc_id);
                }
            }
            if fields_data.is_empty() {
                self.index.remove(&term);
            }
        }

        self.dirt_count -= initial_dirt_count;
    }

    pub fn has(&self, id: &Value) -> bool {
        self.id_to_short.contains_key(&id_key(id))
    }

    pub fn get_stored_fields(&self, id: &Value) -> Option<Value> {
        let short_id = self.id_to_short.get(&id_key(id))?;
        let entries = self.stored_fields.get(short_id)?;
        let mut fields = JsonMap::new();
        for (id, value) in entries {
            fields.insert(self.stored_field_names[*id as usize].clone(), value.clone());
        }
        Some(Value::Object(fields))
    }

    pub fn document_count(&self) -> u32 {
        self.document_count
    }

    pub fn term_count(&self) -> u32 {
        self.index.len() as u32
    }

    pub fn dirt_count(&self) -> u32 {
        self.dirt_count
    }

    pub fn dirt_factor(&self) -> f64 {
        f64::from(self.dirt_count) / f64::from(1 + self.document_count + self.dirt_count)
    }

    fn add_term(&mut self, field_id: u32, document_id: u32, term: &str) {
        let freq = self
            .index
            .fetch_with(term, FieldTermData::default)
            .get_or_insert_with(field_id, DocFreqs::default)
            .get_or_insert_with(document_id, || 0);
        *freq += 1;
    }

    /// Removes one reference to `term` for the given document and field. A
    /// term frequency above one is only decremented, exactly like the
    /// original. Warns and leaves the index unchanged when the reference is
    /// absent, which indicates the document changed before removal.
    fn remove_term(&mut self, field_id: u32, document_id: u32, term: &str) {
        let Some(index_data) = self.index.get_mut(term) else {
            self.warn_document_changed(document_id, field_id, term);
            return;
        };

        let freq = index_data
            .get(field_id)
            .and_then(|field_index| field_index.get(document_id))
            .copied();
        match freq {
            None => {
                self.warn_document_changed(document_id, field_id, term);
                return;
            }
            Some(freq) if freq <= 1 => {
                let remove_field = index_data
                    .get(field_id)
                    .is_some_and(|field_index| field_index.len() <= 1);
                if remove_field {
                    index_data.remove(field_id);
                } else if let Some(field_index) = index_data.get_mut(field_id) {
                    field_index.remove(document_id);
                }
            }
            Some(freq) => {
                if let Some(field_index) = index_data.get_mut(field_id) {
                    field_index.insert(document_id, freq - 1);
                }
            }
        }

        if index_data.is_empty() {
            self.index.remove(term);
        }
    }

    fn warn_document_changed(&self, short_id: u32, field_id: u32, term: &str) {
        let Some(field_name) = self
            .field_ids
            .iter()
            .find(|(_, &id)| id == field_id)
            .map(|(name, _)| name)
        else {
            return;
        };
        let document_id = self
            .document_ids
            .get(&short_id)
            .map(js_to_string)
            .unwrap_or_else(|| "undefined".to_string());
        eprintln!(
            "MiniSearch: document with ID {document_id} has changed before removal: term \"{term}\" was not present in field \"{field_name}\". Removing a document after it has changed can corrupt the index!"
        );
    }

    fn add_field_length(&mut self, document_id: u32, field_id: u32, count: u32, length: u32) {
        let lengths = self.field_length.entry(document_id).or_default();
        let position = field_id as usize;
        if lengths.len() <= position {
            lengths.resize(position + 1, None);
        }
        lengths[position] = Some(length);

        if self.avg_field_length.len() <= position {
            self.avg_field_length.resize(position + 1, None);
        }
        let average = self.avg_field_length[position].unwrap_or(0.0);
        let total = average * f64::from(count) + f64::from(length);
        self.avg_field_length[position] = Some(total / f64::from(count + 1));
    }

    fn remove_field_length(&mut self, field_id: u32, count: u32, length: u32) {
        let position = field_id as usize;
        if self.avg_field_length.len() <= position {
            self.avg_field_length.resize(position + 1, None);
        }
        if count == 1 {
            self.avg_field_length[position] = Some(0.0);
            return;
        }
        let average = self.avg_field_length[position].unwrap_or(0.0);
        let total = average * f64::from(count) - f64::from(length);
        self.avg_field_length[position] = Some(total / f64::from(count - 1));
    }

    fn save_stored_fields(&mut self, document_id: u32, document: &Value) {
        if self.store_fields.is_empty() {
            return;
        }
        // The first `store_fields.len()` interned name IDs are the store
        // fields themselves, in order.
        let entries = self.stored_fields.entry(document_id).or_default();
        for (index, field) in self.store_fields.iter().enumerate() {
            if let Some(value) = document.get(field) {
                let id = index as u16;
                match entries.iter_mut().find(|(existing, _)| *existing == id) {
                    Some((_, existing)) => *existing = value.clone(),
                    None => entries.push((id, value.clone())),
                }
            }
        }
    }

    /// Interns a stored-field name, for indexes loaded from serialized data
    /// whose stored keys are not limited to `store_fields`.
    fn stored_name_id(&mut self, name: &str) -> u16 {
        match self.stored_field_names.iter().position(|n| n == name) {
            Some(index) => index as u16,
            None => {
                self.stored_field_names.push(name.to_string());
                (self.stored_field_names.len() - 1) as u16
            }
        }
    }

    // -- Search --------------------------------------------------------------

    pub fn search(&mut self, query: &Value, options: &Value) -> Result<Vec<Value>> {
        let base = self.default_search.clone();
        let (hits, interner) = self.search_hits(query, options, &base)?;
        Ok(self.hits_to_values(&hits, &interner))
    }

    /// Searches and serializes the results directly to a JSON string,
    /// avoiding intermediate value trees. This is the fast path used by the
    /// JavaScript wrapper together with `JSON.parse`.
    pub fn search_to_json_string(&mut self, query: &Value, options: &Value) -> Result<String> {
        let base = self.default_search.clone();
        let (hits, interner) = self.search_hits(query, options, &base)?;
        Ok(self.hits_to_json_string(&hits, &interner))
    }

    pub fn wildcard_search(&self, options: &Value) -> Result<Vec<Value>> {
        let _ = options; // Document boosting functions are handled by the wrapper.
        let interner = Interner::default();
        let hits: Vec<Hit> = self
            .document_ids
            .keys()
            .map(|&doc_id| Hit {
                doc_id,
                score: 1.0,
                terms: Vec::new(),
                matches: Vec::new(),
            })
            .collect();
        Ok(self.hits_to_values(&hits, &interner))
    }

    pub fn auto_suggest(&mut self, query: &str, options: &Value) -> Result<Vec<Value>> {
        let base = self.default_auto_suggest.clone();
        let query = Value::String(query.to_string());
        let (hits, interner) = self.search_hits(&query, options, &base)?;

        let mut suggestions: IndexMap<String, (f64, Vec<String>, u32)> = IndexMap::new();
        for hit in &hits {
            // Suggestions are built from the matched document terms (the
            // `terms` field of each result), not the query terms.
            let terms: Vec<String> = hit
                .matches
                .iter()
                .map(|(term, _)| interner.resolve(*term).to_string())
                .collect();
            let phrase = terms.join(" ");
            match suggestions.get_mut(&phrase) {
                Some((score, _, count)) => {
                    *score += hit.score;
                    *count += 1;
                }
                None => {
                    suggestions.insert(phrase, (hit.score, terms, 1));
                }
            }
        }

        let mut results: Vec<Value> = suggestions
            .into_iter()
            .map(|(suggestion, (score, terms, count))| {
                json!({
                    "suggestion": suggestion,
                    "terms": terms,
                    "score": score / f64::from(count),
                })
            })
            .collect();
        sort_by_score_desc(&mut results, |result| {
            result.get("score").and_then(as_f64).unwrap_or(0.0)
        });
        Ok(results)
    }

    /// Runs a search resolving option overlays against the given base
    /// options. Auto-suggestions use a different base than plain searches
    /// (combining with AND, and prefix-matching the last query term).
    fn search_hits(
        &mut self,
        query: &Value,
        options: &Value,
        base: &SearchOptions,
    ) -> Result<(Vec<Hit>, Interner)> {
        let mut interner = Interner::default();
        let raw_results = self.execute_query(query, options, base, &mut interner)?;

        let mut hits: Vec<Hit> = raw_results
            .into_iter()
            .map(|(doc_id, raw)| {
                // Quality is based on the matched query terms, as opposed to
                // the matched document terms, which can differ under prefix
                // and fuzzy matching.
                let quality = raw.terms.len().max(1) as f64;
                Hit {
                    doc_id,
                    score: raw.score * quality,
                    terms: raw.terms,
                    matches: raw.matches,
                }
            })
            .collect();

        sort_by_score_desc(&mut hits, |hit| hit.score);
        Ok((hits, interner))
    }

    fn execute_query(
        &mut self,
        query: &Value,
        raw_options: &Value,
        base: &SearchOptions,
        interner: &mut Interner,
    ) -> Result<RawResult> {
        if let Some(queries) = query.get("queries").and_then(Value::as_array) {
            // A combination node: its own keys override the inherited
            // options, and are inherited by its subqueries.
            let merged = shallow_merge(raw_options, query);
            let subquery_options = without_key(&merged, "queries");
            let mut results = Vec::with_capacity(queries.len());
            for subquery in queries {
                results.push(self.execute_query(subquery, &subquery_options, base, interner)?);
            }
            let combine = match subquery_options.get("combineWith") {
                Some(value) => parse_combine(value)?,
                None => Combine::Or,
            };
            return Ok(combine_results(results, combine));
        }

        let Some(text) = query.as_str() else {
            return Err("MiniSearch: query must be a string or a query combination".to_string());
        };

        let options = base.overlay(raw_options)?;
        let terms: Vec<String> = Self::tokenize(text)
            .iter()
            .filter_map(|token| Self::process_term(token))
            .collect();

        let mut results = Vec::with_capacity(terms.len());
        for (position, term) in terms.iter().enumerate() {
            let prefix = match options.prefix {
                Prefix::Enabled(enabled) => enabled,
                Prefix::LastTerm => position == terms.len() - 1,
            };
            results.push(self.execute_query_spec(term, prefix, &options, interner));
        }
        Ok(combine_results(results, options.combine_with))
    }

    /// Scores one query term: an exact pass, then prefix expansion, then
    /// fuzzy expansion, each weighted by its distance from the query term.
    fn execute_query_spec(
        &mut self,
        term: &str,
        prefix: bool,
        options: &SearchOptions,
        interner: &mut Interner,
    ) -> RawResult {
        let search_fields = options.fields.as_ref().unwrap_or(&self.fields);
        // A falsy boost (0) falls back to 1, matching the original's
        // `getOwnProperty(options.boost, field) || 1`.
        let field_boosts: Vec<(String, f64)> = search_fields
            .iter()
            .map(|field| {
                let boost = options.boost.get(field).copied().unwrap_or(0.0);
                (field.clone(), if boost == 0.0 { 1.0 } else { boost })
            })
            .collect();

        let scoring = Scoring {
            source_term: interner.intern(term),
            field_boosts: &field_boosts,
            bm25: options.bm25,
        };
        let mut results = RawResult::default();
        let mut stale = Vec::new();

        // Exact match.
        self.term_results(
            &scoring,
            term,
            scoring.source_term,
            1.0,
            &mut results,
            &mut stale,
        );
        self.apply_stale(term, &mut stale);

        let mut prefix_terms: Vec<String> = Vec::new();
        if prefix {
            self.index.for_each_prefix(term, &mut |derived, _| {
                prefix_terms.push(derived.to_string());
            });
        }
        let fuzzy_matches = self.fuzzy_matches(term, options);
        // For the prefix-overlap skip check in the fuzzy pass: a hash set
        // beats a linear scan when both expansions are large.
        let prefix_term_set: FxHashSet<&str> = if fuzzy_matches.is_empty() {
            FxHashSet::default()
        } else {
            prefix_terms.iter().map(String::as_str).collect()
        };

        let query_length = term.chars().count() as f64;
        for derived in &prefix_terms {
            let term_length = derived.chars().count() as f64;
            let distance = term_length - query_length;
            if distance == 0.0 {
                continue; // Skip the exact match.
            }
            // Weight gradually approaches 0 as distance grows. Prefix matches
            // decay more slowly than fuzzy matches, because they stay relevant
            // at larger length differences.
            let weight = options.weights.prefix * term_length / (term_length + 0.3 * distance);
            let derived_id = interner.intern(derived);
            self.term_results(
                &scoring,
                derived,
                derived_id,
                weight,
                &mut results,
                &mut stale,
            );
            self.apply_stale(derived, &mut stale);
        }

        for (derived, distance) in &fuzzy_matches {
            if *distance == 0 {
                continue; // Skip the exact match.
            }
            // A term matched by prefix search is always scored as a prefix
            // result, exactly as in the original.
            if prefix_term_set.contains(derived.as_str()) {
                continue;
            }
            let term_length = derived.chars().count() as f64;
            let weight = options.weights.fuzzy * term_length / (term_length + *distance as f64);
            let derived_id = interner.intern(derived);
            self.term_results(
                &scoring,
                derived,
                derived_id,
                weight,
                &mut results,
                &mut stale,
            );
            self.apply_stale(derived, &mut stale);
        }

        results
    }

    /// Resolves the fuzzy setting for a term into matching index terms with
    /// their edit distances. Fractional settings scale with the term length,
    /// capped by `maxFuzzy`.
    fn fuzzy_matches(&self, term: &str, options: &SearchOptions) -> Vec<(String, usize)> {
        let fraction = match options.fuzzy {
            Fuzzy::Off => return Vec::new(),
            Fuzzy::Auto => 0.2,
            Fuzzy::Value(value) => value,
        };
        let term_length = term.chars().count() as f64;
        let max_distance = if fraction < 1.0 {
            options.max_fuzzy.min((term_length * fraction).round())
        } else {
            fraction
        };
        if max_distance >= 1.0 {
            self.index.fuzzy_get(term, max_distance as usize)
        } else {
            Vec::new()
        }
    }

    /// Scores one derived term against all searched fields, accumulating into
    /// `results`. References to discarded documents are recorded in `stale`
    /// for removal by the caller; the running `matching_fields` count is
    /// decremented as they are found, mirroring the original's in-loop
    /// bookkeeping.
    fn term_results(
        &self,
        scoring: &Scoring,
        derived_term: &str,
        derived_id: u16,
        term_weight: f64,
        results: &mut RawResult,
        stale: &mut Vec<(u32, u32)>,
    ) {
        let Some(field_term_data) = self.index.get(derived_term) else {
            return;
        };

        for (field, field_boost) in scoring.field_boosts {
            let Some(&field_id) = self.field_ids.get(field) else {
                continue;
            };
            let Some(field_term_freqs) = field_term_data.get(field_id) else {
                continue;
            };

            let mut matching_fields = field_term_freqs.len() as u32;
            let avg_field_length = self
                .avg_field_length
                .get(field_id as usize)
                .copied()
                .flatten()
                .unwrap_or(0.0);

            for &(doc_id, term_freq) in field_term_freqs.iter() {
                if !self.document_ids.contains_key(&doc_id) {
                    stale.push((field_id, doc_id));
                    matching_fields -= 1;
                    continue;
                }

                let field_length = self
                    .field_length
                    .get(&doc_id)
                    .and_then(|lengths| lengths.get(field_id as usize).copied().flatten())
                    .unwrap_or(0);

                let raw_score = bm25_score(
                    f64::from(term_freq),
                    f64::from(matching_fields),
                    f64::from(self.document_count),
                    f64::from(field_length),
                    avg_field_length,
                    scoring.bm25,
                );
                let weighted_score = term_weight * field_boost * raw_score;

                match results.get_mut(&doc_id) {
                    Some(result) => {
                        result.score += weighted_score;
                        result.add_term(scoring.source_term);
                        result.add_match(derived_id, field_id);
                    }
                    None => {
                        results.insert(
                            doc_id,
                            RawScore {
                                score: weighted_score,
                                terms: vec![scoring.source_term],
                                matches: vec![(derived_id, vec![field_id])],
                            },
                        );
                    }
                }
            }
        }
    }

    /// Applies the stale-reference removals collected by `term_results`. The
    /// original removes them one reference at a time during search; a stale
    /// reference with a term frequency above one is only decremented, and is
    /// fully cleaned up by later searches or vacuuming.
    fn apply_stale(&mut self, derived_term: &str, stale: &mut Vec<(u32, u32)>) {
        for (field_id, doc_id) in stale.drain(..) {
            self.remove_term(field_id, doc_id, derived_term);
        }
    }

    // -- Result assembly -----------------------------------------------------

    /// Field names keyed by field ID, for result assembly.
    fn field_names(&self) -> FxIndexMap<u32, &str> {
        self.field_ids
            .iter()
            .map(|(name, &id)| (id, name.as_str()))
            .collect()
    }

    fn hits_to_values(&self, hits: &[Hit], interner: &Interner) -> Vec<Value> {
        let field_names = self.field_names();
        let field_name = |id: &u32| json!(field_names.get(id).copied().unwrap_or(""));

        hits.iter()
            .map(|hit| {
                let mut result = JsonMap::new();
                result.insert("id".to_string(), self.document_ids[&hit.doc_id].clone());
                result.insert("score".to_string(), json!(hit.score));
                result.insert(
                    "terms".to_string(),
                    Value::Array(
                        hit.matches
                            .iter()
                            .map(|(term, _)| json!(interner.resolve(*term)))
                            .collect(),
                    ),
                );
                result.insert(
                    "queryTerms".to_string(),
                    Value::Array(
                        hit.terms
                            .iter()
                            .map(|term| json!(interner.resolve(*term)))
                            .collect(),
                    ),
                );
                let mut matches = JsonMap::new();
                for (term, fields) in &hit.matches {
                    matches.insert(
                        interner.resolve(*term).to_string(),
                        Value::Array(fields.iter().map(field_name).collect()),
                    );
                }
                result.insert("match".to_string(), Value::Object(matches));

                if let Some(stored) = self.stored_fields.get(&hit.doc_id) {
                    for (id, value) in stored {
                        result.insert(self.stored_field_names[*id as usize].clone(), value.clone());
                    }
                }
                Value::Object(result)
            })
            .collect()
    }

    /// Writes the result array as JSON in a single pass. Stored fields are
    /// emitted after the core keys with duplicate keys allowed: `JSON.parse`
    /// keeps the last occurrence, replicating the `Object.assign` override
    /// semantics of the original result assembly.
    fn hits_to_json_string(&self, hits: &[Hit], interner: &Interner) -> String {
        let field_names = self.field_names();

        let mut out = String::with_capacity(hits.len() * 128 + 2);
        out.push('[');
        for (index, hit) in hits.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str("{\"id\":");
            json_value_to(&mut out, &self.document_ids[&hit.doc_id]);
            out.push_str(",\"score\":");
            json_number_to(&mut out, hit.score);
            out.push_str(",\"terms\":[");
            for (position, (term, _)) in hit.matches.iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                json_string_to(&mut out, interner.resolve(*term));
            }
            out.push_str("],\"queryTerms\":[");
            for (position, term) in hit.terms.iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                json_string_to(&mut out, interner.resolve(*term));
            }
            out.push_str("],\"match\":{");
            for (position, (term, fields)) in hit.matches.iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                json_string_to(&mut out, interner.resolve(*term));
                out.push_str(":[");
                for (field_position, field_id) in fields.iter().enumerate() {
                    if field_position > 0 {
                        out.push(',');
                    }
                    json_string_to(&mut out, field_names.get(field_id).copied().unwrap_or(""));
                }
                out.push(']');
            }
            out.push('}');
            if let Some(stored) = self.stored_fields.get(&hit.doc_id) {
                for (id, value) in stored {
                    out.push(',');
                    json_string_to(&mut out, &self.stored_field_names[*id as usize]);
                    out.push(':');
                    json_value_to(&mut out, value);
                }
            }
            out.push('}');
        }
        out.push(']');
        out
    }

    // -- Serialization -------------------------------------------------------

    pub fn to_json(&self) -> Value {
        let mut index_entries = Vec::new();
        self.index.for_each(&mut |term, fields_data| {
            let mut data = JsonMap::new();
            for (field_id, doc_freqs) in fields_data.iter() {
                let mut freqs = JsonMap::new();
                for (doc_id, freq) in doc_freqs.iter() {
                    freqs.insert(doc_id.to_string(), json!(freq));
                }
                data.insert(field_id.to_string(), Value::Object(freqs));
            }
            index_entries.push(json!([term, Value::Object(data)]));
        });

        let mut document_ids = JsonMap::new();
        for (short_id, id) in &self.document_ids {
            document_ids.insert(short_id.to_string(), id.clone());
        }
        let mut field_lengths = JsonMap::new();
        for (short_id, lengths) in &self.field_length {
            field_lengths.insert(
                short_id.to_string(),
                Value::Array(
                    lengths
                        .iter()
                        .map(|length| match length {
                            Some(length) => json!(length),
                            None => Value::Null,
                        })
                        .collect(),
                ),
            );
        }
        let mut stored = JsonMap::new();
        for (short_id, entries) in &self.stored_fields {
            let mut fields = JsonMap::new();
            for (id, value) in entries {
                fields.insert(self.stored_field_names[*id as usize].clone(), value.clone());
            }
            stored.insert(short_id.to_string(), Value::Object(fields));
        }
        let mut field_ids = JsonMap::new();
        for (field, id) in &self.field_ids {
            field_ids.insert(field.clone(), json!(id));
        }

        json!({
            "documentCount": self.document_count,
            "nextId": self.next_id,
            "documentIds": document_ids,
            "fieldIds": field_ids,
            "fieldLength": field_lengths,
            "averageFieldLength": self.avg_field_length.iter().map(|avg| match avg {
                Some(value) => json!(value),
                None => Value::Null,
            }).collect::<Vec<_>>(),
            "storedFields": stored,
            "dirtCount": self.dirt_count,
            "index": index_entries,
            "serializationVersion": SERIALIZATION_VERSION,
        })
    }

    /// Serializes the index directly to a JSON string, equivalent to
    /// `JSON.stringify` of `to_json` but without materializing value trees
    /// or crossing the bindings with nested objects.
    pub fn to_json_string(&self) -> String {
        let mut out = String::with_capacity(4096);
        out.push_str("{\"documentCount\":");
        out.push_str(&self.document_count.to_string());
        out.push_str(",\"nextId\":");
        out.push_str(&self.next_id.to_string());

        out.push_str(",\"documentIds\":{");
        for (position, (short_id, id)) in self.document_ids.iter().enumerate() {
            if position > 0 {
                out.push(',');
            }
            json_string_to(&mut out, &short_id.to_string());
            out.push(':');
            json_value_to(&mut out, id);
        }

        out.push_str("},\"fieldIds\":{");
        for (position, (field, id)) in self.field_ids.iter().enumerate() {
            if position > 0 {
                out.push(',');
            }
            json_string_to(&mut out, field);
            out.push(':');
            out.push_str(&id.to_string());
        }

        out.push_str("},\"fieldLength\":{");
        for (position, (short_id, lengths)) in self.field_length.iter().enumerate() {
            if position > 0 {
                out.push(',');
            }
            json_string_to(&mut out, &short_id.to_string());
            out.push_str(":[");
            for (length_position, length) in lengths.iter().enumerate() {
                if length_position > 0 {
                    out.push(',');
                }
                match length {
                    Some(length) => out.push_str(&length.to_string()),
                    None => out.push_str("null"),
                }
            }
            out.push(']');
        }

        out.push_str("},\"averageFieldLength\":[");
        for (position, average) in self.avg_field_length.iter().enumerate() {
            if position > 0 {
                out.push(',');
            }
            match average {
                Some(average) => json_number_to(&mut out, *average),
                None => out.push_str("null"),
            }
        }

        out.push_str("],\"storedFields\":{");
        for (position, (short_id, entries)) in self.stored_fields.iter().enumerate() {
            if position > 0 {
                out.push(',');
            }
            json_string_to(&mut out, &short_id.to_string());
            out.push_str(":{");
            for (entry_position, (id, value)) in entries.iter().enumerate() {
                if entry_position > 0 {
                    out.push(',');
                }
                json_string_to(&mut out, &self.stored_field_names[*id as usize]);
                out.push(':');
                json_value_to(&mut out, value);
            }
            out.push('}');
        }

        out.push_str("},\"dirtCount\":");
        out.push_str(&self.dirt_count.to_string());

        out.push_str(",\"index\":[");
        let mut first_term = true;
        self.index.for_each(&mut |term, fields_data| {
            if !first_term {
                out.push(',');
            }
            first_term = false;
            out.push('[');
            json_string_to(&mut out, term);
            out.push_str(",{");
            for (field_position, (field_id, doc_freqs)) in fields_data.iter().enumerate() {
                if field_position > 0 {
                    out.push(',');
                }
                json_string_to(&mut out, &field_id.to_string());
                out.push_str(":{");
                for (doc_position, (doc_id, freq)) in doc_freqs.iter().enumerate() {
                    if doc_position > 0 {
                        out.push(',');
                    }
                    json_string_to(&mut out, &doc_id.to_string());
                    out.push(':');
                    out.push_str(&freq.to_string());
                }
                out.push('}');
            }
            out.push_str("}]");
        });

        out.push_str("],\"serializationVersion\":");
        out.push_str(&SERIALIZATION_VERSION.to_string());
        out.push('}');
        out
    }

    pub fn load_json(json_text: &str, options: &Value) -> Result<Engine> {
        if options.is_null() {
            return Err(
                "MiniSearch: loadJSON should be given the same options used when serializing the index"
                    .to_string(),
            );
        }
        let data: Value = serde_json::from_str(json_text)
            .map_err(|error| format!("MiniSearch: invalid JSON index: {error}"))?;
        Self::load_js(&data, options)
    }

    fn load_js(data: &Value, options: &Value) -> Result<Engine> {
        let version = data.get("serializationVersion").and_then(Value::as_u64);
        if version != Some(1) && version != Some(2) {
            return Err(
                "MiniSearch: cannot deserialize an index created with an incompatible version"
                    .to_string(),
            );
        }

        let mut engine = Engine::new(options)?;

        engine.document_count = data
            .get("documentCount")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        engine.next_id = data.get("nextId").and_then(Value::as_u64).unwrap_or(0) as u32;
        engine.dirt_count = data.get("dirtCount").and_then(Value::as_u64).unwrap_or(0) as u32;

        if let Some(ids) = data.get("documentIds").and_then(Value::as_object) {
            for (short_id, id) in ids {
                let short_id = parse_short_id(short_id)?;
                engine.document_ids.insert(short_id, id.clone());
                engine.id_to_short.insert(id_key(id), short_id);
            }
        }

        if let Some(lengths) = data.get("fieldLength").and_then(Value::as_object) {
            for (short_id, list) in lengths {
                let list = list
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| item.as_u64().map(|length| length as u32))
                            .collect()
                    })
                    .unwrap_or_default();
                engine.field_length.insert(parse_short_id(short_id)?, list);
            }
        }

        if let Some(averages) = data.get("averageFieldLength").and_then(Value::as_array) {
            engine.avg_field_length = averages.iter().map(as_f64).collect();
        }

        if let Some(stored) = data.get("storedFields").and_then(Value::as_object) {
            for (short_id, fields) in stored {
                if let Some(fields) = fields.as_object() {
                    let short_id = parse_short_id(short_id)?;
                    let mut entries = Vec::with_capacity(fields.len());
                    for (name, value) in fields {
                        entries.push((engine.stored_name_id(name), value.clone()));
                    }
                    engine.stored_fields.insert(short_id, entries);
                }
            }
        }

        if let Some(field_ids) = data.get("fieldIds").and_then(Value::as_object) {
            engine.field_ids.clear();
            for (field, id) in field_ids {
                let Some(id) = id.as_u64() else {
                    return Err("MiniSearch: invalid field ID in serialized index".to_string());
                };
                engine.field_ids.insert(field.clone(), id as u32);
            }
        }

        if let Some(entries) = data.get("index").and_then(Value::as_array) {
            for entry in entries {
                let Some(pair) = entry.as_array() else {
                    return Err("MiniSearch: invalid index entry in serialized index".to_string());
                };
                let (Some(term), Some(fields_data)) = (
                    pair.first().and_then(Value::as_str),
                    pair.get(1).and_then(Value::as_object),
                ) else {
                    return Err("MiniSearch: invalid index entry in serialized index".to_string());
                };

                let mut data_map = FieldTermData::default();
                for (field_id, index_entry) in fields_data {
                    let field_id: u32 = field_id.parse().map_err(|_| {
                        "MiniSearch: invalid field ID in serialized index".to_string()
                    })?;
                    // Version 1 nested the entry inside a field named "ds".
                    let index_entry = if version == Some(1) {
                        index_entry.get("ds").unwrap_or(index_entry)
                    } else {
                        index_entry
                    };
                    let Some(freqs) = index_entry.as_object() else {
                        continue;
                    };
                    let mut doc_freqs = DocFreqs::default();
                    for (doc_id, freq) in freqs {
                        doc_freqs
                            .insert(parse_short_id(doc_id)?, freq.as_u64().unwrap_or(0) as u32);
                    }
                    data_map.insert(field_id, doc_freqs);
                }
                engine.index.insert(term, data_map);
            }
        }

        Ok(engine)
    }
}

fn parse_short_id(text: &str) -> Result<u32> {
    text.parse()
        .map_err(|_| "MiniSearch: invalid document ID in serialized index".to_string())
}

/// Stable sort by descending score, matching JavaScript's stable `sort`.
/// Non-comparable (NaN) scores compare as equal, like a comparator returning
/// NaN in JavaScript.
fn sort_by_score_desc<T>(items: &mut [T], score: impl Fn(&T) -> f64) {
    items.sort_by(|a, b| {
        score(b)
            .partial_cmp(&score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// BM25+ scoring, identical to the original implementation.
fn bm25_score(
    term_freq: f64,
    matching_count: f64,
    total_count: f64,
    field_length: f64,
    avg_field_length: f64,
    params: Bm25,
) -> f64 {
    let Bm25 { k, b, d } = params;
    let inv_doc_freq = (1.0 + (total_count - matching_count + 0.5) / (matching_count + 0.5)).ln();
    inv_doc_freq
        * (d + term_freq * (k + 1.0)
            / (term_freq + k * (1.0 - b + b * field_length / avg_field_length)))
}

fn combine_results(results: Vec<RawResult>, combine: Combine) -> RawResult {
    let mut iterator = results.into_iter();
    let Some(mut combined) = iterator.next() else {
        return RawResult::default();
    };
    for other in iterator {
        combined = match combine {
            Combine::Or => combine_or(combined, other),
            Combine::And => combine_and(combined, other),
            Combine::AndNot => combine_and_not(combined, other),
        };
    }
    combined
}

fn combine_or(mut a: RawResult, b: RawResult) -> RawResult {
    for (doc_id, other) in b {
        match a.get_mut(&doc_id) {
            Some(existing) => {
                existing.score += other.score;
                for term in other.terms {
                    existing.add_term(term);
                }
                for (term, fields) in other.matches {
                    existing.assign_match(term, fields);
                }
            }
            None => {
                a.insert(doc_id, other);
            }
        }
    }
    a
}

fn combine_and(mut a: RawResult, b: RawResult) -> RawResult {
    let mut combined = RawResult::default();
    for (doc_id, other) in b {
        let Some(mut existing) = a.shift_remove(&doc_id) else {
            continue;
        };
        existing.score += other.score;
        for term in other.terms {
            existing.add_term(term);
        }
        for (term, fields) in other.matches {
            existing.assign_match(term, fields);
        }
        combined.insert(doc_id, existing);
    }
    combined
}

fn combine_and_not(mut a: RawResult, b: RawResult) -> RawResult {
    for doc_id in b.keys() {
        a.shift_remove(doc_id);
    }
    a
}
