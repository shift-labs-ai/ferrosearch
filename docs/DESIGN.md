# Design document

This document describes ferrosearch's design for developers contributing to
the project. ferrosearch implements the behavior of MiniSearch 7 in Rust,
and its architecture parallels the original implementation. The sections
below focus on what the native implementation adds or changes.

## Goals (and non-goals)

1. **Behavioral compatibility.** Given the same documents, options, and
   queries, ferrosearch returns what minisearch 7.2.0 returns: same scores,
   same ordering (including ties), same match data, same errors, same
   serialized index. Compatibility outranks speed: ferrosearch rejects any
   optimization that changes observable behavior.
2. **Native performance where the boundary allows it** — indexing from JSON,
   serialization, index loading, auto-suggest.
3. **Small API surface**, mirroring the original class plus a few additive
   JSON fast paths.

Non-goals:

- Function-valued options. Per-token or per-result JavaScript callbacks
  cannot cross the JSON boundary and would negate native performance; the
  README documents pre/post-processing alternatives instead.
- Browser targets. The project evaluated and rejected a wasm build:
  string-heavy workloads pay UTF-8/UTF-16 conversion at the wasm boundary
  on every call.
- Behavior changes. Where MiniSearch has surprising behavior —
  falsy-coalesced boosts, wholesale `bm25` replacement, order-dependent
  cleanup bookkeeping — ferrosearch replicates and documents it rather than
  altering it.

## Architecture

```text
src/
├── lib.rs      Node-API bindings (napi-rs); no logic
├── engine.rs   indexing, search, scoring, lifecycle, serialization
├── options.rs  option types + the overlay parsing that mirrors JS spreads
├── js.rs       JavaScript semantics: string coercion, ID identity, JSON writing
└── radix.rs    the radix tree (SearchableMap counterpart)
```

Documents, options, and queries cross the boundary as `serde_json::Value`.
Bulk paths (`addAllJson`, `searchJson`, `toJsonString`) cross as single JSON
strings, which costs far less than converting object graphs through the
bindings.

### The radix tree (`radix.rs`)

The inverted index is a compressed prefix tree, corresponding to
MiniSearch's `SearchableMap`. MiniSearch stores each node as a JavaScript
`Map` where the empty-string `LEAF` key holds the value and other keys hold
child edges, all in insertion order. Two different iteration orders of that
representation are observable through the API:

- `TreeIterator` (entries, prefix views) visits each node's keys in *reverse*
  insertion order — its `dive`/`backtrack` walk consumes the key array from
  the end.
- `fuzzySearch` visits keys in *forward* insertion order.

Result ordering, match-data ordering, and suggestion phrases all depend on
these orders, so the port must replicate both. Each node therefore keeps one
insertion-ordered `Vec` of slots:

```rust
enum Slot<T> {
    Leaf(T),
    Child(Box<str>, Node<T>),
}
```

Each node preserves the leaf's position among the edges rather than storing
the value in a separate field. Traversals iterate the slot vector backward
or forward to match the corresponding original. Edge splits and merges
append the new edge and remove the old one, replicating the `Map`
delete-and-set ordering of `createPath` and `merge`.

### Fuzzy search

The algorithm is MiniSearch's reused-matrix variation of Wagner–Fischer.
One Levenshtein matrix per query updates incrementally during a depth-first
traversal. The traversal computes only the diagonal band of `2 × maxDistance
+ 1` and prunes a subtree as soon as a row's minimum exceeds the maximum
distance. The trade-offs versus Levenshtein automata and trigram indexes
carry over unchanged.

Two deliberate differences:

- The engine measures distances in Unicode scalar values, not UTF-16 code
  units; behavior differs only for astral-plane characters.
- Where the JS version silently walks off the end of its `Uint8Array` for
  terms longer than `query + maxDistance` (yielding NaN distances that can
  never match), the port bounds-checks and prunes — same results, no
  undefined arithmetic.

### The engine (`engine.rs`)

