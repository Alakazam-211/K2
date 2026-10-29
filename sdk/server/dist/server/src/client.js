// The server SDK's client (runtime S23, §5.2): one door, two actors.
//
// - Every call carries one pass. **The default actor is the guest of the
//   request you are serving** (`withGuest`, `middleware`); a call with no
//   guest bound fails `guest_required`. Acting as the App is written out:
//   `k2.asApp()`. `k2.asGuest(pass)` names a guest explicitly.
// - The App pass is a file K2 rotates; it is read again when it changes
//   and once more (with one retry of the call) when K2 answers
//   `pass_invalid`.
// - Answers are projected onto the catalog's closed shapes (`wire.ts`).
// - Users and rooms of this App go through the App key on
//   `/v1/apps/<K2_APP_ID>/…` (S45), not a pass.
// - RS-A15 (0.46.1): with the owner's `guests-rooms` grant, the App's own
//   pass reads and sets its guests' rooms through the catalog rows:
//   `k2.asApp().guests.rooms({username})` and
//   `k2.asApp().guests.setRooms({username, rooms})` (no App key, no owner
//   token).
import { AsyncLocalStorage } from 'node:async_hooks';
import { K2Error, fromResponse } from "./errors.js";
import { APP_VERBS } from "./generated/verbs.js";
import { SecretFile } from "./pass.js";
import { openSocket } from "./socket.js";
import { watchThreadActivity } from "./activity.js";
import { buildRequest, project } from "./wire.js";
/** The verbs a call needs no pass's identity for, beyond reaching the link. */
const LOGIN_VERB = 'guests.login';
/** The must-change answer inside a `password_change_required` refusal, or
 *  null for any other error. A malformed one is rethrown (never a login). */
