// Resumable uploads through K2's transfer door (0.46.9, `k2 study app-transfers`).
//
// The App's server starts an upload as the person and hands the client its
// door path and token:
//
//   const up = await k2.asGuest(pass).files.uploadCreate({ workspace, dir: 'uploads', filename, size, sha256 })
//   // → { uploadId, url: '/_k2/transfers/uploads/<id>', transferToken: 'k2up_…', missing, partMax, … }
//
// Then any code with fetch (the web page, a Node worker) sends the bytes:
//
//   const done = await uploadToDoor({ base: 'https://notes.box.k2.dev', url: up.url, token: up.transferToken, data: file })
//
// It asks the door what it holds, PUTs only the missing ranges (each with
// its own sha256), retries network errors, 5xx and 429 with backoff, and
// completes with the file's sha256. Call it again after any failure: it
// resumes. Phones hand the same PUTs to the OS instead (iOS background
// URLSession, Android WorkManager); the Rust side is the
// k2-transfer-client crate.
/** The most one range carries. */
export const MAX_PART = 16 * 1024 * 1024;
/** A refusal from the door, with K2's error code. */
export class TransferError extends Error {
    status;
    code;
    hint;
    retryAfterSecs;
    name = 'TransferError';
    constructor(status, code, hint, retryAfterSecs) {
        super(`${code}: ${hint}`);
        this.status = status;
        this.code = code;
        this.hint = hint;
        this.retryAfterSecs = retryAfterSecs;
    }
    /** Worth sending the same request again (5xx, 408, 429; network errors are retried too). */
    get retryable() {
        return this.status >= 500 || this.status === 408 || this.status === 429;
    }
}
/** The `[start, length]` pieces covering `missing`, each at most `part` bytes. */
export function missingParts(missing, part) {
    const size = Math.max(1, Math.min(MAX_PART, Math.floor(part)));
    const out = [];
    for (const [first, last] of missing) {
        for (let at = first; at <= last; at += size)
            out.push([at, Math.min(size, last + 1 - at)]);
    }
    return out;
}
/** The wait before try `attempt` (1-based): exponential, capped at 30 s, half of it jitter. */
export function backoffMs(attempt, baseMs = 500, retryAfterSecs, random = Math.random) {
    const capped = Math.min(30_000, baseMs * 2 ** Math.max(0, attempt - 1));
    const wait = capped / 2 + random() * (capped / 2);
    return Math.max(wait, (retryAfterSecs ?? 0) * 1000);
}
const hex = (b) => [...new Uint8Array(b)].map((x) => x.toString(16).padStart(2, '0')).join('');
async function sha256Hex(bytes) {
    return hex(await crypto.subtle.digest('SHA-256', bytes));
}
async function slice(data, start, len) {
    if (data instanceof Uint8Array)
        return data.subarray(start, start + len);
    return new Uint8Array(await data.slice(start, start + len).arrayBuffer());
}
async function whole(data) {
    return data instanceof Uint8Array ? data : new Uint8Array(await data.arrayBuffer());
}
async function refusal(r) {
    const text = await r.text().catch(() => '');
    let code = 'failed';
    let hint = text.slice(0, 200);
    try {
        const v = JSON.parse(text);
        if (v.error?.code)
            code = v.error.code;
        if (typeof v.error?.hint === 'string')
            hint = v.error.hint;
    }
    catch {
        // Not JSON: the text is the hint.
    }
    const after = r.headers.get('retry-after');
    return new TransferError(r.status, code, hint, after && /^\d+$/.test(after) ? Number(after) : undefined);
}
/** One request with retries; `make` builds it fresh each try. */
async function send(o, make) {
    const f = o.fetch ?? fetch;
    const attempts = Math.max(1, o.attempts ?? 6);
    for (let attempt = 1;; attempt++) {
        let err;
        try {
            const [url, init] = make();
            const r = await f(url, { ...init, signal: o.signal });
            if (r.ok)
                return r;
            err = await refusal(r);
        }
        catch (e) {
            if (o.signal?.aborted)
                throw e;
            err = e;
        }
        const retryable = !(err instanceof TransferError) || err.retryable;
        if (!retryable || attempt >= attempts)
            throw err;
        const after = err instanceof TransferError ? err.retryAfterSecs : undefined;
        await new Promise((res) => setTimeout(res, backoffMs(attempt, o.backoffMs ?? 500, after)));
    }
}
/** Upload `data` through the door, resuming whatever it already holds. */
export async function uploadToDoor(o) {
    const target = `${o.base.replace(/\/+$/, '')}${o.url}`;
    const auth = { authorization: `Bearer ${o.token}` };
    const size = o.data instanceof Uint8Array ? o.data.byteLength : o.data.size;
    const sha = o.sha256 ?? (await sha256Hex(await whole(o.data)));
    let st = (await (await send(o, () => [target, { method: 'GET', headers: auth }])).json());
    if (st.size !== size)
        throw new TransferError(0, 'usage', `the data has ${size} bytes, the upload was made for ${st.size}`);
    if (!st.done) {
        o.onProgress?.(st.receivedBytes, st.size);
        const part = Math.min(o.partSize ?? 8 * 1024 * 1024, st.partMax || MAX_PART);
        for (const [start, len] of missingParts(st.missing, part)) {
            const bytes = await slice(o.data, start, len);
            const sum = await sha256Hex(bytes);
            const headers = {
                ...auth,
                'content-type': 'application/octet-stream',
                'content-range': `bytes ${start}-${start + len - 1}/${size}`,
                'k2-content-sha256': sum,
            };
            const r = await send(o, () => [target, { method: 'PUT', headers, body: bytes }]);
            st = (await r.json());
            o.onProgress?.(st.receivedBytes, st.size);
        }
    }
    const body = JSON.stringify({ sha256: sha });
    const r = await send(o, () => [`${target}/complete`, { method: 'POST', headers: { ...auth, 'content-type': 'application/json' }, body }]);
    return (await r.json());
}
/**
 * A link a browser can open for a download token (`files.downloadToken`,
 * `thread.attachmentToken`): the token rides in the query, since a link
 * can't set a header. It is short-lived and opens that one file only; an
 * OS download task should send `Authorization: Bearer <token>` instead.
 */
export function downloadLink(base, url, token) {
    return `${base.replace(/\/+$/, '')}${url}?token=${encodeURIComponent(token)}`;
}
