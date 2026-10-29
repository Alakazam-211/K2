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

import { AsyncLocalStorage } from 'node:async_hooks'

import { K2Error, fromResponse } from './errors.ts'
import { APP_VERBS, type AppCallVerb, type AppSubscribeVerb, type AppSurfaces, type AppVerbRow, type AppVerbs, type AppGuestLogin } from './generated/verbs.ts'
import { SecretFile } from './pass.ts'
import { openSocket, type WebSocketCtor } from './socket.ts'
import { watchThreadActivity, type ActivityActor, type ActivityState, type WatchActivityOptions, type WatchActivityTarget } from './activity.ts'
import { buildRequest, project } from './wire.ts'

export interface K2Config {
  /** Where K2 is: K2_GATEWAY_URL (in a wall http://127.0.0.1:8730). */
  gatewayUrl: string
  /** The App pass file (K2_PASS_FILE); null when this App has none. */
  passFile?: string | null
  /** This App's service id (K2_APP_ID): logins name it. */
  appId?: string | null
  /** This App's slug (K2_APP_NAME). */
  appName?: string | null
  /** The App key file ($K2_SECRETS_DIR/K2_APP_KEY); null when not granted. */
  appKeyFile?: string | null
  fetch?: typeof fetch
  WebSocket?: WebSocketCtor
}

/** Who a call acts as. */
type Cred = { kind: 'guest'; pass: string } | { kind: 'app' } | { kind: 'default' }

/** The verbs a call needs no pass's identity for, beyond reaching the link. */
const LOGIN_VERB = 'guests.login' as const

/**
 * A login whose password someone else set (K2 0.46.6: the owner, an
 * admin, an agent, an App key, or K2 generated it): no pass and no
 * session. Send the person's browser to `changeUrl` (from a fetch-based
 * login, hand it to your page and set `location.href`). With no
 * `changeUrl`, your own form posts `{token: changeToken, password}` to
 * `POST /cli/skin/password/reset` through your link. They choose a new
 * password (it must differ from the temporary one: else 400
 * `password_unchanged`, and the token still works), then sign in with it.
 * The change token works once, for 15 minutes; signing in again with the
 * temporary password gets a new one. Never log it. A person's own
 * password (a reset link, password.change) never asks for this.
 */
export interface AppGuestMustChange {
  ok: false
  mustChangePassword: true
  /** One-time token (`k2chg_…`) for `POST /cli/skin/password/reset` `{token, password}`. */
  changeToken: string
  /** The App's own set-password page (`k2 app login settings <app> --set-password-url`), else K2's on the box's front door, with the token in the `#fragment`; null when there is neither. */
  changeUrl: string | null
  /** K2's set-password page path on any K2 door (`/cli/skin/password/set`). */
  changePath: string
  /** When the change token ends, seconds since the epoch. */
  changeExpiresAt: number
  username: string
  app: string
}

/** The must-change answer inside a `password_change_required` refusal, or
 *  null for any other error. A malformed one is rethrown (never a login). */
function mustChangeOf(e: unknown): AppGuestMustChange | null {
  if (!(e instanceof K2Error) || e.code !== 'password_change_required') return null
  const b = e.body
  if (typeof b !== 'object' || b === null) throw e
  const v = b as Record<string, unknown>
  const tok = v.changeToken
  if (v.mustChangePassword !== true || typeof tok !== 'string' || !/^k2chg_[0-9a-f]+$/.test(tok)) throw e
  return {
    ok: false,
    mustChangePassword: true,
    changeToken: tok,
    changeUrl: typeof v.changeUrl === 'string' ? v.changeUrl : null,
    changePath: typeof v.changePath === 'string' ? v.changePath : '/cli/skin/password/set',
    changeExpiresAt: typeof v.changeExpiresAt === 'number' ? v.changeExpiresAt : 0,
    username: typeof v.username === 'string' ? v.username : '',
    app: typeof v.app === 'string' ? v.app : '',
  }
}

function isGuestPass(p: string): boolean {
  return /^k2skn_[A-Za-z0-9._-]+$/.test(p)
}

/** `subscribe`'s options (0.46.5). */
export interface SubscribeOptions {
  /** The socket is open (first time and after every reconnect): catch up
   *  here, after the gap, never on the close-time `resync` error. */
  onOpen?: () => void
}