function mustChangeOf(e) {
    if (!(e instanceof K2Error) || e.code !== 'password_change_required')
        return null;
    const b = e.body;
    if (typeof b !== 'object' || b === null)
        throw e;
    const v = b;
    const tok = v.changeToken;
    if (v.mustChangePassword !== true || typeof tok !== 'string' || !/^k2chg_[0-9a-f]+$/.test(tok))
        throw e;
    return {
        ok: false,
        mustChangePassword: true,
        changeToken: tok,
        changeUrl: typeof v.changeUrl === 'string' ? v.changeUrl : null,
        changePath: typeof v.changePath === 'string' ? v.changePath : '/cli/skin/password/set',
        changeExpiresAt: typeof v.changeExpiresAt === 'number' ? v.changeExpiresAt : 0,
        username: typeof v.username === 'string' ? v.username : '',
        app: typeof v.app === 'string' ? v.app : '',
    };
}
function isGuestPass(p) {
    return /^k2skn_[A-Za-z0-9._-]+$/.test(p);
}
function header(req, name) {
    const h = req.headers;
    if (typeof h.get === 'function')
        return h.get(name);
    const v = h[name.toLowerCase()];
    return Array.isArray(v) ? (v[0] ?? null) : (v ?? null);
}
export class K2App {
    gatewayUrl;
    appId;
    appName;
    passFile;
    keyFile;
    fetchImpl;
    WebSocketImpl;
    guest = new AsyncLocalStorage();
    defaultActor;
    appActor;
    constructor(cfg) {
        const url = cfg.gatewayUrl.replace(/\/+$/, '');
        if (!/^https?:\/\/[^/]+$/.test(url)) {
            throw new K2Error({ code: 'usage', hint: `gatewayUrl must be http://host:port, got ${JSON.stringify(cfg.gatewayUrl)}` });
        }
        this.gatewayUrl = url;
        this.appId = cfg.appId ?? null;
        this.appName = cfg.appName ?? null;
        this.passFile = cfg.passFile ? new SecretFile(cfg.passFile) : null;
        this.keyFile = cfg.appKeyFile ? new SecretFile(cfg.appKeyFile) : null;
        this.fetchImpl = cfg.fetch ?? fetch;
        this.WebSocketImpl = cfg.WebSocket ?? globalThis.WebSocket ?? null;
        this.defaultActor = this.actor({ kind: 'default' });
        this.appActor = this.actor({ kind: 'app' });
        Object.assign(this, surfacesOf(this.defaultActor), { thread: this.defaultActor.thread });
    }
    // ── actors ─────────────────────────────────────────────────────────
    /** Calls as the App itself (its pass file). Written out on purpose. */
    asApp() {
        return this.appActor;
    }
    /** Calls as one guest (their session pass, from `login`). */
    asGuest(pass) {
        if (!isGuestPass(pass))
            throw new K2Error({ code: 'usage', hint: 'asGuest takes a guest session pass (k2skn_…)' });
        return this.actor({ kind: 'guest', pass });
    }
    /** Run `fn` with `pass` as the default actor for every call inside it. */
    withGuest(pass, fn) {
        if (!isGuestPass(pass))
            throw new K2Error({ code: 'usage', hint: 'withGuest takes a guest session pass (k2skn_…)' });
        return this.guest.run(pass, fn);
    }
    /** The guest bound to the current call chain, if any. */
    currentGuest() {
        return this.guest.getStore() ?? null;
    }
    /**
     * The guest pass a request carries: `Authorization: Bearer k2skn_…`, or
     * the `cookie` named (default `k2_skin_session`). Null when none.
     */
    guestFrom(req, cookie = 'k2_skin_session') {
        const auth = header(req, 'authorization');
        const bearer = auth && /^bearer\s+(\S+)$/i.exec(auth.trim());
        if (bearer && isGuestPass(bearer[1]))
            return bearer[1];
        const raw = header(req, 'cookie');
        if (raw) {
            for (const part of raw.split(';')) {
                const [k, ...rest] = part.trim().split('=');
                const v = decodeURIComponent(rest.join('='));
                if (k === cookie && isGuestPass(v))
                    return v;
            }
        }
        return null;
    }
    /**
     * Connect/Express-style middleware: the request's guest (guestFrom) is
     * the default actor for everything the handler calls.
     */
    middleware(cookie = 'k2_skin_session') {
        return (req, _res, next) => {
            const pass = this.guestFrom(req, cookie);
            if (pass)
                this.withGuest(pass, () => next());
            else
                next();
        };
    }
    // ── login ──────────────────────────────────────────────────────────
    /**
     * Log a guest in through this App (`guests.login`; `app` is this App's
     * id). Through a wall's link the App pass opens the door; the answer's
     * `pass` is the guest's session pass.
     */
    //
    // Must-change (K2 0.46.6, catalog v37): when someone else set this
    // person's password (the owner, an admin, an agent, an App key, or K2
    // generated it), the right password gets NO pass. The answer is then
    // {ok: false, mustChangePassword: true, changeToken, changeUrl, ...}
    // (AppGuestMustChange): send the person's browser to changeUrl, where
    // they choose a new password; then they sign in with it. Check
    // `'mustChangePassword' in s` before using `s.pass`.
    async login(args) {
        try {
            return await this.actor({ kind: 'default' }).call(LOGIN_VERB, args);
        }
        catch (e) {
            const changed = mustChangeOf(e);
            if (changed)
                return changed;
            throw e;
        }
    }
    // ── users and rooms (the App key) ──────────────────────────────────
    users = {
        list: () => this.appKey('GET', 'users'),
        create: (body) => this.appKey('POST', 'users', body),
        setRooms: (body) => this.appKey('POST', 'users/rooms', body),
        disable: (body) => this.appKey('POST', 'users/disable', body),
        enable: (body) => this.appKey('POST', 'users/enable', body),
        remove: (body) => this.appKey('POST', 'users/remove', body),
        unlock: (body) => this.appKey('POST', 'users/unlock', body),
        passwordReset: (body) => this.appKey('POST', 'users/password-reset', body),
    };
    rooms = {
        /** A user's rooms in this App ([] takes every room away). */
        set: (body) => this.appKey('POST', 'users/rooms', body),
    };
    /** The App key's own view: this App, its scopes and ceiling. */
    keyWhoami() {
        return this.appKey('GET', 'whoami');
    }
    async appKey(method, verb, body) {
        const where = `app key ${verb}`;
        if (!this.keyFile || !this.keyFile.exists()) {
            throw new K2Error({ code: 'no_app_key', hint: 'no App key: the owner runs k2 app key mint <app> --scopes users,rooms (it arrives as $K2_SECRETS_DIR/K2_APP_KEY)', verb: where });
        }
        if (!this.appId)
            throw new K2Error({ code: 'usage', hint: 'K2_APP_ID is not set: the App key opens /v1/apps/<K2_APP_ID>/… only', verb: where });
        const key = this.keyFile.read();
        const res = await this.fetchImpl(`${this.gatewayUrl}/v1/apps/${encodeURIComponent(this.appId)}/${verb}`, {
            method,
            headers: { authorization: `Bearer ${key}`, ...(method === 'POST' ? { 'content-type': 'application/json' } : {}) },
            body: method === 'POST' ? JSON.stringify(body ?? {}) : undefined,
        });
        const text = await res.text();
        if (!res.ok)
            throw fromResponse(where, res.status, text, res.headers.get('retry-after'));
        return JSON.parse(text);
    }
    // ── the transport ─────────────────────────────────────────────────
    actor(cred) {
        const a = {
            call: (verb, args) => this.callAs(cred, verb, args),
            subscribe: (verb, args, onFrame, onError, opts) => this.subscribeAs(cred, verb, args, onFrame, onError, opts),
        };
        const out = Object.assign(a, surfacesOf(a));
        // 0.46.5 activity SDK: the Thread turn, live and right (activity.ts).
        // A socket opened inside withGuest keeps that guest, like subscribe.
        const thread = out.thread;
        thread.watchActivity = (target, cb, opts) => {
            const bound = cred.kind === 'default' ? { kind: 'guest', pass: this.passFor(cred, 'thread.stream') } : cred;
            const b = this.actor(bound);
            return watchThreadActivity(b, target, cb, opts);
        };
        return out;
    }
    row(verb) {
        const row = APP_VERBS[verb];
        if (!row)
            throw new K2Error({ code: 'unknown_verb', hint: `${verb} isn't an App verb (k2 study app-api lists them)`, verb });
        return row;
    }
    /** The pass a call presents, or null (a login on a --cmd service: no pass needed). */
    passFor(cred, verb) {
        if (verb === LOGIN_VERB) {
            // A login opens the link with the App's pass when it has one.
            return this.passFile && this.passFile.exists() ? this.passFile.read() : null;
        }
        if (cred.kind === 'guest')
            return cred.pass;
        if (cred.kind === 'default') {
            const g = this.guest.getStore();
            if (g)
                return g;
            throw new K2Error({
                code: 'guest_required',
                hint: `${verb}: no guest is bound to this call; serve the request inside k2.withGuest(pass, …) (or k2.middleware()), or write k2.asApp() to act as the App`,
                verb,
            });
        }
        if (!this.passFile || !this.passFile.exists()) {
            throw new K2Error({ code: 'no_app_pass', hint: `${verb}: this App has no pass file (K2_PASS_FILE); asApp works inside a wall`, verb });
        }
        return this.passFile.read();
    }
    async callAs(cred, verb, args) {
        const row = this.row(verb);
        if (!row.http)
            throw new K2Error({ code: 'usage', hint: `${verb} is a subscribe verb: use subscribe`, verb });
        if (row.door === 'helper')
            throw new K2Error({ code: 'not_on_link', hint: `${verb} is for a guest's own browser session, never a server`, verb });
        let callArgs = args;
        if (verb === LOGIN_VERB && this.appId && (args?.app === undefined || args.app === ''))
            callArgs = { ...args, app: this.appId };
        const req = buildRequest(row.http, callArgs);
        const usesAppPass = verb === LOGIN_VERB || cred.kind === 'app';
        for (let attempt = 0;; attempt++) {
            const pass = this.passFor(cred, verb);
            const headers = {};
            if (pass)
                headers.authorization = `Bearer ${pass}`;
            if (req.body !== null)
                headers['content-type'] = 'application/json';
            const res = await this.fetchImpl(`${this.gatewayUrl}${req.pathAndQuery}`, { method: req.method, headers, body: req.body ?? undefined });
            const text = await res.text();
            if (res.ok) {
                const v = text ? JSON.parse(text) : null;
                return project(row.response, v, row.responseNames);
            }
            const err = fromResponse(verb, res.status, text, res.headers.get('retry-after'));
            // The App pass rotated under us: read the file again, once.
            if (attempt === 0 && usesAppPass && err.code === 'pass_invalid' && this.passFile)
                continue;
            throw err;
        }
    }
    subscribeAs(cred, verb, args, onFrame, onError, opts) {
        const row = this.row(verb);
        const sock = row.socket;
        if (!sock)
            throw new K2Error({ code: 'usage', hint: `${verb} is a call verb: use call`, verb });
        const WS = this.WebSocketImpl;
        if (!WS)
            throw new K2Error({ code: 'verb_unavailable', hint: 'no WebSocket in this runtime (Node 22+ has one)', verb });
        const value = args?.[sock.param];
        if (typeof value !== 'string' || !value)
            throw new K2Error({ code: 'usage', hint: `${verb} needs args.${sock.param}`, verb });
        // Resolve the actor now: a socket opened inside withGuest keeps that guest.
        const bound = cred.kind === 'default' ? { kind: 'guest', pass: this.passFor(cred, verb) } : cred;
        return openSocket({
            verb,
            row,
            WebSocket: WS,
            url: () => {
                const pass = this.passFor(bound, verb);
                const q = new URLSearchParams({ [sock.param]: value });
                // Fixed opt-ins the row names (0.46.3: `ear=1`, the guest ear on
                // the Thread sockets).
                for (const [k, v] of Object.entries(sock.query ?? {}))
                    q.set(k, v);
                if (pass)
                    q.set('token', pass);
                return `${this.gatewayUrl.replace(/^http/, 'ws')}${sock.path}?${q.toString()}`;
            },
            onFrame,
            onOpen: opts?.onOpen,
            onError: onError ??
                ((e) => {
                    console.warn(`[k2] ${e.message}`);
                }),
        });
    }
}
/** One object per surface, each action calling `a.call` / `a.subscribe`. */
function surfacesOf(a) {
    const out = {};
    for (const [verb, row] of Object.entries(APP_VERBS)) {
        if (row.door === 'helper')
            continue;
        const [surface, action] = verb.split('.');
        const s = (out[surface] ??= {});
        s[action] =
            row.kind === 'subscribe'
                ? (args, onFrame, onError) => a.subscribe(verb, args, onFrame, onError)
                : (args) => a.call(verb, args);
    }
    return out;
}
