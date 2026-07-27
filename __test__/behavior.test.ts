// Exhaustive behavior-pinning suite. The original JavaScript MiniSearch is
// the oracle: for a matrix of corpora, options, and operations, ferrosearch
// must produce equivalent output — same result order, same scores (to 1e-9),
// same match data, same errors, and the same serialized index.

import { describe, expect, test } from "bun:test";
import JsMiniSearch from "minisearch";
// @ts-expect-error resolved after napi build
import { MiniSearch } from "../ferrosearch.js";

// -- Deep equivalence with float tolerance -----------------------------------

function expectEquivalent(actual: unknown, expected: unknown, path = "$"): void {
  if (typeof expected === "number" && !Number.isFinite(expected)) {
    // Non-finite scores cannot exist in JSON: ferrosearch surfaces them as
    // null, exactly like `JSON.stringify(NaN)` in the original.
    expect(actual === null || (typeof actual === "number" && !Number.isFinite(actual)), path).toBe(true);
    return;
  }
  if (typeof expected === "number" && typeof actual === "number") {
    expect(actual, path).toBeCloseTo(expected, 9);
    return;
  }
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual)) throw new Error(`${path}: expected array`);
    expect(actual.length, `${path}.length`).toBe(expected.length);
    for (let i = 0; i < expected.length; i++) {
      expectEquivalent(actual[i], expected[i], `${path}[${i}]`);
    }
    return;
  }
  if (expected !== null && typeof expected === "object") {
    if (actual === null || typeof actual !== "object") throw new Error(`${path}: expected object`);
    const expectedKeys = Object.keys(expected as object).sort();
    const actualKeys = Object.keys(actual as object).sort();
    expect(actualKeys, `${path} keys`).toEqual(expectedKeys);
    for (const key of expectedKeys) {
      expectEquivalent(
        (actual as Record<string, unknown>)[key],
        (expected as Record<string, unknown>)[key],
        `${path}.${key}`,
      );
    }
    return;
  }
  expect(actual, path).toEqual(expected as any);
}

// -- Corpora -----------------------------------------------------------------

const books = [
  { id: 1, title: "Moby Dick", text: "Call me Ishmael. Some years ago I sailed the seas", category: "fiction", extra: null },
  { id: 2, title: "Zen and the Art of Motorcycle Maintenance", text: "I can see by my watch that it is time to ride the motorcycle", category: "fiction" },
  { id: 3, title: "Neuromancer", text: "The sky above the port was the color of television", category: "fiction" },
  { id: 4, title: "Zen and the Art of Archery", text: "At first sight it must seem that the art of archery is zen", category: "non-fiction" },
  { id: 5, title: "Gödel, Escher, Bach", text: "An eternal golden braid of art and logic", category: "non-fiction" },
  { id: 6, title: "The Art of War", text: "All warfare is based on deception and art", category: "non-fiction" },
  { id: 7, title: "Zen Mind, Beginner's Mind", text: "In the beginner's mind there are many possibilities", category: "non-fiction" },
  { id: 8, title: "Motorcycle Diaries", text: "A journey across South America on an old motorcycle", category: "biography" },
];

const oddDocuments = [
  { id: 0, title: "zero id", text: "the zero identifier" },
  { id: "0", title: "string zero id", text: "the string zero identifier" },
  { id: 1.5, title: "float id", text: "a fractional identifier" },
  { id: "emoji", title: "unicode 🚀 café naïve", text: "diacritics café naïveté coöperate" },
  { id: "numbers", title: 42 as any, text: 2.5 as any },
  { id: "boolean", title: true as any, text: "boolean true field" },
  { id: "array", title: ["short", "term"] as any, text: "array valued title" },
  { id: "object", title: { nested: 1 } as any, text: "object valued title" },
  { id: "empty", title: "", text: "empty title string" },
  { id: "punct", title: "!!! --- ...", text: "only punctuation title" },
  { id: "spaces", title: "a  b\u00a0c\nd\re", text: "unicode spaces and newlines" },
  { id: "casing", title: "İstanbul STRASSE ß", text: "unicode case folding" },
  { id: "repeat", title: "echo echo echo echo", text: "echo echo repeated terms" },
  { id: "missing" },
];

const options = { fields: ["title", "text"], storeFields: ["title", "category"] };

