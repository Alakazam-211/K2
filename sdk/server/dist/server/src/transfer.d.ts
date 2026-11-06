/** The most one range carries. */
export declare const MAX_PART: number;
/** What the door holds of an upload. Ranges are `[first, last]`, inclusive. */
export interface DoorUploadStatus {
    uploadId: string;
    url: string;
    size: number;
    received: Array<[number, number]>;
    missing: Array<[number, number]>;
    receivedBytes: number;
    complete: boolean;
    done: boolean;
    /** Milliseconds since the epoch. */
    expiresAt: number;
    partMax: number;
    relPath?: string;
    sha256?: string;
}
/** A finished upload. */
export interface DoorUploadDone {
    uploadId: string;
    done: true;
    relPath: string;
    size: number;
    sha256: string;
}
/** A refusal from the door, with K2's error code. */
export declare class TransferError extends Error {
    readonly status: number;
    readonly code: string;
    readonly hint: string;
    readonly retryAfterSecs?: number | undefined;
    readonly name = "TransferError";
    constructor(status: number, code: string, hint: string, retryAfterSecs?: number | undefined);
    /** Worth sending the same request again (5xx, 408, 429; network errors are retried too). */
    get retryable(): boolean;
}
export interface UploadToDoorOptions {
    /** The App's own origin, e.g. `https://notes.box.k2.dev`. */
    base: string;
    /** The `url` files.uploadCreate answered. */
    url: string;
    /** The upload token (`transferToken`). */
    token: string;
    /** The whole file. */
    data: Blob | Uint8Array;
    /** Bytes per range (default 8 MiB; at most 16 MiB and the door's partMax). */
    partSize?: number;
    /** The file's sha256 (hex), when you have it; else it is computed here (whole file in memory). */
    sha256?: string;
    /** Tries per request, the first included (default 6). */
    attempts?: number;
    /** The first wait in ms (doubles each try, at most 30 s; default 500). */
    backoffMs?: number;
    onProgress?: (done: number, total: number) => void;
    fetch?: typeof fetch;
    signal?: AbortSignal;
}
/** The `[start, length]` pieces covering `missing`, each at most `part` bytes. */
export declare function missingParts(missing: ReadonlyArray<readonly [number, number]>, part: number): Array<[number, number]>;
/** The wait before try `attempt` (1-based): exponential, capped at 30 s, half of it jitter. */
export declare function backoffMs(attempt: number, baseMs?: number, retryAfterSecs?: number, random?: () => number): number;
/** Upload `data` through the door, resuming whatever it already holds. */
export declare function uploadToDoor(o: UploadToDoorOptions): Promise<DoorUploadDone>;
/**
 * A link a browser can open for a download token (`files.downloadToken`,
 * `thread.attachmentToken`): the token rides in the query, since a link
 * can't set a header. It is short-lived and opens that one file only; an
 * OS download task should send `Authorization: Bearer <token>` instead.
 */
export declare function downloadLink(base: string, url: string, token: string): string;