The index maps `term → field → document → term frequency`, with fields and
documents referenced by short numeric IDs, exactly like the original. JS
`Map` iteration order is observable in scoring and tie ordering, so every
replacement structure must preserve insertion order. The document-keyed
engine maps are insertion-ordered `IndexMap`s with the Fx hasher. The
posting lists themselves are flat insertion-ordered pair vectors (`VecMap`).
Posting lists are typically tiny, so linear lookup beats hashing — lookups
scan from the back, where the most recently indexed document lives.
Dropping the per-`(term, field)` hash-table overhead cuts index memory by
about 20%. Two further compactions shrink the index. Stored fields are
`(interned name ID, value)` pairs sharing one name table instead of
per-document keyed maps. Document-ID identity is an enum keyed on f64 bits
(SameValueZero, like a JS `Map`) instead of formatted strings. Together the
index is roughly 3x smaller than the JS original's `Map`-based index.

Scoring is BM25+ with the original's constants and the same
order-of-operations, including its quirks:

- The scoring loop decrements `matching_fields` *mid-loop* as it encounters
  stale (discarded) document references, so documents later in the posting
  list score against a smaller count — order-dependent, and replicated.
- Search removes stale references afterward (collect-then-apply, since Rust
  cannot mutate the map under iteration), one frequency step at a time,
  matching the original's `removeTerm` semantics.

Search accumulation avoids per-document allocation. The engine interns query
and derived terms to `u16` IDs for the duration of one query, and
per-document bookkeeping stores integer pairs. Result assembly resolves the
strings at the end.

Result assembly has two equivalent forms: `Value` trees for the object API,
and a single-pass JSON string writer for `searchJson`. The writer emits
stored fields *after* the core keys, with duplicate keys permitted, and
`JSON.parse` keeps the last occurrence. This reproduces the original's
`Object.assign` override semantics: a stored field named `score` shadows
the computed score, as in MiniSearch.

### JavaScript semantics (`js.rs`)

One module concentrates everything that must behave like JavaScript:

- `js_to_string` — JS string coercion for field values and error messages
  (`[object Object]`, arrays joined with commas, integer-formatted floats).
- `id_key` — document-ID identity as an enum. JS `Map` keys distinguish `1`
  from `"1"` and compare numbers by value (SameValueZero); numeric IDs key
  on their f64 bits.
- JSON writing that matches `JSON.stringify`, including `NaN → null`.

### Options (`options.rs`)

MiniSearch merges options with shallow object spreads at several layers:
library defaults, constructor `searchOptions`, per-call options, and
per-query-tree nodes. `SearchOptions::overlay` reproduces one spread layer,
and callers chain it in the order the original spreads. Auto-suggest
resolves through the same chain from a different base (combine with `AND`,
prefix on the last term). This includes the original's behavior of
constructor `searchOptions` leaking into auto-suggest defaults.

## The native boundary

Crossing the native boundary dominates performance. Strategy:

- Bulk data crosses as JSON strings. Native `serde_json` parsing/writing
  beats per-object binding conversion — ~4.5x for serialization, ~1.2x for
  bulk indexing.
- Bulk search cannot be won this way: a warm JIT builds thousands of
  monomorphic result objects faster than any serialize-transfer-parse cycle.
  The result cache memoizes `(query, options) → JSON` and only serves reads
  while `dirt_count == 0`, where search is provably pure. Even a cache hit
  pays `JSON.parse`, which alone exceeds a warm JS search for
  thousand-result queries. Compatibility forbids paging or lazy results, so
  the README's performance table documents this loss.
- The package entry point (`ferrosearch.js`) routes `addAll` and `search`
  through the JSON paths and provides the `FerroSearch.wildcard` symbol
  query, keeping the native class free of JavaScript-only concerns.

## Testing strategy

The minisearch package defines correctness operationally as the reference
implementation. The Bun suite builds every scenario twice, reference and
ferrosearch. It asserts equivalent results (scores to 1e-9), error
messages, and serialized index state after every lifecycle operation.
Serialized indexes round-trip in both directions. Corpora include
unicode case folding, mixed-type fields, punctuation-only values, and
colliding ID types. Rust unit tests cover the radix tree's structural
operations directly.

The reference implementation pins behavior externally, so internals can
change freely without behavioral risk.
