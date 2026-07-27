# API reference

ferrosearch exposes one class, `MiniSearch`, mirroring the [original API
](https://lucaong.github.io/minisearch/classes/MiniSearch.MiniSearch.html)
minus function-valued options (see [Differences from
MiniSearch](../README.md#differences-from-minisearch)). All methods throw
regular JavaScript errors with MiniSearch's error messages.

## Constructor

```javascript
new MiniSearch(options)
```

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `fields` | `string[]` | required | Document fields to index. |
| `storeFields` | `string[]` | `[]` | Fields stored and returned with results. |
| `idField` | `string` | `"id"` | Field uniquely identifying a document. IDs can be strings, numbers, or booleans; `1` and `"1"` are distinct. |
| `searchOptions` | `SearchOptions` | — | Default search options (see below). |
| `autoSuggestOptions` | `SearchOptions` | — | Default auto-suggest options. |
| `autoVacuum` | `boolean \| { minDirtCount?, minDirtFactor? }` | `true` | Automatic vacuuming after discards. `true` uses thresholds 20 / 0.1; falsy thresholds fall back to the defaults. |
| `cache` | `boolean` | `true` | Memoize search results per `(query, options)`, invalidated on any mutation and bypassed while discards are pending vacuum. Additive to the original. |

Throws `MiniSearch: option "fields" must be provided` when `fields` is
missing.

## Search options

Accepted by `search`, `searchJson`, `autoSuggest`, per-query-tree node, and
the constructor defaults.

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `fields` | `string[]` | all indexed fields | Fields to search in. |
| `boost` | `{ [field]: number }` | `{}` | Per-field score multiplier. A falsy boost counts as 1. |
| `prefix` | `boolean` | `false` | Match terms by prefix. |
| `fuzzy` | `boolean \| number` | `false` | `true` = 0.2 × term length; a number < 1 is a fraction of term length; ≥ 1 is a maximum edit distance. |
| `maxFuzzy` | `number` | `6` | Cap for fractional fuzzy distances. |
| `weights` | `{ fuzzy?, prefix? }` | `{ fuzzy: 0.45, prefix: 0.375 }` | Relative weights of fuzzy and prefix matches; exact matches weigh 1. |
| `combineWith` | `"OR" \| "AND" \| "AND_NOT"` | `"OR"` | How term results combine. Case-insensitive; invalid values throw. |
| `bm25` | `{ k, b, d }` | `{ k: 1.2, b: 0.7, d: 0.5 }` | BM25+ parameters. A partial object replaces the whole set. |

## Indexing

| Method | Description |
| --- | --- |
| `add(document)` | Adds one document. Throws on a missing ID field or a duplicate ID. |
| `addAll(documents)` | Adds an array of documents. |
| `addAllJson(json)` | Adds all documents from a JSON array string. Fastest bulk path. |

## Removal lifecycle

| Method | Description |
| --- | --- |
| `remove(document)` | Immediately removes a document. The document must be unchanged since indexing; mismatches warn on stderr. |
| `removeAll(documents?)` | Removes the given documents, or resets the whole index when called without arguments. |
| `discard(id)` | Hides a document by ID; the index is cleaned lazily by searches and vacuuming. |
| `discardAll(ids)` | Discards many IDs with at most one auto-vacuum at the end. |
| `replace(document)` | `discard` + `add` under the same ID. |
| `vacuum()` | Synchronously removes all references to discarded documents. |

## Search

| Method | Description |
| --- | --- |
| `search(query, options?)` | Returns scored results sorted by descending score. `query` is a string, a `{ queries: [...] }` combination tree, or the `MiniSearch.wildcard` symbol. |
| `searchJson(query, options?)` | Same, as a JSON string — pair with `JSON.parse` for the fastest path. |
| `wildcardSearch(options?)` | Results for all documents (the original's `MiniSearch.wildcard`). |
| `autoSuggest(query, options?)` | Ranked query suggestions. Defaults: terms combine with `AND`, prefix search on the last term. |

Each result carries `id`, `score`, `terms` (matched document terms),
`queryTerms`, `match` (term → fields), and all stored fields.

## Inspection

| Member | Description |
| --- | --- |
| `has(id)` | Whether a document with this ID is indexed and searchable. |
| `getStoredFields(id)` | Stored fields for the ID, or `null`. |
| `documentCount` | Number of searchable documents. |
| `termCount` | Number of terms in the index. |
| `dirtCount` | Discards since the last vacuum. |
| `dirtFactor` | Discarded fraction of the index, 0–1. |

## Serialization

| Method | Description |
| --- | --- |
| `toJson()` | Plain-object index representation (MiniSearch serialization version 2). `JSON.stringify` produces a MiniSearch-compatible index. |
| `toJsonString()` | The same serialization written natively in one pass — much faster. |
| `MiniSearch.loadJson(json, options)` | Static. Loads a serialized index (version 1 or 2, from either library). Must receive the same options used when serializing. `loadJSON` is an alias matching the original's name. |
