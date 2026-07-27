"use strict";

// Package entry point. Extends the native class with the JSON fast paths for
// bulk operations and the wildcard symbol query. Documents, options, and
// queries must be JSON-serializable, which the native boundary requires in
// any case.

const native = require("./index.js");

const wildcard = Symbol("*");

class FerroSearch extends native.FerroSearch {
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
    const instance = native.FerroSearch.loadJson(json, options);
    Object.setPrototypeOf(instance, FerroSearch.prototype);
    return instance;
  }

  /** Alias for `loadJson`, for MiniSearch drop-in compatibility. */
  static loadJSON(json, options) {
    return FerroSearch.loadJson(json, options);
  }
}

module.exports = {
  FerroSearch,
  /** MiniSearch drop-in alias. */
  MiniSearch: FerroSearch,
};