function buildBoth(docs: any[] = books, opts: any = options) {
  const js = new JsMiniSearch(opts);
  js.addAll(docs);
  const rust = new MiniSearch(opts);
  rust.addAll(docs);
  return { js, rust };
}

function expectSameSearch(js: JsMiniSearch<any>, rust: any, query: any, searchOptions?: any) {
  expectEquivalent(rust.search(query, searchOptions), js.search(query, searchOptions));
  // The JSON fast path must be byte-equivalent to the object path.
  expectEquivalent(JSON.parse(rust.searchJson(query, searchOptions)), js.search(query, searchOptions));
}

function expectSameIndex(js: JsMiniSearch<any>, rust: any) {
  const jsPlain = JSON.parse(JSON.stringify(js));
  const rustPlain = rust.toJson();
  const sortByTerm = (a: [string, unknown], b: [string, unknown]) => a[0].localeCompare(b[0]);
  jsPlain.index = jsPlain.index.toSorted(sortByTerm);
  rustPlain.index = rustPlain.index.toSorted(sortByTerm);
  expectEquivalent(rustPlain, jsPlain);
}

// A battery of queries exercising every matching mode.
const queryBattery: [any, any?][] = [
  ["zen art motorcycle"],
  ["art"],
  [""],
  ["   "],
  ["!!!"],
  ["nonexistent"],
  ["a"],
  ["zen art", { combineWith: "AND" }],
  ["zen art", { combineWith: "and" }],
  ["zen art", { combineWith: "AND_NOT" }],
  ["art zen", { combineWith: "And_not" as any }],
  ["moto", { prefix: true }],
  ["moto zen art", { prefix: true }],
  ["zen", { prefix: true }],
  ["ismael", { fuzzy: 0.2 }],
  ["ismael", { fuzzy: true }],
  ["ismael", { fuzzy: false }],
  ["ismael", { fuzzy: 1 }],
  ["ismael", { fuzzy: 2 }],
  ["nuromancer", { fuzzy: 2, maxFuzzy: 1 }],
  ["zzzzzzzzzzzz", { fuzzy: 6 }],
  ["moto zen", { prefix: true, fuzzy: 0.2 }],
  ["moto", { prefix: true, fuzzy: 0.2, weights: { fuzzy: 0.9, prefix: 0.1 } }],
  ["moto", { prefix: true, weights: { prefix: 1.5 } as any }],
  ["zen", { fields: ["title"] }],
  ["zen", { fields: ["title", "unknown"] as any }],
  ["zen art", { boost: { title: 2 } }],
  ["zen art", { boost: { title: 0 } }],
  ["zen art", { boost: { title: -1 } }],
  ["zen art", { boost: { unknown: 5 } as any }],
  ["art", { bm25: { k: 1.5, b: 0.5, d: 0.4 } }],
  ["art", { bm25: { k: 1.2 } as any }],
  ["the sea", {}],
  [{ queries: ["zen", "art"] }],
  [{ combineWith: "AND", queries: ["zen", { combineWith: "OR", queries: ["motorcycle", "archery"] }] }],
  [{ combineWith: "AND_NOT", queries: [{ combineWith: "OR", queries: ["art", "sea"] }, "zen", "war"] }],
  [{ combineWith: "AND", queries: [{ queries: ["zen"], prefix: true }, "art"] }, { fuzzy: 0.2 }],
  [{ queries: ["moto"], prefix: true }, { boost: { title: 3 } }],
  [{ queries: [] }],
];

describe("search battery equivalence", () => {
  test("books corpus", () => {
    const { js, rust } = buildBoth();
    for (const [query, searchOptions] of queryBattery) {
      expectSameSearch(js, rust, query, searchOptions);
    }
  });

  test("odd documents corpus", () => {
    const { js, rust } = buildBoth(oddDocuments, { fields: ["title", "text"], storeFields: ["title"] });
    const queries = ["the", "echo", "café", "istanbul", "i̇stanbul", "strasse", "short", "term", "object", "true", "42", "2", "5", "a", "d"];
    for (const query of queries) {
      expectSameSearch(js, rust, query);
      expectSameSearch(js, rust, query, { prefix: true, fuzzy: 0.2 });
    }
  });

  test("single-field corpus with ctor search options", () => {
    const opts = {
      fields: ["text"],
      searchOptions: { prefix: true, fuzzy: 0.3, boost: { text: 2 }, combineWith: "AND" },
    };
    const { js, rust } = buildBoth(books, opts);
    for (const query of ["zen art", "moto", "ismael", ""]) {
      expectSameSearch(js, rust, query);
      // Explicit options override the constructor defaults.
      expectSameSearch(js, rust, query, { prefix: false, fuzzy: false, combineWith: "OR" });
    }
  });
});

