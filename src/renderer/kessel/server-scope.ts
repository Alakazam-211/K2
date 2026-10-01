// ServerScope — which K2 server a request, socket, event bus or storage key
// belongs to (Home M1, `prd-home-plan-v1.md` "Architecture decision").
//
// The client is moving from "one window = one server" to a multi-server
// client: a Home room can show an agent that lives on server B while the
// window is connected to server A. Every request helper (`daemonCli*`),
// every creds lookup (`getDaemonWs`), every socket and every app-level event
// bus therefore takes a `ServerScope` argument. It is REQUIRED — there is no
// silent default — so the typechecker lists every call site.
//
// Two kinds of scope:
//
//   - `primaryScope()` — the window's active server, resolved at CALL time
//     from `useConnectHostStore.activeHost`, exactly as `getDaemonWs` always
//     did. Its `hostKey` follows a server switch. All of today's code runs on
//     it, so behaviour is unchanged.
//   - `scopeForHost(ref)` — one specific saved server (or this computer's
//     daemon), pinned by host key. Its creds come from the switcher's saved
//     login for that server (the `host-ops` / Home `credsForHomeHost` path),
//     read at call time so a re-login is picked up. It never follows a
//     server switch.
//
// Host keys match Home's `handle::host` keys and are canonical
// (`lib/host-key.ts`, MS60): `local`, `<sub>.k2.dev` for https on 443,
// `host:port` otherwise (plain http on 80 included), `[v6]:port` for IPv6.
// `scopeForHost('https://DTL.k2.dev/')`, `scopeForHost('dtl.k2.dev:443')` and
// `scopeForHost('dtl.k2.dev')` are one scope.
//
// `id` is the registry identity (event buses, dial queues): `primary` for the
// primary scope (its subscribers survive a server switch, as the module-level
// registries always did), `host:<hostKey>` for a pinned scope.

import {
  daemonHttpBase,
  daemonWsBase,
  getLocalDaemonWs,
  resolveWindowHostCreds,
  type DaemonWsAvailable,
} from '@/kessel/daemon-ws'
import {
  activeHostKey,
  useConnectHostStore,
  type ActiveHost,
  type ConnectHost,
} from '@/stores/connect-host'
import { LOCAL_HOME_HOST, canonicalHostKey, homeHostKey, savedHostForKey } from '@/lib/host-key'
import { FEATURES, gte, serverSupports, type FeatureKey } from '@/lib/server-capabilities'
import { isReportedFeature, type ReportedFeatureKey } from '@/lib/reported-features'
import { hostScopedKey } from '@/lib/host-scoped-storage'
import { ROOM_WRITE_ROUTES } from '@/kessel/room-writes'

export interface ServerScope {
  /** Registry identity: `primary`, or `host:<hostKey>` for a pinned scope. */
  readonly id: string
  /** True for the window's primary scope (follows the server switcher). */
  readonly isPrimary: boolean
  /** Home-style host key at call time: `local`, `<sub>.k2.dev`, `ip:port`. */
  readonly hostKey: string
  /** Connection identity used by the request layer's host-switched guard.
   *  For the primary scope it is `activeHostKey(activeHost)` (changes on a
   *  switch, or a re-add with a new id); for a pinned scope it is fixed. */
  readonly connectionKey: string
  /** Is this server another machine (not this computer's daemon)? */
  readonly isRemote: boolean
  /** Human label: the saved server's label, or **This computer** for
   *  `local`. For logs and room copy. */
  readonly label: string
  /** The saved server entry right now (fresh token), or null for `local`
   *  or a pinned host that is no longer saved. */
  connectHost(): ConnectHost | null
  /** Does this scope currently point at the window's active connection?
   *  The window-level recovery gate, revive banner and soft health probe
   *  only apply then. Always true for the primary scope. */
  isWindowHost(): boolean
  /** Resolve `{host, port, token, secure}` at call time. */
  creds(): Promise<DaemonWsAvailable>
  /** `<scheme>://<host>[:<port>]` for HTTP. */
  httpBase(): Promise<string>
  /** `<ws|wss>://<host>[:<port>]` for sockets. */
  wsBase(): Promise<string>
  /** Does this server support `feature`? A version key (`FEATURES`) is
   *  decided by its version; a reported key (`REPORTED_FEATURES`) by the
   *  `features` its `/boot-status` listed (unknown ⇒ false). This computer's
   *  daemon supports everything. */
  serverSupports(feature: FeatureKey | ReportedFeatureKey): boolean
  /** Home M4: a view-only room's scope (`viewOnlyScope`). The request layer
   *  refuses every POST on it except the keep-alive (`projects/activate`).
   *  Absent / false for every other scope. */
  readonly viewOnly?: boolean
  /** Home M5: a usable remote room's scope (`remoteRoomScope`). The request
   *  layer sends a write on it only when the route is in the room write
   *  allowlist (`kessel/room-writes.ts`), so a room can do what a room does
   *  on its server and nothing else. Absent / false for every other scope. */
  readonly remoteRoom?: boolean
}

