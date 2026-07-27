# ferrosearch

A Rust port of [MiniSearch](https://github.com/lucaong/minisearch), compiled as a
[napi-rs](https://napi.rs) native addon for Bun and Node.

ferrosearch replicates MiniSearch's behavior — BM25+ scoring, prefix and fuzzy
search, query combination, auto-suggestions, discard/vacuum lifecycle, and the
version-2 serialization format. Indexes serialized by one library load in the
other, and search results are numerically identical.

## Usage

```js
import { MiniSearch } from "ferrosearch";

const miniSearch = new MiniSearch({
  fields: ["title", "text"],
  storeFields: ["title", "category"],
});

miniSearch.addAll(documents);
const results = miniSearch.search("zen art motorcycle", { prefix: true, fuzzy: 0.2 });
```

JSON-string fast paths avoid materializing objects through the native
bindings, in both directions:

```js
const results = JSON.parse(miniSearch.searchJson("zen art motorcycle"));
miniSearch.addAllJson(jsonArrayOfDocuments);
const serialized = miniSearch.toJsonString();
```

## API differences from MiniSearch

- `wildcardSearch(options)` replaces the `MiniSearch.wildcard` symbol query.
- `vacuum()` is synchronous and complete; native code has no main thread to block.
- Function-valued options (`tokenize`, `processTerm`, `extractField`, `filter`,
  `boostDocument`, `boostTerm`, `logger`, function forms of `prefix`/`fuzzy`)
  are not supported by the native engine. The default tokenizer, term
  processor, and field extractor are implemented in Rust; index-corruption
  warnings go to stderr.
- A wildcard node inside a `queries` combination is not supported; use
  `wildcardSearch` at the top level.
- A partial `bm25` object replaces the whole parameter set, exactly like the
  original; the resulting NaN scores surface as `null` (as they would through
  `JSON.stringify`), because JSON has no NaN.
- `addAllAsync` and `loadJSONAsync` are unnecessary: the synchronous native
  calls do not block the JavaScript thread the way pure-JS indexing does.

## Performance

Run the committed benchmark with `bun run bench` (Billboard corpus, 5,086
documents, two indexed fields, compared against minisearch 7.2.0). Measured on
Bun 1.3, Apple Silicon, with warmup:

| Benchmark | Speedup vs JS |
| --- | --- |
| Serialize index (`toJsonString`) | 4.7x |
| Load serialized index | 2.1x |
| Auto suggestion | 1.6x |
| Indexing from a JSON string (`addAllJson`) | 1.2x |
| Queries with few or no results | 1.8–4x |
| Indexing from JavaScript objects | 0.9x |
| Exact / prefix / fuzzy search (mixed, incl. broad queries) | 0.3–0.5x |

Honest summary: once the JavaScript JIT is warm, the original wins most bulk
search scenarios, because every ferrosearch result still crosses the native
boundary as JSON. ferrosearch wins on cold start, selective queries,
auto-suggest, and index loading. Reducing result-transfer cost is the main
open optimization — within the constraint that results stay exactly
MiniSearch-shaped.

## Faithfulness notes

- Insertion-ordered maps replicate JavaScript `Map` iteration order, including
  the original `TreeIterator`'s reverse-insertion-order traversal and
  `fuzzySearch`'s forward traversal, so tie ordering and match order are
  identical.
- Edit distances are measured in Unicode scalar values rather than UTF-16 code
  units; behavior differs only for astral-plane characters.
- The stale-reference cleanup that MiniSearch performs during search (after
  `discard`) is replicated, including its order-dependent `matchingFields`
  bookkeeping.

## Development

```bash
bun install
bun run verify   # cargo fmt --check, clippy, cargo test, release build, bun test
```

The Bun test suite asserts parity against the original minisearch package on
every search mode, the document lifecycle, and serialization in both
directions.

## License

MIT. Derived from MiniSearch by Luca Ongaro (MIT).