/** The surfaces plus `call` / `subscribe` by name, bound to one actor. */
export interface K2Actor extends Omit<AppSurfaces, 'thread'> {
  /** Every Thread verb, plus `watchActivity` (0.46.5): the live Thread
   *  turn of one agent (`{addr}`) or of rooms (`{rooms}`), kept right
   *  (snapshot on every open, frames, resets, a 60 s self-check). */
  readonly thread: AppSurfaces['thread'] & {
    watchActivity(
      target: WatchActivityTarget,
      cb: (states: Map<string, ActivityState>) => void,
      opts?: WatchActivityOptions,
    ): () => void
  }
  call<V extends AppCallVerb>(verb: V, args?: AppVerbs[V]['args']): Promise<AppVerbs[V]['value']>
  subscribe<V extends AppSubscribeVerb>(
    verb: V,
    args: AppVerbs[V]['args'],
    onFrame: (frame: AppVerbs[V]['value']) => void,
    onError?: (error: K2Error) => void,
    opts?: SubscribeOptions,
  ): () => void
}

/** Something with request headers (Node's IncomingMessage, a Fetch Request). */
export interface RequestLike {
  headers: Record<string, string | string[] | undefined> | { get(name: string): string | null }
}

function header(req: RequestLike, name: string): string | null {
  const h = req.headers
  if (typeof (h as { get?: unknown }).get === 'function') return (h as { get(n: string): string | null }).get(name)
  const v = (h as Record<string, string | string[] | undefined>)[name.toLowerCase()]
  return Array.isArray(v) ? (v[0] ?? null) : (v ?? null)
}

export class K2App {
  readonly gatewayUrl: string
  readonly appId: string | null
  readonly appName: string | null
  private readonly passFile: SecretFile | null
  private readonly keyFile: SecretFile | null
  private readonly fetchImpl: typeof fetch
  private readonly WebSocketImpl: WebSocketCtor | null
  private readonly guest = new AsyncLocalStorage<string>()
  private readonly defaultActor: K2Actor
  private readonly appActor: K2Actor

  constructor(cfg: K2Config) {
    const url = cfg.gatewayUrl.replace(/\/+$/, '')
    if (!/^https?:\/\/[^/]+$/.test(url)) {
      throw new K2Error({ code: 'usage', hint: `gatewayUrl must be http://host:port, got ${JSON.stringify(cfg.gatewayUrl)}` })
    }
    this.gatewayUrl = url
    this.appId = cfg.appId ?? null
    this.appName = cfg.appName ?? null
    this.passFile = cfg.passFile ? new SecretFile(cfg.passFile) : null
    this.keyFile = cfg.appKeyFile ? new SecretFile(cfg.appKeyFile) : null
    this.fetchImpl = cfg.fetch ?? fetch
    this.WebSocketImpl = cfg.WebSocket ?? (globalThis as { WebSocket?: WebSocketCtor }).WebSocket ?? null
    this.defaultActor = this.actor({ kind: 'default' })
    this.appActor = this.actor({ kind: 'app' })
    Object.assign(this, surfacesOf(this.defaultActor as unknown as ActorFns), { thread: this.defaultActor.thread })
  }

  // ── actors ─────────────────────────────────────────────────────────

  /** Calls as the App itself (its pass file). Written out on purpose. */
  asApp(): K2Actor {
    return this.appActor
  }

  /** Calls as one guest (their session pass, from `login`). */
  asGuest(pass: string): K2Actor {
    if (!isGuestPass(pass)) throw new K2Error({ code: 'usage', hint: 'asGuest takes a guest session pass (k2skn_…)' })
    return this.actor({ kind: 'guest', pass })
  }

  /** Run `fn` with `pass` as the default actor for every call inside it. */
  withGuest<T>(pass: string, fn: () => T): T {
    if (!isGuestPass(pass)) throw new K2Error({ code: 'usage', hint: 'withGuest takes a guest session pass (k2skn_…)' })
    return this.guest.run(pass, fn)
  }

  /** The guest bound to the current call chain, if any. */
  currentGuest(): string | null {
    return this.guest.getStore() ?? null
  }