/** What `scopeForHost` accepts: `local`, a Home host key string, or a saved
 *  server (matched by hostname + port, never by its client-made id). */
export type HostRef = 'local' | string | Pick<ConnectHost, 'hostname' | 'port' | 'secure'>

function windowActiveHost(): ActiveHost {
  return useConnectHostStore.getState().activeHost
}

/** What `label` says for this computer's daemon. */
export const LOCAL_SCOPE_LABEL = 'This computer'

function hostLabel(host: ConnectHost | 'local'): string {
  return host === 'local' ? LOCAL_SCOPE_LABEL : host.label || host.hostname
}

/** A pinned scope's server is no longer in the saved list (MS10). The room
 *  shows "This server was removed from this computer." It never falls back
 *  to another saved entry or to the window's server. */
export class ServerRemovedError extends Error {
  readonly hostKey: string
  constructor(hostKey: string) {
    super(`no saved server for ${hostKey}: it was removed from this computer`)
    this.name = 'ServerRemovedError'
    this.hostKey = hostKey
  }
}

// Hosted web (G11): `activeHost` can still read 'local' while the creds are
// forced same-origin (`resolveWindowHostCreds`), so the primary `hostKey` is
// `local` on web. That is harmless for single-server web.
const PRIMARY: ServerScope = {
  id: 'primary',
  isPrimary: true,
  get hostKey(): string {
    return homeHostKey(windowActiveHost())
  },
  get connectionKey(): string {
    return activeHostKey(windowActiveHost())
  },
  get isRemote(): boolean {
    return windowActiveHost() !== 'local'
  },
  get label(): string {
    return hostLabel(windowActiveHost())
  },
  connectHost(): ConnectHost | null {
    const active = windowActiveHost()
    return active === 'local' ? null : active
  },
  isWindowHost(): boolean {
    return true
  },
  creds(): Promise<DaemonWsAvailable> {
    return resolveWindowHostCreds()
  },
  async httpBase(): Promise<string> {
    return daemonHttpBase(await resolveWindowHostCreds())
  },
  async wsBase(): Promise<string> {
    return daemonWsBase(await resolveWindowHostCreds())
  },
  serverSupports(feature: FeatureKey | ReportedFeatureKey): boolean {
    if (isReportedFeature(feature)) {
      const active = windowActiveHost()
      return active === 'local' || reportedSupports(homeHostKey(active), feature)
    }
    return serverSupports(feature)
  },
}

/** The window's active server, resolved at call time — exactly what
 *  `getDaemonWs()` resolved before M1. One shared object. */
export function primaryScope(): ServerScope {
  return PRIMARY
}

// ── Pinned scopes ────────────────────────────────────────────────────────

/** Known versions for servers that are NOT the window's server. The window's
 *  server keeps using `connect-host.serverVersion`. Filled by whoever reads a
 *  server's `/boot-status` (M2's connection pool). Unknown → feature gates
 *  return false, the same rule as an active remote with no version yet. */
const knownVersions = new Map<string, string>()
/** The `features` each server's `/boot-status` listed (REPORTED_FEATURES).
 *  No entry ⇒ every reported key reads false. */
const knownFeatures = new Map<string, ReadonlySet<string>>()

/** Note what a server's `/boot-status` said: its version and, when given,
 *  its `features` list (a body without one is an older daemon: none). */
export function noteServerVersion(hostKey: string, version: string | null, features?: readonly string[]): void {
  const key = canonicalHostKey(hostKey)
  if (version) knownVersions.set(key, version)
  else knownVersions.delete(key)
  if (features !== undefined) knownFeatures.set(key, new Set(features))
}

