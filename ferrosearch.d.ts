import { MiniSearch as NativeMiniSearch } from "./index";

export declare class MiniSearch extends NativeMiniSearch {
  /** The special wildcard query matching all documents. */
  static readonly wildcard: unique symbol;
  static loadJson(json: string, options: object): MiniSearch;
  /** Alias matching the original's method name. */
  static loadJSON(json: string, options: object): MiniSearch;
  search(query: unknown, options?: object): unknown[];
}