  /**
   * The guest pass a request carries: `Authorization: Bearer k2skn_…`, or
   * the `cookie` named (default `k2_skin_session`). Null when none.
   */
  guestFrom(req: RequestLike, cookie = 'k2_skin_session'): string | null {
    const auth = header(req, 'authorization')
    const bearer = auth && /^bearer\s+(\S+)$/i.exec(auth.trim())
    if (bearer && isGuestPass(bearer[1] as string)) return bearer[1] as string
    const raw = header(req, 'cookie')
    if (raw) {
      for (const part of raw.split(';')) {
        const [k, ...rest] = part.trim().split('=')
        const v = decodeURIComponent(rest.join('='))
        if (k === cookie && isGuestPass(v)) return v
      }
    }
    return null
  }

  /**
   * Connect/Express-style middleware: the request's guest (guestFrom) is
   * the default actor for everything the handler calls.
   */
  middleware(cookie = 'k2_skin_session'): (req: RequestLike, res: unknown, next: (err?: unknown) => void) => void {
    return (req, _res, next) => {
      const pass = this.guestFrom(req, cookie)
      if (pass) this.withGuest(pass, () => next())
      else next()
    }
  }

  // ── login ──────────────────────────────────────────────────────────

  /**
   * Log a guest in through this App (`guests.login`; `app` is this App's
   * id). Through a wall's link the App pass opens the door; the answer's
   * `pass` is the guest's session pass.
   */
  //
  // Must-change (K2 0.46.6, catalog v38): when someone else set this
  // person's password (the owner, an admin, an agent, an App key, or K2
  // generated it), the right password gets NO pass. The answer is then
  // {ok: false, mustChangePassword: true, changeToken, changeUrl, ...}
  // (AppGuestMustChange): send the person's browser to changeUrl, where
  // they choose a new password; then they sign in with it. Check
  // `'mustChangePassword' in s` before using `s.pass`.
  async login(args: { username: string; password: string }): Promise<AppGuestLogin | AppGuestMustChange> {
    try {
      return await this.actor({ kind: 'default' }).call(LOGIN_VERB, args)
    } catch (e) {
      const changed = mustChangeOf(e)
      if (changed) return changed
      throw e
    }
  }

  // ── users and rooms (the App key) ──────────────────────────────────

  readonly users = {
    list: (): Promise<AppKeyUsers> => this.appKey('GET', 'users'),
    create: (body: { username: string; email?: string; password?: string; rooms?: string[]; role?: string }): Promise<AppKeyUserCreated> =>
      this.appKey('POST', 'users', body),
    setRooms: (body: { username: string; rooms: string[]; applyTokens?: boolean }): Promise<AppKeyRoomsSet> =>
      this.appKey('POST', 'users/rooms', body),
    disable: (body: { username: string }): Promise<AppKeyOk> => this.appKey('POST', 'users/disable', body),
    enable: (body: { username: string }): Promise<AppKeyOk> => this.appKey('POST', 'users/enable', body),
    remove: (body: { username: string }): Promise<AppKeyOk> => this.appKey('POST', 'users/remove', body),
    unlock: (body: { username: string }): Promise<AppKeyOk> => this.appKey('POST', 'users/unlock', body),
    passwordReset: (body: { username: string }): Promise<AppKeyOk> => this.appKey('POST', 'users/password-reset', body),
  }

  readonly rooms = {
    /** A user's rooms in this App ([] takes every room away). */
    set: (body: { username: string; rooms: string[]; applyTokens?: boolean }): Promise<AppKeyRoomsSet> => this.appKey('POST', 'users/rooms', body),
  }

  /** The App key's own view: this App, its scopes and ceiling. */
  keyWhoami(): Promise<AppKeyWhoami> {
    return this.appKey('GET', 'whoami')
  }

  private async appKey<T>(method: 'GET' | 'POST', verb: string, body?: unknown): Promise<T> {
    const where = `app key ${verb}`
    if (!this.keyFile || !this.keyFile.exists()) {
      throw new K2Error({ code: 'no_app_key', hint: 'no App key: the owner runs k2 app key mint <app> --scopes users,rooms (it arrives as $K2_SECRETS_DIR/K2_APP_KEY)', verb: where })
    }
    if (!this.appId) throw new K2Error({ code: 'usage', hint: 'K2_APP_ID is not set: the App key opens /v1/apps/<K2_APP_ID>/… only', verb: where })
    const key = this.keyFile.read()
    const res = await this.fetchImpl(`${this.gatewayUrl}/v1/apps/${encodeURIComponent(this.appId)}/${verb}`, {
      method,
      headers: { authorization: `Bearer ${key}`, ...(method === 'POST' ? { 'content-type': 'application/json' } : {}) },
      body: method === 'POST' ? JSON.stringify(body ?? {}) : undefined,
    })
    const text = await res.text()
    if (!res.ok) throw fromResponse(where, res.status, text, res.headers.get('retry-after'))
    return JSON.parse(text) as T
  }

