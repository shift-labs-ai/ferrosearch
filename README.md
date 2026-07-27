# ferrosearch

A Rust port of [MiniSearch](https://github.com/lucaong/minisearch), compiled as
a [napi-rs](https://napi.rs) native addon for Bun and Node.

ferrosearch replicates MiniSearch's behavior — BM25+ scoring, prefix and fuzzy
search, query combination, auto-suggestions, discard/vacuum lifecycle, and the
version-2 serialization format. Indexes serialized by one library load in the
other, and search results are numerically identical. An oracle test suite runs
every feature side by side with the original and asserts equivalence down to
scores, result order, match data, and error messages.

- [API reference](docs/API.md)
- [Design document](docs/DESIGN.md)

## Use case

Like the original, ferrosearch addresses use cases where full-text search
features are needed — prefix search, fuzzy search, ranking, field boosting —
and the indexed data fits in process memory. Being a native addon, it targets
server-side Bun and Node processes rather than browsers.

Choose ferrosearch over the JS original when your workload leans on its strong
sides: indexing from JSON payloads, serializing and loading indexes, and
auto-suggestions. See [Performance](#performance) for the honest numbers,
including where the JS original is faster.

## Features

- Exact match, prefix search, fuzzy match, field boosting, query combination
  trees (`AND` / `OR` / `AND_NOT`).
- Auto-suggestion engine for query auto-completion.
- BM25+ result ranking, identical to MiniSearch.
- Documents can be added, removed, discarded, and replaced at any time.
- Index serialization compatible with MiniSearch in both directions.
- JSON fast paths that keep bulk data out of the binding layer.

## Installation

The package builds from source and requires Rust and Bun (or Node):

```bash
bun install
bun run build
```

Then import the class:

```javascript
import { MiniSearch } from "ferrosearch";
```

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

const miniSearch = new MiniSearch({
  fields: ["title", "text"], // fields to index for full-text search
  storeFields: ["title", "category"], // fields to return with search results
});

// Index all documents
miniSearch.addAll(documents);

// Search with default options
const results = miniSearch.search("zen art motorcycle");
// => [
//   { id: 2, title: 'Zen and the Art of Motorcycle Maintenance', category: 'fiction', score: 2.77258, match: { ... } },
//   { id: 4, title: 'Zen and the Art of Archery', category: 'non-fiction', score: 1.38629, match: { ... } }
// ]
```

### Search options

```javascript
// Search only specific fields
miniSearch.search("zen", { fields: ["title"] });

// Boost some fields (here "title")
miniSearch.search("zen", { boost: { title: 2 } });

// Prefix search (so that 'moto' will match 'motorcycle')
miniSearch.search("moto", { prefix: true });

// Fuzzy search, here with a max edit distance of 0.2 * term length, rounded
// to the nearest integer. The misspelled 'ismael' will match 'ishmael'.
miniSearch.search("ismael", { fuzzy: 0.2 });

// Combine terms with AND instead of the default OR
miniSearch.search("zen art", { combineWith: "AND" });

// Default search options can be set upon initialization
const tuned = new MiniSearch({
  fields: ["title", "text"],
  searchOptions: { boost: { title: 2 }, fuzzy: 0.2 },
});
```

### Query combination trees

Subqueries can be combined with different operators and options, exactly like
the original's expression trees:

```javascript
// Documents that contain "zen" and ("motorcycle" or "archery")
miniSearch.search({
  combineWith: "AND",
  queries: [
    "zen",
    { combineWith: "OR", queries: ["motorcycle", "archery"] },
  ],
});
```

### Wildcard search

The original accepts a `MiniSearch.wildcard` symbol as the query; symbols
cannot cross the native boundary, so ferrosearch exposes a method instead:

```javascript
// Results for all documents, with stored fields
miniSearch.wildcardSearch();
```

### Auto suggestions

```javascript
miniSearch.autoSuggest("zen ar");
// => [ { suggestion: 'zen archery art', terms: [ 'zen', 'archery', 'art' ], score: 1.73332 },
//      { suggestion: 'zen art', terms: [ 'zen', 'art' ], score: 1.21313 } ]

// Fuzzy suggestions for misspelled input
miniSearch.autoSuggest("neromancer", { fuzzy: 0.2 });
// => [ { suggestion: 'neuromancer', terms: [ 'neuromancer' ], score: 1.03998 } ]
```

Suggestions are ranked by the relevance of the documents that the suggested
search would return.

### Removing, discarding, and replacing documents

```javascript
// Immediate removal: requires the full, unchanged document
miniSearch.remove(documents[0]);

// Discard by ID: faster, cleans up lazily via vacuuming
miniSearch.discard(2);

// Replace a document with a new version under the same ID
miniSearch.replace({ id: 3, title: "Neuromancer (2nd ed.)", text: "..." });

// Vacuuming removes lingering references to discarded documents. It runs
// automatically by default, or manually:
miniSearch.vacuum();
```

### Serialization

Indexes serialize to MiniSearch's format and load in either library:

```javascript
const serialized = miniSearch.toJsonString();

// Later, or in the JS original — both work:
const restored = MiniSearch.loadJson(serialized, { fields: ["title", "text"], storeFields: ["title", "category"] });
```

`loadJson` must be given the same options used when the index was serialized.

### JSON fast paths

Bulk data is fastest as JSON strings, which cross the native boundary once
instead of converting object graphs through the bindings:

```javascript
miniSearch.addAllJson(jsonArrayOfDocuments); // e.g. a file or network payload
const results = JSON.parse(miniSearch.searchJson("zen art", { prefix: true }));
const serialized = miniSearch.toJsonString();
```

Each fast path is exactly equivalent to its object-based counterpart.

## Differences from MiniSearch

Function-valued options cannot cross the native boundary and are not
supported: `tokenize`, `processTerm`, `extractField`, `stringifyField`,
`filter`, `boostDocument`, `boostTerm`, `logger`, and the function forms of
`prefix` and `fuzzy`. The defaults are implemented in Rust: tokenization
splits on Unicode space and punctuation, terms are lowercased, and fields are
read as plain object keys with JavaScript string coercion.

Common workarounds:

- Custom tokenization or stemming: pre-process documents into an indexed field
  before `add`, and pre-process queries before `search`.
- `filter`: filter the returned results — equivalent, since the original
  applies `filter` to fully assembled results.
- Nested field extraction: flatten the fields before indexing.

Other deltas:

- `wildcardSearch(options)` replaces the `MiniSearch.wildcard` symbol query;
  a wildcard node inside a `queries` combination is not supported.
- `vacuum()` is synchronous and complete; native code has no main thread to
  block. `addAllAsync` and `loadJSONAsync` are unnecessary for the same
  reason.
- Index-corruption warnings (removing a changed document) go to stderr.
- A partial `bm25` object replaces the whole parameter set, exactly like the
  original; the resulting NaN scores surface as `null` (as they would through
  `JSON.stringify`), because JSON has no NaN.

## Performance

Run the committed benchmark with `bun run bench:compare` (Billboard corpus, 5,086
documents, two indexed fields, compared against minisearch 7.2.0). Measured on
Bun 1.3, Apple Silicon, with warmup:

| Benchmark | Speedup vs JS |
| --- | --- |
| Serialize index (`toJsonString`) | 5x |
| Load serialized index | 2.3x |
| Auto suggestion | 1.5x |
| Indexing from a JSON string (`addAllJson`) | 1.2–1.3x |
| Selective queries (few results) | 1.0x |
| Indexing from JavaScript objects | 0.9x |
| Exact / prefix / fuzzy search (mixed, incl. broad queries) | 0.3–0.5x |

Memory (`bun run bench:memory`, RSS per index on the same corpus):
ferrosearch ≈ 6.8 MB versus ≈ 22 MB for the JS original — about 3x smaller,
thanks to flat-vector posting lists, interned stored-field names, and
enum-keyed document IDs instead of per-entry `Map` objects and strings.

Honest summary: once the JavaScript JIT is warm, the original wins most bulk
search scenarios, because every ferrosearch result still crosses the native
boundary as JSON; selective queries are at parity. ferrosearch wins on
serialization, index loading, auto-suggest, JSON-string indexing, and cold
start (native code needs no JIT warmup). Reducing result-transfer cost is the
main open optimization — within the constraint that results stay exactly
MiniSearch-shaped.

## Faithfulness

- Insertion-ordered maps replicate JavaScript `Map` iteration order, including
  the original `TreeIterator`'s reverse-insertion-order traversal and
  `fuzzySearch`'s forward traversal, so tie ordering and match order are
  identical.
- Edit distances are measured in Unicode scalar values rather than UTF-16 code
  units; behavior differs only for astral-plane characters.
- The stale-reference cleanup that MiniSearch performs during search (after
  `discard`) is replicated, including its order-dependent `matchingFields`
  bookkeeping.
- The oracle suite (59 tests, ~11,000 assertions) compares every feature
  against the installed minisearch package: search batteries across corpora
  (unicode, mixed-type fields, odd IDs), full index-state equality after every
  lifecycle operation, error messages, and serialization round trips in both
  directions.

## Development

```bash
bun install
bun run verify   # cargo fmt --check, clippy, cargo test, release build, bun test
bun run bench:compare   # speed benchmark against the JS original
bun run bench:memory    # memory benchmark against the JS original
```

The [design document](docs/DESIGN.md) explains the internals: the slot-based
radix tree, the reused-matrix fuzzy search, interned-term scoring, and the
native-boundary strategy.

## License

MIT. Derived from MiniSearch by Luca Ongaro (MIT).
