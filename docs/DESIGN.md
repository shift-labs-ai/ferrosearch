# Design document

This document explains ferrosearch's design to developers contributing to the
project. It follows the structure of MiniSearch's [design
document](https://github.com/lucaong/minisearch/blob/master/DESIGN_DOCUMENT.md),
because the architecture deliberately parallels the original; the sections
below focus on what a Rust port adds or changes.

## Goals (and non-goals)

1. **Behavioral fidelity.** Given the same documents, options, and queries,
   ferrosearch returns what minisearch 7.2.0 returns: same scores, same
   ordering (including ties), same match data, same errors, same serialized
   index. Fidelity outranks speed: an optimization that changes observable
   behavior is rejected.
2. **Native performance where the boundary allows it** — indexing from JSON,
   serialization, index loading, auto-suggest, selective queries.
3. **Small API surface**, mirroring the original class plus a few additive
   JSON fast paths.

Non-goals:

- Function-valued options. JavaScript callbacks per token or per result would
  destroy native performance and cannot cross the JSON boundary; the README
  documents pre/post-processing workarounds instead.
- Browser targets. Use the original (or a wasm build, which was considered and
  rejected: string-heavy workloads pay UTF-8/UTF-16 conversion at the wasm
  boundary on every call).
- De-quirking. Where the original has surprising behavior — falsy-coalesced
  boosts, wholesale `bm25` replacement, order-dependent cleanup bookkeeping —
  ferrosearch replicates it and documents it, rather than "fixing" it.

## Architecture

```text
src/
├── lib.rs      Node-API bindings (napi-rs); no logic
├── engine.rs   indexing, search, scoring, lifecycle, serialization
├── options.rs  option types + the overlay parsing that mirrors JS spreads
├── js.rs       JavaScript semantics: string coercion, ID identity, JSON writing
└── radix.rs    the radix tree (SearchableMap counterpart)
```

Documents, options, and queries cross the boundary as `serde_json::Value`;
bulk paths (`addAllJson`, `searchJson`, `toJsonString`) cross as single JSON
strings, which is significantly cheaper than converting object graphs through
the bindings.

### The radix tree (`radix.rs`)

Like the original's `SearchableMap`, the inverted index is a compressed prefix
tree. The original stores each node as a JavaScript `Map` where the empty
string `LEAF` key holds the value and other keys hold child edges, all in
insertion order. That representation has a property that matters more than it
looks: **two different iteration orders are observable through the API.**

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

The leaf's position among the edges is preserved (it is not a separate field),
and traversals iterate the slot vector backward or forward to match the
corresponding original. Edge splits and merges append the new edge and remove
the old one, replicating the `Map` delete-and-set ordering of `createPath` and
`merge`.

### Fuzzy search

The algorithm is the original's reused-matrix variation of Wagner–Fischer:
one Levenshtein matrix is allocated per query and updated incrementally during
a depth-first traversal, computing only the diagonal band of `2 × maxDistance
+ 1` and pruning a subtree as soon as a row's minimum exceeds the maximum
distance. See the original design document for the full rationale; the
trade-offs (versus Levenshtein automata and trigram indexes) carry over
unchanged.

Two deliberate differences:

- Distances are measured in Unicode scalar values, not UTF-16 code units;
  behavior differs only for astral-plane characters.
- Where the JS version silently walks off the end of its `Uint8Array` for
  terms longer than `query + maxDistance` (yielding NaN distances that can
  never match), the port bounds-checks and prunes — same results, no
  undefined arithmetic.

### The engine (`engine.rs`)

The index maps `term → field → document → term frequency`, with fields and
documents referenced by short numeric IDs, exactly like the original. All
integer-keyed maps are insertion-ordered `IndexMap`s with the Fx hasher: JS
`Map` iteration order is observable in scoring and tie ordering, so ordinary
hash maps would silently diverge.

Scoring is BM25+ with the original's constants and the same
order-of-operations, including its quirks:

- `matching_fields` is decremented *mid-loop* as stale (discarded) document
  references are encountered, so documents later in the posting list score
  against a smaller count — order-dependent, and replicated.
- Stale references found during search are removed afterward
  (collect-then-apply, since Rust cannot mutate the map being iterated), one
  frequency step at a time, matching the original's `removeTerm` semantics.

Search accumulation avoids per-document allocation: query and derived terms
are interned to `u16` IDs for the duration of one query, and per-document
bookkeeping stores integer pairs. Strings are resolved only during final
result assembly.

Result assembly has two equivalent forms: `Value` trees for the object API,
and a single-pass JSON string writer for `searchJson`. The writer emits
stored fields *after* the core keys with duplicate keys permitted —
`JSON.parse` keeps the last occurrence, which reproduces the original's
`Object.assign` override semantics (a stored field named `score` really does
shadow the score, there as here).

### JavaScript semantics (`js.rs`)

Everything that must behave like JavaScript is concentrated in one module:

- `js_to_string` — JS string coercion for field values and error messages
  (`[object Object]`, arrays joined with commas, integer-formatted floats).
- `id_key` — document-ID identity. JS `Map` keys distinguish `1` from `"1"`;
  IDs are namespaced by type into string keys.
- JSON writing that matches `JSON.stringify`, including `NaN → null`.

### Options (`options.rs`)

MiniSearch merges options with shallow object spreads at several layers
(library defaults → constructor `searchOptions` → per-call options →
per-query-tree node). `SearchOptions::overlay` reproduces one spread layer;
callers chain it in the same order the original spreads. Auto-suggest is a
different *base* (combine with `AND`, prefix on the last term) resolved
through the same chain — including the original's behavior of constructor
`searchOptions` leaking into auto-suggest defaults.

## The native boundary

The main performance lesson of this port: **crossing the boundary dominates
everything else.** Strategy:

- Bulk data crosses as JSON strings. Native `serde_json` parsing/writing plus
  the engine's `JSON.parse` on the JS side beats per-object binding
  conversion by 2–5x.
- Bulk search cannot be won this way: a warm JIT builds thousands of
  monomorphic result objects faster than any serialize-transfer-parse cycle.
  Fidelity forbids paging or lazy results, so this loss is accepted and
  documented in the README's performance table.

## Testing strategy

Correctness is defined operationally: *the installed minisearch package is
the oracle.* The Bun suite builds every scenario twice — original and port —
and asserts equivalence of results (scores to 1e-9), full serialized index
state after every lifecycle operation, error messages, and round-trips of
serialized indexes in both directions. Corpora include unicode case folding,
mixed-type fields, punctuation-only values, and colliding ID types. Rust unit
tests cover the radix tree's structural operations directly.

This is what makes refactoring safe: the port's internals may be reshaped
freely (and have been), because behavior is pinned externally against the
implementation being ported.