  // ── the transport ─────────────────────────────────────────────────

  private actor(cred: Cred): K2Actor {
    const a = {
      call: (verb: string, args?: unknown) => this.callAs(cred, verb, args as Record<string, unknown> | undefined),
      subscribe: (verb: string, args: unknown, onFrame: (f: unknown) => void, onError?: (e: K2Error) => void, opts?: SubscribeOptions) =>
        this.subscribeAs(cred, verb, args as Record<string, unknown>, onFrame, onError, opts),
    }
    const out = Object.assign(a, surfacesOf(a)) as unknown as K2Actor
    // 0.46.5 activity SDK: the Thread turn, live and right (activity.ts).
    // A socket opened inside withGuest keeps that guest, like subscribe.
    const thread = (out as unknown as { thread: Record<string, unknown> }).thread
    thread.watchActivity = (target: WatchActivityTarget, cb: (states: Map<string, ActivityState>) => void, opts?: WatchActivityOptions) => {
      const bound: Cred = cred.kind === 'default' ? { kind: 'guest', pass: this.passFor(cred, 'thread.stream') as string } : cred
      const b = this.actor(bound) as unknown as ActivityActor
      return watchThreadActivity(b, target, cb, opts)
    }
    return out
  }

  private row(verb: string): AppVerbRow {
    const row = (APP_VERBS as Record<string, AppVerbRow>)[verb]
    if (!row) throw new K2Error({ code: 'unknown_verb', hint: `${verb} isn't an App verb (k2 study app-api lists them)`, verb })
    return row
  }

  /** The pass a call presents, or null (a login on a --cmd service: no pass needed). */
  private passFor(cred: Cred, verb: string): string | null {
    if (verb === LOGIN_VERB) {
      // A login opens the link with the App's pass when it has one.
      return this.passFile && this.passFile.exists() ? this.passFile.read() : null
    }
    if (cred.kind === 'guest') return cred.pass
    if (cred.kind === 'default') {
      const g = this.guest.getStore()
      if (g) return g
      throw new K2Error({
        code: 'guest_required',
        hint: `${verb}: no guest is bound to this call; serve the request inside k2.withGuest(pass, …) (or k2.middleware()), or write k2.asApp() to act as the App`,
        verb,
      })
    }
    if (!this.passFile || !this.passFile.exists()) {
      throw new K2Error({ code: 'no_app_pass', hint: `${verb}: this App has no pass file (K2_PASS_FILE); asApp works inside a wall`, verb })
    }
    return this.passFile.read()
  }

  private async callAs(cred: Cred, verb: string, args: Record<string, unknown> | undefined): Promise<unknown> {
    const row = this.row(verb)
    if (!row.http) throw new K2Error({ code: 'usage', hint: `${verb} is a subscribe verb: use subscribe`, verb })
    if (row.door === 'helper') throw new K2Error({ code: 'not_on_link', hint: `${verb} is for a guest's own browser session, never a server`, verb })
    let callArgs = args
    if (verb === LOGIN_VERB && this.appId && (args?.app === undefined || args.app === '')) callArgs = { ...args, app: this.appId }
    const req = buildRequest(row.http, callArgs)
    const usesAppPass = verb === LOGIN_VERB || cred.kind === 'app'
    for (let attempt = 0; ; attempt++) {
      const pass = this.passFor(cred, verb)
      const headers: Record<string, string> = {}
      if (pass) headers.authorization = `Bearer ${pass}`
      if (req.body !== null) headers['content-type'] = 'application/json'
      const res = await this.fetchImpl(`${this.gatewayUrl}${req.pathAndQuery}`, { method: req.method, headers, body: req.body ?? undefined })
      const text = await res.text()
      if (res.ok) {
        const v: unknown = text ? JSON.parse(text) : null
        return project(row.response, v, row.responseNames)
      }
      const err = fromResponse(verb, res.status, text, res.headers.get('retry-after'))
      // The App pass rotated under us: read the file again, once.
      if (attempt === 0 && usesAppPass && err.code === 'pass_invalid' && this.passFile) continue
      throw err
    }
  }

