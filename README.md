# ferrosearch

An in-memory full-text search engine for Bun and Node, implemented in Rust as
a native addon.

ferrosearch provides exact, prefix, and fuzzy matching with BM25+ ranking,
query combination trees, auto-suggestions, and a full document lifecycle. It
is index- and API-compatible with MiniSearch 7: serialized indexes load in
either library, and a test suite of 12,000+ assertions verifies that search
results, scores, result ordering, and error messages are identical.

- [API reference](docs/API.md)
- [Design document](docs/DESIGN.md)

## Features

- Exact match, prefix search, fuzzy match, field boosting, query combination
  trees (`AND` / `OR` / `AND_NOT`).
- Auto-suggestion engine for query auto-completion.
- BM25+ result ranking.
- Documents can be added, removed, discarded, and replaced at any time.
- Index serialization, interchangeable with MiniSearch in both directions.
- JSON fast paths that keep bulk data out of the binding layer.
- Result caching for repeated queries, invalidated on mutation.
- Roughly 3x lower memory usage than an equivalent JavaScript index.

## Installation

```bash
bun add @shift-labs/ferrosearch
```

Prebuilt binaries are published for macOS (arm64, x64) and Linux (x64 and
arm64, glibc and musl). The matching binary is selected automatically at
install time. On other platforms, build from source with Rust installed:
`bun install && bun run build`.

```javascript
import { FerroSearch } from "@shift-labs/ferrosearch";
```

For drop-in migration from MiniSearch, the same class is also exported under
the alias `MiniSearch`.

## Usage

### Basic usage

```javascript
const documents = [
  { id: 1, title: "Moby Dick", text: "Call me Ishmael. Some years ago...", category: "fiction" },
  { id: 2, title: "Zen and the Art of Motorcycle Maintenance", text: "I can see by my watch...", category: "fiction" },
  { id: 3, title: "Neuromancer", text: "The sky above the port was...", category: "fiction" },
  { id: 4, title: "Zen and the Art of Archery", text: "At first sight it must seem...", category: "non-fiction" },
  // ...and more
];

const index = new FerroSearch({
  fields: ["title", "text"], // fields to index for full-text search
  storeFields: ["title", "category"], // fields to return with search results
});

// Index all documents
index.addAll(documents);

// Search with default options
const results = index.search("zen art motorcycle");
// => [
//   { id: 2, title: 'Zen and the Art of Motorcycle Maintenance', category: 'fiction', score: 2.77258, match: { ... } },
//   { id: 4, title: 'Zen and the Art of Archery', category: 'non-fiction', score: 1.38629, match: { ... } }
// ]
```

### Search options

```javascript
// Search only specific fields
index.search("zen", { fields: ["title"] });

// Boost some fields (here "title")
index.search("zen", { boost: { title: 2 } });

// Prefix search ('moto' matches 'motorcycle')
index.search("moto", { prefix: true });

// Fuzzy search with a maximum edit distance of 0.2 * term length, rounded
// to the nearest integer ('ismael' matches 'ishmael')
index.search("ismael", { fuzzy: 0.2 });

// Combine terms with AND instead of the default OR
index.search("zen art", { combineWith: "AND" });

// Default search options can be set upon initialization
const tuned = new FerroSearch({
  fields: ["title", "text"],
  searchOptions: { boost: { title: 2 }, fuzzy: 0.2 },
});
```

### Query combination trees

Subqueries can be combined with different operators and options:

```javascript
// Documents that contain "zen" and ("motorcycle" or "archery")
index.search({
  combineWith: "AND",
  queries: [
    "zen",
    { combineWith: "OR", queries: ["motorcycle", "archery"] },
  ],
});
```

### Wildcard search

The `FerroSearch.wildcard` symbol queries all documents; the equivalent
`wildcardSearch` method is also available:

```javascript
index.search(FerroSearch.wildcard);
index.wildcardSearch();
```

### Auto suggestions

```javascript
index.autoSuggest("zen ar");
// => [ { suggestion: 'zen archery art', terms: [ 'zen', 'archery', 'art' ], score: 1.73332 },
//      { suggestion: 'zen art', terms: [ 'zen', 'art' ], score: 1.21313 } ]

// Fuzzy suggestions for misspelled input
index.autoSuggest("neromancer", { fuzzy: 0.2 });
// => [ { suggestion: 'neuromancer', terms: [ 'neuromancer' ], score: 1.03998 } ]
```

Suggestions are ranked by the relevance of the documents that the suggested
search would return.

### Removing, discarding, and replacing documents

```javascript
// Immediate removal: requires the full, unchanged document
index.remove(documents[0]);

// Discard by ID: faster, cleans up lazily via vacuuming
index.discard(2);

// Replace a document with a new version under the same ID
index.replace({ id: 3, title: "Neuromancer (2nd ed.)", text: "..." });

// Vacuuming removes lingering references to discarded documents. It runs
// automatically by default, or manually:
index.vacuum();
```

### Serialization

```javascript
const serialized = index.toJsonString();

// Later:
const restored = FerroSearch.loadJson(serialized, {
  fields: ["title", "text"],
  storeFields: ["title", "category"],
});
```

