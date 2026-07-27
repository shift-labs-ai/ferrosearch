"use strict";

// Package entry point: a thin wrapper over the native class that routes bulk
// operations through the JSON fast paths (measurably faster than converting
// object graphs through the bindings) and restores the `MiniSearch.wildcard`
// symbol query of the original API. Documents and options must be
// JSON-serializable, which the native boundary requires in any case.

const native = require("./index.js");

const wildcard = Symbol("*");

class MiniSearch extends native.MiniSearch {
  /** The special wildcard query matching all documents. */
  static wildcard = wildcard;

  addAll(documents) {
    this.addAllJson(JSON.stringify(documents));
  }

  search(query, options) {
    if (query === wildcard) return this.wildcardSearch(options);
    return JSON.parse(this.searchJson(query, options));
  }

  static loadJson(json, options) {
    const instance = native.MiniSearch.loadJson(json, options);
    Object.setPrototypeOf(instance, MiniSearch.prototype);
    return instance;
  }

  /** Alias matching the original's method name. */
  static loadJSON(json, options) {
    return MiniSearch.loadJson(json, options);
  }
}

module.exports = { MiniSearch };