describe("index state equivalence", () => {
  test("books corpus serializes identically", () => {
    const { js, rust } = buildBoth();
    expectSameIndex(js, rust);
  });

  test("odd documents serialize identically", () => {
    const { js, rust } = buildBoth(oddDocuments, { fields: ["title", "text"], storeFields: ["title"] });
    expectSameIndex(js, rust);
  });

  test("counts match across corpora", () => {
    for (const docs of [books, oddDocuments]) {
      const { js, rust } = buildBoth(docs, { fields: ["title", "text"] });
      expect(rust.documentCount).toBe(js.documentCount);
      expect(rust.termCount).toBe(js.termCount);
    }
  });

  test("custom idField", () => {
    const docs = [
      { key: "a", title: "first document" },
      { key: "b", title: "second document" },
    ];
    const opts = { fields: ["title"], idField: "key" };
    const { js, rust } = buildBoth(docs, opts);
    expectSameSearch(js, rust, "document");
    expect(rust.has("a")).toBe(true);
    expect(rust.has("c")).toBe(false);
    expectSameIndex(js, rust);
  });

  test("stored fields can shadow result keys, like Object.assign", () => {
    const docs = [{ id: 1, title: "shadow", score: "not-a-number", terms: "stored-terms" }];
    const opts = { fields: ["title"], storeFields: ["score", "terms"] };
    const { js, rust } = buildBoth(docs, opts);
    const [jsResult] = js.search("shadow");
    const [rustResult] = rust.search("shadow");
    expect(jsResult.score).toBe("not-a-number" as any);
    expect(rustResult.score).toBe("not-a-number");
    expect(rustResult.terms).toBe("stored-terms");
    const [jsonResult] = JSON.parse(rust.searchJson("shadow"));
    expect(jsonResult.score).toBe("not-a-number");
  });
});

describe("error equivalence", () => {
  test("constructor requires fields", () => {
    expect(() => new (JsMiniSearch as any)({})).toThrow('MiniSearch: option "fields" must be provided');
    expect(() => new MiniSearch({})).toThrow('MiniSearch: option "fields" must be provided');
  });

  test("document identity errors", () => {
    const { rust } = buildBoth();
    expect(() => rust.add(books[0])).toThrow("MiniSearch: duplicate ID 1");
    expect(() => rust.add({ title: "no id" })).toThrow('MiniSearch: document does not have ID field "id"');
    expect(() => rust.remove({ id: 99, title: "ghost" })).toThrow(
      "MiniSearch: cannot remove document with ID 99: it is not in the index",
    );
    expect(() => rust.discard(99)).toThrow(
      "MiniSearch: cannot discard document with ID 99: it is not in the index",
    );
  });

  test("string and number IDs are distinct", () => {
    const { js, rust } = buildBoth(
      [
        { id: 1, title: "number one" },
        { id: "1", title: "string one" },
      ],
      { fields: ["title"] },
    );
    expect(rust.documentCount).toBe(js.documentCount);
    expect(rust.has(1)).toBe(true);
    expect(rust.has("1")).toBe(true);
    expectSameSearch(js, rust, "one");
  });

  test("invalid combination operator", () => {
    const { js, rust } = buildBoth();
    expect(() => js.search("zen art", { combineWith: "XOR" as any })).toThrow("Invalid combination operator: XOR");
    expect(() => rust.search("zen art", { combineWith: "XOR" })).toThrow("Invalid combination operator: XOR");
  });

  test("loadJson requires options and a compatible version", () => {
    const { js } = buildBoth();
    const serialized = JSON.stringify(js);
    expect(() => MiniSearch.loadJson(serialized, null)).toThrow(
      "MiniSearch: loadJSON should be given the same options used when serializing the index",
    );
    const wrongVersion = JSON.stringify({ ...JSON.parse(serialized), serializationVersion: 99 });
    expect(() => MiniSearch.loadJson(wrongVersion, options)).toThrow(
      "MiniSearch: cannot deserialize an index created with an incompatible version",
    );
  });
});

