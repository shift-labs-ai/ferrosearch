import { FerroSearch as NativeFerroSearch } from "./index";

export declare class FerroSearch extends NativeFerroSearch {
  /** The special wildcard query matching all documents. */
  static readonly wildcard: unique symbol;
  static loadJson(json: string, options: object): FerroSearch;
  /** Alias for `loadJson`, for MiniSearch drop-in compatibility. */
  static loadJSON(json: string, options: object): FerroSearch;
  search(query: unknown, options?: object): unknown[];
}

/** MiniSearch drop-in alias. */
export declare const MiniSearch: typeof FerroSearch;
