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
  /** Does this server's version support `feature`? */
  serverSupports(feature: FeatureKey): boolean
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
  serverSupports(feature: FeatureKey): boolean {
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

export function noteServerVersion(hostKey: string, version: string | null): void {
  const key = canonicalHostKey(hostKey)
  if (version) knownVersions.set(key, version)
  else knownVersions.delete(key)
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
    serverSupports(feature: FeatureKey): boolean {
      if (isLocal) return true
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

/** Prefix a workspace-only storage key (or in-memory map key) with the
 *  scope's host key, so two servers with the same paths and ids never share
 *  an entry. `<hostKey>|<key>`. */
export function scopedKey(scope: ServerScope, key: string): string {
  return `${scope.hostKey}|${key}`
}

export function __resetServerScopesForTests(): void {
  pinned.clear()
  knownVersions.clear()
}
