import { describe, expect, test } from "bun:test";
import JsMiniSearch from "minisearch";
// @ts-expect-error resolved after napi build
import { FerroSearch } from "../ferrosearch.js";

/**
 * The observable contract of `loadJson`: error messages and their
 * precedence, tolerances for weird-but-valid JSON, the legacy version-1
 * format, and round trips on a long-field corpus (the knowledge-base
 * document shape that motivated the streaming loader). Both load paths —
 * the streaming version-2 loader and the value-tree version-1 fallback —
 * must satisfy this suite; it was captured against the value-tree loader
 * before the streaming rewrite, so green here means the two are equivalent.
 */

const options = {
  fields: ["title", "text"],
  storeFields: ["file"],
};

// ─── deterministic long-field corpus ────────────────────────────────

function mulberry32(seed: number): () => number {
  let a = seed;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const WORDS =
  `alpha beta gamma delta epsilon zeta eta theta iota kappa lambda sigma
   vector search index cache retrieval embedding cluster token payroll
   telescope fermentation guarantee lease kubernetes ingress outbox relay
   naïve café über résumé 東京 postgres braintree adyen ledger vacuum`.split(
    /\s+/,
  );

function longFieldDocs(count: number, wordsPerDoc: number) {
  const rand = mulberry32(0xf0ad);
  const docs = [];
  for (let id = 0; id < count; id++) {
    const words: string[] = [];
    for (let w = 0; w < wordsPerDoc; w++) {
      words.push(WORDS[Math.floor(rand() * WORDS.length)]);
    }
    docs.push({
      id,
      title: `note-${id} ${words[0]} ${words[1]}`,
      text: words.join(" "),
      file: `folder-${id % 20}/note-${id}.md`,
    });
  }
  return docs;
}

const sortByTerm = (a: [string, unknown], b: [string, unknown]) =>
  a[0].localeCompare(b[0]);

function expectSameState(actual: any, reference: any) {
  const a = JSON.parse(JSON.stringify(actual.toJson()));
  const r = JSON.parse(JSON.stringify(reference));
  expect(a.documentCount).toBe(r.documentCount);
  expect(a.nextId).toBe(r.nextId);
  expect(a.documentIds).toEqual(r.documentIds);
  expect(a.fieldIds).toEqual(r.fieldIds);
  expect(a.fieldLength).toEqual(r.fieldLength);
  expect(a.averageFieldLength).toEqual(r.averageFieldLength);
  expect(a.storedFields).toEqual(r.storedFields);
  expect(a.dirtCount).toBe(r.dirtCount);
  expect(a.index.toSorted(sortByTerm)).toEqual(r.index.toSorted(sortByTerm));
}

describe("loadJson round trips on a long-field corpus", () => {
  const docs = longFieldDocs(200, 150);

  test("ferrosearch → ferrosearch, full state equality", () => {
    const original = new FerroSearch(options);
    original.addAllJson(JSON.stringify(docs));
    const loaded = FerroSearch.loadJson(original.toJsonString(), options);
    expectSameState(loaded, original.toJson());
    expect(loaded.documentCount).toBe(original.documentCount);
    expect(loaded.termCount).toBe(original.termCount);
  });

  test("minisearch → ferrosearch, identical search results and scores", () => {
    const js = new JsMiniSearch(options);
    js.addAll(docs);
    const loaded = FerroSearch.loadJson(JSON.stringify(js), options);
    for (const query of ["vector cache", "telescope", "naïve 東京", "zzz"]) {
      const want = js.search(query, { prefix: true, fuzzy: 0.2 });
      const got = loaded.search(query, { prefix: true, fuzzy: 0.2 });
      expect(got.length).toBe(want.length);
      for (let i = 0; i < want.length; i++) {
        expect(got[i].id).toBe(want[i].id);
        expect(got[i].score).toBeCloseTo(want[i].score, 10);
        expect(got[i].file).toBe(want[i].file);
      }
    }
  });

  test("ferrosearch → minisearch, identical search results and scores", () => {
    const original = new FerroSearch(options);
    original.addAllJson(JSON.stringify(docs));
    const js = JsMiniSearch.loadJSON(original.toJsonString(), options);
    const want = original.search("guarantee lease", { prefix: true });
    const got = js.search("guarantee lease", { prefix: true });
    expect(got.length).toBe(want.length);
    for (let i = 0; i < want.length; i++) {
      expect(got[i].id).toBe(want[i].id);
      expect(got[i].score).toBeCloseTo(want[i].score, 10);
    }
  });
});

// ─── error messages and precedence ──────────────────────────────────

function minimalIndex(overrides: Record<string, unknown> = {}): string {
  return JSON.stringify({
    documentCount: 0,
    nextId: 0,
    documentIds: {},
    fieldIds: { title: 0, text: 1 },
    fieldLength: {},
    averageFieldLength: [],
    storedFields: {},
    dirtCount: 0,
    index: [],
    serializationVersion: 2,
    ...overrides,
  });
}

describe("loadJson error contract", () => {
  test("null options", () => {
    expect(() => FerroSearch.loadJson(minimalIndex(), null)).toThrow(
      "MiniSearch: loadJSON should be given the same options used when serializing the index",
    );
  });

  test("invalid JSON syntax", () => {
    expect(() => FerroSearch.loadJson("{not json", options)).toThrow(
      /^MiniSearch: invalid JSON index: /,
    );
    expect(() => FerroSearch.loadJson("", options)).toThrow(
      /^MiniSearch: invalid JSON index: /,
    );
  });

  test("valid JSON that is not an object → version error", () => {
    for (const text of ["[1,2,3]", "5", '"hello"', "null", "true"]) {
      expect(() => FerroSearch.loadJson(text, options)).toThrow(
        "MiniSearch: cannot deserialize an index created with an incompatible version",
      );
    }
  });

  test("missing or wrong serializationVersion", () => {
    expect(() => FerroSearch.loadJson("{}", options)).toThrow(
      "MiniSearch: cannot deserialize an index created with an incompatible version",
    );
    expect(() =>
      FerroSearch.loadJson(minimalIndex({ serializationVersion: 3 }), options),
    ).toThrow(
      "MiniSearch: cannot deserialize an index created with an incompatible version",
    );
    expect(() =>
      FerroSearch.loadJson(
        minimalIndex({ serializationVersion: "2" }),
        options,
      ),
    ).toThrow(
      "MiniSearch: cannot deserialize an index created with an incompatible version",
    );
  });

  test("version error precedes option validation errors", () => {
    // Both the version and the options are invalid: version wins.
    expect(() =>
      FerroSearch.loadJson(minimalIndex({ serializationVersion: 9 }), {}),
    ).toThrow(
      "MiniSearch: cannot deserialize an index created with an incompatible version",
    );
    // Version is fine: option validation runs.
    expect(() => FerroSearch.loadJson(minimalIndex(), {})).toThrow(
      'MiniSearch: option "fields" must be provided',
    );
  });

  test("invalid document ID in documentIds", () => {
    expect(() =>
      FerroSearch.loadJson(
        minimalIndex({ documentIds: { "not-a-number": 7 } }),
        options,
      ),
    ).toThrow("MiniSearch: invalid document ID in serialized index");
  });

  test("invalid index entries", () => {
    const cases = [
      { index: [5] },
      { index: ["nope"] },
      { index: [[]] },
      { index: [["term"]] },
      { index: [["term", 5]] },
      { index: [[7, { "0": { "0": 1 } }]] },
    ];
    for (const overrides of cases) {
      expect(() =>
        FerroSearch.loadJson(minimalIndex(overrides), options),
      ).toThrow("MiniSearch: invalid index entry in serialized index");
    }
  });

  test("invalid field ID in index entry", () => {
    expect(() =>
      FerroSearch.loadJson(
        minimalIndex({ index: [["term", { abc: { "0": 1 } }]] }),
        options,
      ),
    ).toThrow("MiniSearch: invalid field ID in serialized index");
  });

  test("invalid field ID value in fieldIds", () => {
    expect(() =>
      FerroSearch.loadJson(minimalIndex({ fieldIds: { title: "x" } }), options),
    ).toThrow("MiniSearch: invalid field ID in serialized index");
  });

  test("invalid document ID in posting list", () => {
    expect(() =>
      FerroSearch.loadJson(
        minimalIndex({ index: [["term", { "0": { abc: 1 } }]] }),
        options,
      ),
    ).toThrow("MiniSearch: invalid document ID in serialized index");
  });
});

// ─── tolerance for weird-but-valid inputs ───────────────────────────

describe("loadJson tolerance contract", () => {
  test("non-integer term frequencies load as 0", () => {
    const loaded = FerroSearch.loadJson(
      minimalIndex({
        documentCount: 4,
        nextId: 4,
        documentIds: { "0": 10, "1": 11, "2": 12, "3": 13 },
        index: [
          ["hello", { "0": { "0": 2.5, "1": "9", "2": null, "3": 7 } }],
        ],
      }),
      options,
    );
    const entry = loaded
      .toJson()
      .index.find(([term]: [string]) => term === "hello");
    expect(entry[1]).toEqual({ "0": { "0": 0, "1": 0, "2": 0, "3": 7 } });
  });

  test("wrong-typed scalar sections default to 0", () => {
    const loaded = FerroSearch.loadJson(
      minimalIndex({ documentCount: "many", nextId: 3.5, dirtCount: null }),
      options,
    );
    const plain = loaded.toJson();
    expect(plain.documentCount).toBe(0);
    expect(plain.nextId).toBe(0);
    expect(plain.dirtCount).toBe(0);
  });

  test("non-array index section is skipped silently", () => {
    for (const index of [{}, "nope", 5, null]) {
      const loaded = FerroSearch.loadJson(minimalIndex({ index }), options);
      expect(loaded.termCount).toBe(0);
    }
  });

  test("non-object sections are skipped silently", () => {
    const loaded = FerroSearch.loadJson(
      minimalIndex({
        documentIds: [1, 2],
        fieldLength: "x",
        storedFields: 9,
        averageFieldLength: { not: "an array" },
      }),
      options,
    );
    const plain = loaded.toJson();
    expect(plain.documentIds).toEqual({});
    expect(plain.fieldLength).toEqual({});
    expect(plain.storedFields).toEqual({});
    expect(plain.averageFieldLength).toEqual([]);
  });

  test("non-array fieldLength values load as empty", () => {
    const loaded = FerroSearch.loadJson(
      minimalIndex({ fieldLength: { "0": "x", "1": [3, null, "y"] } }),
      options,
    );
    expect(loaded.toJson().fieldLength).toEqual({
      "0": [],
      "1": [3, null, null],
    });
  });

  test("non-object field posting values skip the field, keep the term", () => {
    const loaded = FerroSearch.loadJson(
      minimalIndex({ index: [["hello", { "0": 5, "1": [1, 2] }]] }),
      options,
    );
    expect(loaded.termCount).toBe(1);
    const entry = loaded
      .toJson()
      .index.find(([term]: [string]) => term === "hello");
    expect(entry[1]).toEqual({});
  });

  test("extra elements in an index entry are ignored", () => {
    const loaded = FerroSearch.loadJson(
      minimalIndex({
        documentCount: 1,
        nextId: 1,
        documentIds: { "0": 42 },
        index: [["hello", { "0": { "0": 1 } }, "extra", { junk: true }]],
      }),
      options,
    );
    expect(loaded.termCount).toBe(1);
    expect(loaded.search("hello")[0].id).toBe(42);
  });

  test("duplicate index terms and field keys: last occurrence wins", () => {
    const loaded = FerroSearch.loadJson(
      minimalIndex({
        documentCount: 1,
        nextId: 1,
        documentIds: { "0": 1 },
        index: [
          ["hello", { "0": { "0": 1 } }],
          ["hello", { "0": { "0": 5 } }],
        ],
      }),
      options,
    );
    expect(loaded.termCount).toBe(1);
    const entry = loaded
      .toJson()
      .index.find(([term]: [string]) => term === "hello");
    expect(entry[1]).toEqual({ "0": { "0": 5 } });
  });

  test("duplicate document keys in a posting object: last wins, first position", () => {
    // Raw JSON with duplicate keys inside one object; JSON.parse and the
    // value-tree loader keep the last value at the first key's position.
    const json =
      '{"documentCount":2,"nextId":2,"documentIds":{"0":10,"1":11},' +
      '"fieldIds":{"title":0,"text":1},"fieldLength":{},"averageFieldLength":[],' +
      '"storedFields":{},"dirtCount":0,' +
      '"index":[["hello",{"0":{"0":1,"1":2,"0":9}}]],"serializationVersion":2}';
    const loaded = FerroSearch.loadJson(json, options);
    const entry = loaded
      .toJson()
      .index.find(([term]: [string]) => term === "hello");
    expect(entry[1]).toEqual({ "0": { "0": 9, "1": 2 } });
    expect(JSON.stringify(entry[1]["0"])).toBe('{"0":9,"1":2}');
  });

  test("unknown top-level keys are ignored", () => {
    const loaded = FerroSearch.loadJson(
      minimalIndex({ futureField: { anything: [1, 2, 3] } }),
      options,
    );
    expect(loaded.documentCount).toBe(0);
  });

  test("stored field names outside storeFields are preserved", () => {
    const loaded = FerroSearch.loadJson(
      minimalIndex({
        documentCount: 1,
        nextId: 1,
        documentIds: { "0": 1 },
        storedFields: { "0": { file: "a.md", extra: "kept" } },
      }),
      options,
    );
    expect(loaded.getStoredFields(1)).toEqual({ file: "a.md", extra: "kept" });
  });
});

// ─── legacy version-1 format ────────────────────────────────────────

describe("loadJson version 1", () => {
  test("postings nested under 'ds' load correctly", () => {
    const loaded = FerroSearch.loadJson(
      minimalIndex({
        serializationVersion: 1,
        documentCount: 1,
        nextId: 1,
        documentIds: { "0": 7 },
        fieldLength: { "0": [1, 1] },
        averageFieldLength: [1, 1],
        index: [["hello", { "0": { ds: { "0": 1 } } }]],
      }),
      options,
    );
    expect(loaded.termCount).toBe(1);
    expect(loaded.search("hello")[0].id).toBe(7);
  });

  test("v1 entries without 'ds' treat the object as postings", () => {
    const loaded = FerroSearch.loadJson(
      minimalIndex({
        serializationVersion: 1,
        documentCount: 1,
        nextId: 1,
        documentIds: { "0": 7 },
        fieldLength: { "0": [1, 1] },
        averageFieldLength: [1, 1],
        index: [["hello", { "0": { "0": 1 } }]],
      }),
      options,
    );
    expect(loaded.search("hello")[0].id).toBe(7);
  });
});
