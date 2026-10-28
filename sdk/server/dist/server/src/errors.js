// K2Error: every refusal the server SDK surfaces, with one stable `code`
// from the verb catalog's shared error list (runtime S23). The daemon's
// routes answer in a few shapes ({error:{code,hint}}, {ok:false,error},
// {error:"text"}); `fromResponse` folds them into one.
export class K2Error extends Error {
    name = 'K2Error';
    code;
    hint;
    verb;
    /** The HTTP status (0 when the SDK refused before any request). */
    status;
    cap;
    room;
    /** 423 locked and 429 rate_limited: when to try again. */
    retryAfterSecs;
    body;
    constructor(init) {
        super(`${init.code}: ${init.hint}`);
        this.code = init.code;
        this.hint = init.hint;
        this.verb = init.verb;
        this.status = init.status ?? 0;
        this.cap = init.cap;
        this.room = init.room;
        this.retryAfterSecs = init.retryAfterSecs;
        this.body = init.body;
    }
}
/** Plain-text refusals K2 sends as `{"error": "<text>"}`, mapped to codes. */
const TEXT_CODES = [
    [/^invalid or revoked skin token$/, 'pass_invalid'],
    [/^invalid username or password$/, 'bad_login'],
    [/^invalid password$/, 'bad_login'],
    [/^locked$/, 'locked'],
    [/^rate_limited$/, 'rate_limited'],
    [/^too_large$/, 'too_large'],
    [/^no_such_heartbeat$/, 'not_found'],
    [/^no live pinned chat$/, 'not_found'],
    [/^offset_mismatch$|^upload_busy$|^in_flight$|^backoff$|^disabled$|^handle_taken$|^handle_retired$/, 'conflict'],
];
function byStatus(status) {
    if (status === 400 || status === 405)
        return 'usage';
    if (status === 401)
        return 'pass_invalid';
    if (status === 403)
        return 'cap_not_granted';
    if (status === 404)
        return 'not_found';
    if (status === 409)
        return 'conflict';
    if (status === 413)
        return 'too_large';
    if (status === 415)
        return 'json_required';
    if (status === 423)
        return 'locked';
    if (status === 429)
        return 'rate_limited';
    return 'failed';
}
function isRecord(v) {
    return typeof v === 'object' && v !== null && !Array.isArray(v);
}
/** One K2Error from a non-2xx answer (`text` is the raw body). */
export function fromResponse(verb, status, text, retryAfterHeader) {
    let body = text;
    try {
        body = text ? JSON.parse(text) : null;
    }
    catch {
        // Not JSON: keep the text as the hint below.
    }
    let code = byStatus(status);
    let hint = typeof body === 'string' && body ? body : `HTTP ${status}`;
    let cap;
    let retry;
    if (isRecord(body)) {
        const err = body.error;
        if (isRecord(err)) {
            if (typeof err.code === 'string')
                code = err.code;
            if (typeof err.hint === 'string')
                hint = err.hint;
        }
        else if (typeof err === 'string') {
            hint = typeof body.message === 'string' ? `${err}: ${body.message}` : err;
            const missing = /^missing capability (.+)$/.exec(err);
            const mapped = TEXT_CODES.find(([re]) => re.test(err));
            if (missing) {
                code = 'cap_not_granted';
                cap = missing[1];
            }
            else if (mapped) {
                code = mapped[1];
            }
            else if (/^[a-z][a-z0-9_]*$/.test(err)) {
                code = err;
            }
        }
        if (typeof body.retryAfterSecs === 'number')
            retry = body.retryAfterSecs;
    }
    if (retry === undefined && retryAfterHeader && /^\d+$/.test(retryAfterHeader))
        retry = Number(retryAfterHeader);
    return new K2Error({ code, hint, verb, status, cap, retryAfterSecs: retry, body });
}