  private subscribeAs(
    cred: Cred,
    verb: string,
    args: Record<string, unknown>,
    onFrame: (f: unknown) => void,
    onError?: (e: K2Error) => void,
    opts?: SubscribeOptions,
  ): () => void {
    const row = this.row(verb)
    const sock = row.socket
    if (!sock) throw new K2Error({ code: 'usage', hint: `${verb} is a call verb: use call`, verb })
    const WS = this.WebSocketImpl
    if (!WS) throw new K2Error({ code: 'verb_unavailable', hint: 'no WebSocket in this runtime (Node 22+ has one)', verb })
    const value = args?.[sock.param]
    if (typeof value !== 'string' || !value) throw new K2Error({ code: 'usage', hint: `${verb} needs args.${sock.param}`, verb })
    // Resolve the actor now: a socket opened inside withGuest keeps that guest.
    const bound: Cred = cred.kind === 'default' ? { kind: 'guest', pass: this.passFor(cred, verb) as string } : cred
    return openSocket({
      verb,
      row,
      WebSocket: WS,
      url: () => {
        const pass = this.passFor(bound, verb)
        const q = new URLSearchParams({ [sock.param]: value })
        // Fixed opt-ins the row names (0.46.3: `ear=1`, the guest ear on
        // the Thread sockets).
        for (const [k, v] of Object.entries(sock.query ?? {})) q.set(k, v)
        if (pass) q.set('token', pass)
        return `${this.gatewayUrl.replace(/^http/, 'ws')}${sock.path}?${q.toString()}`
      },
      onFrame,
      onOpen: opts?.onOpen,
      onError:
        onError ??
        ((e) => {
          console.warn(`[k2] ${e.message}`)
        }),
    })
  }
}

/** An actor's two entry points, untyped (the surfaces type them). */
interface ActorFns {
  call: (verb: string, args?: unknown) => Promise<unknown>
  subscribe: (verb: string, args: unknown, onFrame: (f: unknown) => void, onError?: (e: K2Error) => void) => () => void
}

/** One object per surface, each action calling `a.call` / `a.subscribe`. */
function surfacesOf(a: ActorFns): Record<string, Record<string, unknown>> {
  const out: Record<string, Record<string, unknown>> = {}
  for (const [verb, row] of Object.entries(APP_VERBS as Record<string, AppVerbRow>)) {
    if (row.door === 'helper') continue
    const [surface, action] = verb.split('.') as [string, string]
    const s = (out[surface] ??= {})
    s[action] =
      row.kind === 'subscribe'
        ? (args: unknown, onFrame: (f: unknown) => void, onError?: (e: K2Error) => void) => a.subscribe(verb, args, onFrame, onError)
        : (args?: unknown) => a.call(verb, args)
  }
  return out
}

// The App key routes' answers (`/v1/apps/<id>/…`, app_key_routes.rs).
export interface AppKeyUser {
  readonly username: string
  readonly email: string | null
  readonly fullName: string | null
  readonly hasPassword: boolean
  readonly enabled: boolean
  readonly roleId: string | null
  readonly rooms: string[]
  readonly roomHandles: string[]
  readonly app: string
}
export interface AppKeyUsers {
  readonly app: string
  readonly users: AppKeyUser[]
}
export interface AppKeyUserCreated {
  readonly ok: boolean
  readonly user: AppKeyUser
}
export interface AppKeyRoomsSet {
  readonly ok: boolean
  readonly username: string
  readonly app: string
  readonly rooms: string[]
  readonly roomHandles: string[]
}
export interface AppKeyOk {
  readonly ok: boolean
  readonly username: string
  readonly [key: string]: unknown
}
export interface AppKeyWhoami {
  readonly app: string
  readonly keyId: string
  readonly scopes: string[]
  readonly rooms: string[]
  readonly roomHandles: string[]
  readonly roleIds: string[]
}

// The surfaces are attached at runtime (one per App verb): declare them.
export interface K2App extends Omit<AppSurfaces, 'thread'> {
  readonly thread: K2Actor['thread']
}