function reportedSupports(hostKey: string, feature: ReportedFeatureKey): boolean {
  return knownFeatures.get(canonicalHostKey(hostKey))?.has(feature) ?? false
}


/** Host key for any `HostRef`. */
export function hostKeyOf(ref: HostRef): string {
  if (ref === LOCAL_HOME_HOST) return LOCAL_HOME_HOST
  if (typeof ref === 'string') return canonicalHostKey(ref)
  return homeHostKey(ref)
}

function windowHostKey(): string {
  return homeHostKey(windowActiveHost())
}

function makeHostScope(hostKey: string): ServerScope {
  const isLocal = hostKey === LOCAL_HOME_HOST
  const saved = (): ConnectHost | null =>
    isLocal ? null : savedHostForKey(useConnectHostStore.getState().hosts, hostKey)
  const creds = async (): Promise<DaemonWsAvailable> => {
    if (isLocal) return getLocalDaemonWs()
    const host = saved()
    if (!host) throw new ServerRemovedError(hostKey)
    return { port: host.port, token: host.token, host: host.hostname, secure: host.secure }
  }
  const isWindowHost = (): boolean => windowHostKey() === hostKey
  return {
    id: `host:${hostKey}`,
    isPrimary: false,
    hostKey,
    connectionKey: `host:${hostKey}`,
    isRemote: !isLocal,
    get label(): string {
      if (isLocal) return LOCAL_SCOPE_LABEL
      const host = saved()
      return host ? hostLabel(host) : hostKey
    },
    connectHost: saved,
    isWindowHost,
    creds,
    async httpBase(): Promise<string> {
      return daemonHttpBase(await creds())
    },
    async wsBase(): Promise<string> {
      return daemonWsBase(await creds())
    },
    serverSupports(feature: FeatureKey | ReportedFeatureKey): boolean {
      if (isLocal) return true
      if (isReportedFeature(feature)) return reportedSupports(hostKey, feature)
      if (isWindowHost()) return serverSupports(feature)
      const version = knownVersions.get(hostKey)
      if (!version) return false
      return gte(version, FEATURES[feature])
    },
  }
}

const pinned = new Map<string, ServerScope>()

/** A scope pinned to one server, built from the switcher's saved login for
 *  it. The same object comes back for the same host key. */
export function scopeForHost(ref: HostRef): ServerScope {
  const key = hostKeyOf(ref)
  if (!key) throw new Error('scopeForHost: empty host key')
  let scope = pinned.get(key)
  if (!scope) {
    scope = makeHostScope(key)
    pinned.set(key, scope)
  }
  return scope
}

// ── View-only scopes (Home M4) ───────────────────────────────────────────
//
// A remote room opened from Home in M4 is READ-ONLY on its server: it shows
// B's tabs, drawers and terminals, holds B's presence socket and tells B it
// is watching (`projects/activate`), and changes nothing else on B. The room
// runs on a view-only twin of B's scope: the same server, the same id (so
// buses, dial queues and caps are shared), the same creds — but
// `daemonCliPost` refuses every route except `VIEW_ONLY_POST_ROUTES` with a
// `ViewOnlyWriteError`. That is the one net under every write path in the
// room's components (file ops, renames, compose sends, toggles). What stays
// allowed past M4 is M5's decision.

/** POST routes a view-only room may still send: the keep-alive (MS39), and
 *  file search, which is a read the daemon takes as POST (`fs/search-tree`). */
export const VIEW_ONLY_POST_ROUTES: ReadonlySet<string> = new Set(['projects/activate', 'fs/search-tree'])

/** A write was attempted from a view-only (M4 preview) room. */
export class ViewOnlyWriteError extends Error {
  readonly hostKey: string
  readonly route: string
  constructor(hostKey: string, route: string) {
    super(`View only (preview): ${route} is not sent to ${hostKey} from a remote room`)
    this.name = 'ViewOnlyWriteError'
    this.hostKey = hostKey
    this.route = route
  }
}

const viewOnlyTwins = new WeakMap<ServerScope, ServerScope>()

/** The view-only twin of `base` (one object per base scope). Every field
 *  and method is `base`'s; only `viewOnly` differs. */