describe("lifecycle equivalence", () => {
  test("remove keeps both indexes equivalent", () => {
    const { js, rust } = buildBoth();
    for (const doc of [books[1], books[4], books[7]]) {
      js.remove(doc);
      rust.remove(doc);
    }
    expectSameIndex(js, rust);
    for (const [query, searchOptions] of queryBattery) {
      expectSameSearch(js, rust, query, searchOptions);
    }
  });

  test("removing every document empties the index", () => {
    const { js, rust } = buildBoth();
    for (const doc of books) {
      js.remove(doc);
      rust.remove(doc);
    }
    expect(rust.termCount).toBe(0);
    expect(js.termCount).toBe(0);
    expectSameIndex(js, rust);
  });

  test("removeAll() resets, removeAll(docs) removes", () => {
    const { js, rust } = buildBoth();
    js.removeAll([books[0], books[1]]);
    rust.removeAll([books[0], books[1]]);
    expectSameIndex(js, rust);
    js.removeAll();
    rust.removeAll();
    expect(rust.documentCount).toBe(0);
    expectSameIndex(js, rust);
    // Adding after a reset restarts short IDs identically.
    js.addAll(books.slice(0, 3));
    rust.addAll(books.slice(0, 3));
    expectSameIndex(js, rust);
  });

  test("discard hides documents and searches self-heal identically", () => {
    const { js, rust } = buildBoth();
    js.discard(2);
    rust.discard(2);
    js.discard(4);
    rust.discard(4);
    expect(rust.dirtCount).toBe(js.dirtCount);
    expect(rust.dirtFactor).toBeCloseTo(js.dirtFactor, 12);
    for (const [query, searchOptions] of queryBattery) {
      expectSameSearch(js, rust, query, searchOptions);
    }
  });

  test("discard then re-add the same ID", () => {
    const { js, rust } = buildBoth();
    js.discard(3);
    rust.discard(3);
    const updated = { ...books[2], title: "Neuromancer Second Edition" };
    js.add(updated);
    rust.add(updated);
    expectSameSearch(js, rust, "neuromancer");
    expectSameSearch(js, rust, "edition");
    expect(rust.has(3)).toBe(true);
  });

  test("replace is discard plus add", () => {
    const { js, rust } = buildBoth();
    const updated = { ...books[5], text: "war is over, art remains" };
    js.replace(updated);
    rust.replace(updated);
    expectSameSearch(js, rust, "war art remains");
    expect(() => rust.replace({ id: 99, title: "ghost" })).toThrow(
      "MiniSearch: cannot discard document with ID 99: it is not in the index",
    );
  });

  test("vacuum cleans all discarded references", async () => {
    const { js, rust } = buildBoth();
    for (const doc of books.slice(0, 4)) {
      js.discard(doc.id);
      rust.discard(doc.id);
    }
    await js.vacuum();
    rust.vacuum();
    expect(rust.dirtCount).toBe(0);
    expect(js.dirtCount).toBe(0);
    expectSameIndex(js, rust);
  });

  test("auto vacuum triggers at the default thresholds", async () => {
    const docs = Array.from({ length: 30 }, (_, i) => ({ id: i, title: `document number ${i}` }));
    const js = new JsMiniSearch({ fields: ["title"] });
    js.addAll(docs);
    const rust = new MiniSearch({ fields: ["title"] });
    rust.addAll(docs);
    for (let i = 0; i < 25; i++) {
      js.discard(i);
      rust.discard(i);
    }
    // The original vacuums asynchronously in batches; ferrosearch vacuums
    // synchronously. Both converge to the same state: the vacuum triggered
    // at the 20th discard resets the dirt counter, and the remaining five
    // discards stay below the threshold.
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(js.dirtCount).toBe(5);
    expect(rust.dirtCount).toBe(5);
    expectSameIndex(js, rust);
  });

  test("autoVacuum false never vacuums on discard", () => {
    const docs = Array.from({ length: 30 }, (_, i) => ({ id: i, title: `document number ${i}` }));
    const rust = new MiniSearch({ fields: ["title"], autoVacuum: false });
    rust.addAll(docs);
    for (let i = 0; i < 25; i++) rust.discard(i);
    expect(rust.dirtCount).toBe(25);
    rust.vacuum();
    expect(rust.dirtCount).toBe(0);
  });

  test("discardAll triggers at most one auto vacuum", () => {
    const docs = Array.from({ length: 30 }, (_, i) => ({ id: i, title: `document number ${i}` }));
    const rust = new MiniSearch({ fields: ["title"] });
    rust.addAll(docs);
    rust.discardAll(Array.from({ length: 25 }, (_, i) => i));
    expect(rust.dirtCount).toBe(0);
    expect(rust.documentCount).toBe(5);
  });

  test("getStoredFields parity", () => {
    const { js, rust } = buildBoth();
    expectEquivalent(rust.getStoredFields(4), js.getStoredFields(4));
    expect(rust.getStoredFields(99)).toBeNull();
    expect(js.getStoredFields(99)).toBeUndefined();
  });
});

