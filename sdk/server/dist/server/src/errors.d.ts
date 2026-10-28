import type { AppErrorCode } from './generated/verbs.ts';
export interface K2ErrorInit {
    code: AppErrorCode;
    hint: string;
    verb?: string;
    status?: number;
    cap?: string;
    room?: string;
    retryAfterSecs?: number;
    /** The refusal body as K2 sent it (parsed when it was JSON). */
    body?: unknown;
}
export declare class K2Error extends Error {
    readonly name = "K2Error";
    readonly code: AppErrorCode;
    readonly hint: string;
    readonly verb: string | undefined;
    /** The HTTP status (0 when the SDK refused before any request). */
    readonly status: number;
    readonly cap: string | undefined;
    readonly room: string | undefined;
    /** 423 locked and 429 rate_limited: when to try again. */
    readonly retryAfterSecs: number | undefined;
    readonly body: unknown;
    constructor(init: K2ErrorInit);
}
/** One K2Error from a non-2xx answer (`text` is the raw body). */
export declare function fromResponse(verb: string, status: number, text: string, retryAfterHeader: string | null): K2Error;