`loadJson` must be given the same options used when the index was serialized.
The serialization format is MiniSearch's version-2 format; indexes transfer
between the two libraries in both directions.

### JSON fast paths

Bulk data crosses the native boundary as JSON strings — once, instead of
converting object graphs through the bindings. The package entry point
already routes `addAll` and `search` through these paths; they are also
available directly:

```javascript
index.addAllJson(jsonArrayOfDocuments); // e.g. a file or network payload
const results = JSON.parse(index.searchJson("zen art", { prefix: true }));
const serialized = index.toJsonString();
```

Each fast path is exactly equivalent to its object-based counterpart.
Documents, options, and queries must be JSON-serializable, which the native
boundary requires in any case.

### Result cache

Search results are memoized per `(query, options)` and invalidated on any
mutation, so repeated queries — autocomplete keystrokes, dashboard refreshes
— skip the engine. The cache is only consulted when no discarded documents
are pending vacuum, where search is a pure function of the index, so cached
results are observably identical to recomputed ones. Disable it with
`new FerroSearch({ ..., cache: false })`.

## MiniSearch compatibility

ferrosearch implements the behavior of minisearch 7.2.0. Given the same
documents, options, and queries, it returns the same results: scores, result
ordering (including ties), match data, serialized indexes, and error
messages. The test suite verifies this equivalence against the minisearch
package directly.

Function-valued options cannot cross the native boundary and are not
supported: `tokenize`, `processTerm`, `extractField`, `stringifyField`,
`filter`, `boostDocument`, `boostTerm`, `logger`, and the function forms of
`prefix` and `fuzzy`. The defaults are implemented natively: tokenization
splits on Unicode space and punctuation, terms are lowercased, and fields are
read as plain object keys with JavaScript string coercion.

Equivalent alternatives:

- Custom tokenization or stemming: pre-process documents into an indexed
  field before `add`, and pre-process queries before `search`.
- `filter`: filter the returned results; the original applies `filter` to
  fully assembled results, so this is equivalent.
- Nested field extraction: flatten the fields before indexing.

Additional differences:

- A wildcard node inside a `queries` combination tree is not supported; use
  the wildcard query at the top level.
- `vacuum()` is synchronous and complete; native code has no main thread to
  block. `addAllAsync` and `loadJSONAsync` are therefore not provided.
- Index-corruption warnings (removing a changed document) go to stderr.
- A partial `bm25` object replaces the whole parameter set, as in the
  original; the resulting NaN scores surface as `null` (as they would through
  `JSON.stringify`), because JSON has no NaN.

## Performance

The committed benchmarks run against minisearch 7.2.0 on the Billboard corpus
(5,086 documents, two indexed fields), measured on Bun 1.3, Apple Silicon,
with warmup. Reproduce with `bun run bench:compare` and
`bun run bench:memory`.

| Benchmark | Relative to minisearch |
| --- | --- |
| Serialize index (`toJsonString`) | 5x |
| Load serialized index | 2.2x |
| Auto suggestion | 1.6x |
| Indexing from a JSON string (`addAllJson`) | 1.3x |
| Indexing from JavaScript objects | 1.15x |
| Selective queries (few results) | 1.0x |
| Repeated queries (result cache) | 0.5x |
| Exact / prefix / fuzzy search (cold, mixed incl. broad queries) | 0.3–0.5x |

Memory: approximately 6.8 MB RSS per index versus approximately 22 MB for
the JavaScript implementation on the same corpus — about 3x smaller, through
flat-vector posting lists, interned stored-field names, and enum-keyed
document IDs.

Result transfer bounds bulk search performance: every result array crosses
the native boundary as JSON, and parsing a thousand-result payload costs more
than a warm JIT search in the JavaScript implementation. Bulk queries
returning large result sets therefore favor the JavaScript implementation,
while indexing, serialization, loading, auto-suggestion, selective queries,
and cold start favor ferrosearch.

## Implementation guarantees

- Insertion-ordered structures replicate JavaScript `Map` iteration order
  throughout, so tie ordering and match-data ordering are identical to
  MiniSearch's.
- Edit distances are measured in Unicode scalar values rather than UTF-16
  code units; behavior differs only for astral-plane characters.
- The incremental index cleanup that MiniSearch performs during search after
  `discard` is replicated, including its order-dependent scoring bookkeeping.
- The test suite (63 tests, 12,000+ assertions) compares every feature
  against the minisearch package: search batteries across corpora (unicode,
  mixed-type fields, edge-case IDs), full index-state equality after every
  lifecycle operation, error messages, and serialization round trips in both
  directions.

## Development

```bash
bun install
bun run verify          # cargo fmt --check, clippy, cargo test, release build, bun test
bun run bench:compare   # speed benchmark
bun run bench:memory    # memory benchmark
```

The [design document](docs/DESIGN.md) describes the internals: the slot-based
radix tree, the reused-matrix fuzzy search, interned-term scoring, and the
native-boundary strategy.

## License

MIT. Derived from MiniSearch by Luca Ongaro (MIT).