export function viewOnlyScope(base: ServerScope): ServerScope {
  if (base.viewOnly) return base
  let twin = viewOnlyTwins.get(base)
  if (!twin) {
    twin = Object.create(base, { viewOnly: { value: true, enumerable: true } }) as ServerScope
    viewOnlyTwins.set(base, twin)
  }
  return twin
}

// ── Usable remote rooms (Home M5) ────────────────────────────────────────
//
// A Home room on another server that the user can USE (type, open / close /
// split tabs, rename, file writes, chat-history resume, heartbeat launch …)
// runs on a remote-room twin of that server's scope: the same server, id and
// creds, so every write goes to THAT server and nowhere else. On top, the
// request layer sends a write only when its route is in `ROOM_WRITE_ROUTES`
// (`kessel/room-writes.ts`) — a narrow, explicit list of what a room does on
// its server. Anything else (projects/delete, presets, settings, mail,
// Finder on the daemon's machine …) is refused with `RoomWriteRefusedError`
// before any request leaves. This computer's Tauri commands stay behind the
// MS67 local-only gates (`lib/local-only-actions.ts`).

/** A write from a usable remote room that is not on the room allowlist. */
export class RoomWriteRefusedError extends Error {
  readonly hostKey: string
  readonly route: string
  constructor(hostKey: string, route: string) {
    super(`Remote room: ${route} is not something a room sends to ${hostKey}`)
    this.name = 'RoomWriteRefusedError'
    this.hostKey = hostKey
    this.route = route
  }
}

const remoteRoomTwins = new WeakMap<ServerScope, ServerScope>()

/** The usable remote-room twin of `base` (one object per base scope). Every
 *  field and method is `base`'s; only `remoteRoom` differs. A view-only
 *  scope never becomes usable, and the window's primary scope never becomes
 *  a room scope (a pinned room is always pinned to one server). */
export function remoteRoomScope(base: ServerScope): ServerScope {
  if (base.remoteRoom) return base
  if (base.viewOnly) throw new Error('remoteRoomScope: a view-only scope cannot become a usable room scope')
  if (base.isPrimary) throw new Error('remoteRoomScope: a room scope is pinned to one server (scopeForHost)')
  let twin = remoteRoomTwins.get(base)
  if (!twin) {
    twin = Object.create(base, { remoteRoom: { value: true, enumerable: true } }) as ServerScope
    remoteRoomTwins.set(base, twin)
  }
  return twin
}

/** Throws when a write on `scope` to `route` is not allowed: a view-only
 *  room sends only the keep-alive (`ViewOnlyWriteError`); a usable remote
 *  room only the room allowlist (`RoomWriteRefusedError`). Every other scope
 *  passes. The request layer calls it before every POST; code that sends a
 *  write any other way (a raw spawn / close fetch, a GET-shaped verb such as
 *  `heartbeat/launch`) calls it itself. */
export function assertScopeMayWrite(scope: ServerScope, route: string): void {
  if (scope.viewOnly && !VIEW_ONLY_POST_ROUTES.has(route)) {
    throw new ViewOnlyWriteError(scope.hostKey, route)
  }
  if (scope.remoteRoom && !ROOM_WRITE_ROUTES.has(route)) {
    throw new RoomWriteRefusedError(scope.hostKey, route)
  }
}

/** May `scope` send a write to `route`? (UI: hide what the request layer
 *  would refuse.) */
export function scopeMayWrite(scope: ServerScope, route: string): boolean {
  if (scope.viewOnly) return VIEW_ONLY_POST_ROUTES.has(route)
  if (scope.remoteRoom) return ROOM_WRITE_ROUTES.has(route)
  return true
}

/** The request layer's POST check (`daemonCliPost`). */
export function assertScopeMayPost(scope: ServerScope, route: string): void {
  assertScopeMayWrite(scope, route)
}

/** Prefix a workspace-only storage key (or in-memory map key) with the
 *  scope's host key, so two servers with the same paths and ids never share
 *  an entry. `<hostKey>|<key>`. */
export function scopedKey(scope: ServerScope, key: string): string {
  return hostScopedKey(scope.hostKey, key)
}

export function __resetServerScopesForTests(): void {
  pinned.clear()
  knownVersions.clear()
  knownFeatures.clear()
}
