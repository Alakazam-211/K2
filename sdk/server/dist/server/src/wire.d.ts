import type { AppVerbRow, Schema } from './generated/verbs.ts';
type Http = NonNullable<AppVerbRow['http']>;
/** One query value as K2 reads it (arrays comma-joined, booleans true/false). */
export declare function queryValue(v: unknown): string | null;
export interface WireRequest {
    method: 'GET' | 'POST';
    /** Path and query, e.g. `/cli/thread?addr=sales`. */
    pathAndQuery: string;
    /** The JSON body (POST only). */
    body: string | null;
}
/** The request for `args` on an http row. */
export declare function buildRequest(http: Http, args: Record<string, unknown> | undefined): WireRequest;
/**
 * `value` read through `schema`: object properties under their wire names
 * (`names`: catalog → wire, at any depth), unnamed fields dropped, arrays
 * item by item, maps and `json` values as they are.
 */
export declare function project(schema: Schema, value: unknown, names: Readonly<Record<string, string>>): unknown;
export {};