describe("autoSuggest equivalence", () => {
  const suggestBattery: [string, any?][] = [
    ["zen ar"],
    ["zen"],
    ["neromancer", { fuzzy: 0.2 }],
    ["moto", {}],
    ["art of", {}],
    ["", {}],
    ["nonexistent", {}],
    ["zen ar", { combineWith: "OR" }],
    ["zen ar", { prefix: false }],
  ];

  test("suggestion battery", () => {
    const { js, rust } = buildBoth();
    for (const [query, suggestOptions] of suggestBattery) {
      expectEquivalent(rust.autoSuggest(query, suggestOptions), js.autoSuggest(query, suggestOptions));
    }
  });

  test("ctor autoSuggestOptions apply", () => {
    const opts = { ...options, autoSuggestOptions: { fuzzy: 0.3, combineWith: "OR" } };
    const { js, rust } = buildBoth(books, opts);
    for (const query of ["zen ar", "neromancer", "moto"]) {
      expectEquivalent(rust.autoSuggest(query), js.autoSuggest(query));
    }
  });

  test("ctor searchOptions leak into autoSuggest, like the original", () => {
    const opts = { ...options, searchOptions: { fuzzy: 0.4, boost: { title: 3 } } };
    const { js, rust } = buildBoth(books, opts);
    for (const query of ["zen ar", "neromancer"]) {
      expectEquivalent(rust.autoSuggest(query), js.autoSuggest(query));
    }
  });
});

describe("wildcard equivalence", () => {
  test("matches all documents with stored fields", () => {
    const { js, rust } = buildBoth();
    expectEquivalent(rust.wildcardSearch(), js.search(JsMiniSearch.wildcard));
    // The wrapper restores the wildcard symbol of the original API.
    expectEquivalent(rust.search(MiniSearch.wildcard), js.search(JsMiniSearch.wildcard));
  });

  test("empty index yields no results", () => {
    const rust = new MiniSearch({ fields: ["title"] });
    expect(rust.wildcardSearch()).toEqual([]);
  });

  test("wildcard after discard skips discarded documents", () => {
    const { js, rust } = buildBoth();
    js.discard(1);
    rust.discard(1);
    expectEquivalent(rust.wildcardSearch(), js.search(JsMiniSearch.wildcard));
  });
});

describe("result cache", () => {
  test("repeated searches return identical results without observable effects", () => {
    const { js, rust } = buildBoth();
    const first = rust.search("zen art", { prefix: true });
    const second = rust.search("zen art", { prefix: true });
    expectEquivalent(second, first);
    expectEquivalent(first, js.search("zen art", { prefix: true }));
    expect(rust.termCount).toBe(js.termCount);
  });

  test("mutations invalidate cached results", () => {
    const { js, rust } = buildBoth();
    expectEquivalent(rust.search("fresh"), js.search("fresh"));
    const added = { id: 9, title: "Fresh Document", text: "a fresh addition", category: "new" };
    js.add(added);
    rust.add(added);
    expectEquivalent(rust.search("fresh"), js.search("fresh"));
    js.remove(added);
    rust.remove(added);
    expectEquivalent(rust.search("fresh"), js.search("fresh"));
  });

  test("caching is bypassed while discarded documents are pending vacuum", () => {
    const { js, rust } = buildBoth();
    js.discard(2);
    rust.discard(2);
    // Repeated identical searches must replicate the original's incremental
    // self-healing, so they cannot be served from cache while dirty.
    for (let i = 0; i < 3; i++) {
      expectEquivalent(rust.search("motorcycle"), js.search("motorcycle"));
      expect(rust.termCount).toBe(js.termCount);
    }
  });

  test("cache can be disabled", () => {
    const { js } = buildBoth();
    const uncached = new MiniSearch({ ...options, cache: false });
    uncached.addAll(books);
    expectEquivalent(uncached.search("zen art"), js.search("zen art"));
    expectEquivalent(uncached.search("zen art"), js.search("zen art"));
  });
});

