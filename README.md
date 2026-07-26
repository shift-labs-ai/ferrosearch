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

The fast path returns results as a JSON string, which avoids materializing
result objects through the native bindings:

```js
const results = JSON.parse(miniSearch.searchJson("zen art motorcycle"));
```

## API differences from MiniSearch

- `wildcardSearch(options)` replaces the `MiniSearch.wildcard` symbol query.
- `vacuum()` is synchronous and complete; native code has no main thread to block.
- Function-valued options (`tokenize`, `processTerm`, `extractField`, `filter`,
  `boostDocument`, `boostTerm`, function forms of `prefix`/`fuzzy`) are not
  supported by the native engine. The default tokenizer, term processor, and
  field extractor are implemented in Rust.
- `addAllAsync` and `loadJSONAsync` are unnecessary: the synchronous native
  calls do not block the JavaScript thread the way pure-JS indexing does.

## Performance

Billboard corpus (5,086 documents, two indexed fields), Bun 1.3, Apple Silicon,
compared against minisearch 7.2.0:

| Operation | Speedup |
| --- | --- |
| Indexing | 1.5x |
| Search, no/few results | 1.8–4x |
| Search, ~400 results | 0.8x |
| Search, ~1,200 results | 0.7x |

Rust wins on indexing and selective queries. Very broad queries returning a
large fraction of the corpus are currently slower, because every result still
crosses the native boundary; this is the main open optimization.

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