describe("serialization equivalence", () => {
  test("round trips preserve search behavior in both directions", () => {
    const { js, rust } = buildBoth(oddDocuments, { fields: ["title", "text"], storeFields: ["title"] });
    const fromJs = MiniSearch.loadJson(JSON.stringify(js), { fields: ["title", "text"], storeFields: ["title"] });
    const fromRust = JsMiniSearch.loadJSON(JSON.stringify(rust.toJson()), { fields: ["title", "text"], storeFields: ["title"] });
    for (const query of ["the", "echo", "café", "term", "true"]) {
      expectEquivalent(fromJs.search(query), js.search(query));
      expectEquivalent(fromRust.search(query), js.search(query));
    }
    expect(fromJs.documentCount).toBe(js.documentCount);
    expect(fromJs.termCount).toBe(js.termCount);
  });

  test("loaded index supports the full lifecycle", () => {
    const { js, rust } = buildBoth();
    const loaded = MiniSearch.loadJson(JSON.stringify(js), options);
    loaded.add({ id: 9, title: "Added After Load", text: "fresh document", category: "new" });
    js.add({ id: 9, title: "Added After Load", text: "fresh document", category: "new" });
    expectEquivalent(loaded.search("fresh"), js.search("fresh"));
    loaded.remove(books[0]);
    js.remove(books[0]);
    expectEquivalent(loaded.search("ishmael"), js.search("ishmael"));
    expectSameIndex(js, loaded);
    // The unrelated instance is untouched.
    expect(rust.documentCount).toBe(8);
  });

  test("addAllJson is equivalent to addAll", () => {
    const { js, rust } = buildBoth();
    const fromJson = new MiniSearch(options);
    fromJson.addAllJson(JSON.stringify(books));
    expectSameIndex(js, fromJson);
    expectSameSearch(js, fromJson, "zen art motorcycle", { prefix: true, fuzzy: 0.2 });
    expect(() => fromJson.addAllJson("not json")).toThrow("MiniSearch: invalid JSON documents");
    expect(rust.documentCount).toBe(fromJson.documentCount);
  });

  test("toJsonString equals JSON.stringify of toJson and loads everywhere", () => {
    const { js, rust } = buildBoth(oddDocuments, { fields: ["title", "text"], storeFields: ["title"] });
    expectEquivalent(JSON.parse(rust.toJsonString()), rust.toJson());
    const jsLoaded = JsMiniSearch.loadJSON(rust.toJsonString(), { fields: ["title", "text"], storeFields: ["title"] });
    expectEquivalent(jsLoaded.search("the"), js.search("the"));
    const rustLoaded = MiniSearch.loadJson(rust.toJsonString(), { fields: ["title", "text"], storeFields: ["title"] });
    expectEquivalent(rustLoaded.search("the"), js.search("the"));
  });

  test("version 1 index format loads", () => {
    const { js } = buildBoth();
    const plain = JSON.parse(JSON.stringify(js));
    const v1 = {
      ...plain,
      serializationVersion: 1,
      index: plain.index.map(([term, data]: [string, Record<string, unknown>]) => [
        term,
        Object.fromEntries(Object.entries(data).map(([fieldId, entry]) => [fieldId, { ds: entry }])),
      ]),
    };
    const rust = MiniSearch.loadJson(JSON.stringify(v1), options);
    const jsLoaded = JsMiniSearch.loadJSON(JSON.stringify(v1), options);
    expectEquivalent(rust.search("zen art motorcycle"), jsLoaded.search("zen art motorcycle"));
  });
});
