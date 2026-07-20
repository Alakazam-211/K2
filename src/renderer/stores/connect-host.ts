// connect-host store — the host-aware connection substrate for K2 Connect
// (PRD: .k2so/prds/k2-connect-client-ux.md, build order step #1).
//
// The desktop client can target ANY daemon: the local bundled daemon
// ("This Mac"), a self-hosted box, or a hosted K2 Connect server. This
// store holds WHICH daemon the renderer is currently talking to and the
// address book of saved hosts.
//
// ## What lives here now (step #1/#2)
//   - `activeHost`: 'local' | ConnectHost  (default 'local')
//   - `hosts`: ConnectHost[]               (the saved address book)
//   - selectHost / addHost / removeHost mutators
//   - localStorage persistence of the host list MINUS the token.
//
// ## Persistence (step #3 — landed)
//   - The NON-SECRET host list persists to `~/.k2so/connect-hosts.json`
//     via the `connect_hosts_{read,write}` Tauri commands (localStorage
//     is kept as a synchronous boot cache / non-Tauri fallback).
//   - REMEMBERED tokens persist to the OS keychain via
//     `k2_secret_{set,get,delete}` (service `K2_CONNECT_KEYCHAIN_SERVICE`,
//     account = host.id). Non-remembered tokens stay in memory only.
//   - `hydrateFromDisk()` runs once at boot: loads the file then resolves
//     each remembered host's token from the keychain. Call it from the
//     app shell.
//
// ## What is deliberately deferred
//   - Remote AcceptancePolicy / soft-reconnect / latency readout
//     (steps #4/#5) — partially landed in the ConnectionGate work.
//
// ## Local is byte-identical
// When `activeHost === 'local'`, daemon-ws.ts resolves the exact same
// Tauri `daemon_ws_url` path it always has. This store is purely
// additive — a fresh install with no saved hosts behaves exactly as
// today.

import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { withRemoteRetry } from '@/lib/remote-retry'
import type { RemoteRecoveryState } from '@/lib/remote-recovery'
import { isWebClient } from '@/lib/is-web'
import {
  logoutWebSession,
  persistWebSessionToken,
  withDaemonFetch,
} from '@/web/session-token'

/**
 * Keychain service name for remembered remote-host tokens. Each token is
 * stored under `(K2_CONNECT_KEYCHAIN_SERVICE, host.id)` via the
 * `k2_secret_*` Tauri commands. Exported so the Settings → Connections UI
 * can store/forget tokens with the SAME key the store reads on boot.
 */
export const K2_CONNECT_KEYCHAIN_SERVICE = 'dev.k2.connect.host-token'
/** Pre-0.40 service name — read fallback only, copied forward on first read. */
const LEGACY_K2_CONNECT_KEYCHAIN_SERVICE = 'com.k2so.connect.host-token'

/**
 * Keychain service name for a remembered remote-host PASSWORD. Distinct
 * from the session-token service above: the password is the long-lived
 * credential the user types once; the token is the short-lived session
 * bearer obtained from `POST /cli/auth/login`. When "remember password"
 * is on we persist the password here (keyed by host id) so connect-time
 * auto-login needs no prompt; the session token still lives under
 * {@link K2_CONNECT_KEYCHAIN_SERVICE}. Neither ever touches
 * connect-hosts.json.
 */
export const K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE = 'dev.k2.connect.host-password'
/** Pre-0.40 service name — read fallback only, copied forward on first read. */
const LEGACY_K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE = 'com.k2so.connect.host-password'

/**
 * A saved K2 server the client can connect to. Mirrors the Phase 3 PRD
 * §E.3 schema (with the §3 correction that the token does NOT live in
 * the persisted JSON — it goes to the keychain in step #3; for now it's
 * memory-only).
 */
export interface ConnectHost {
  /** Stable client-generated id (used as the dropdown key + removeHost arg). */
  id: string
  /** User-facing name shown in the switcher (e.g. "Hetzner box"). */
  label: string
  /** Hostname or IP of the daemon (no scheme, no port). */
  hostname: string
  /**
   * Account username for `POST /cli/auth/login` (connect-users, #617).
   * NON-SECRET — persists to connect-hosts.json alongside hostname/port.
   * Optional for backward-compat with hosts saved before connect-users
   * (those used a raw host token and have no username); a remote added
   * via the connect-users flow always has one. The local host never has
   * a username (it uses the daemon token).
   */
  username?: string
  /** Daemon port. For a secure hosted remote this is typically 443
   *  (and the port is omitted from the built URL). */
  port: number
  /**
   * TLS (K2 Connect step #4). When true, daemon-ws.ts builds
   * `https://`/`wss://` URLs and omits port 443 — for a hosted tunnel
   * like `rosson.k2.dev` where Caddy terminates TLS. Defaults to true
   * for non-local hostnames added via the switcher; false for
   * localhost/LAN direct-IP. Non-secret, so it persists to localStorage.
   */
  secure: boolean
  /**
   * SESSION token (rides as `?token=`). SECRET — never persisted to
   * localStorage or connect-hosts.json. Held in memory for the session.
   *
   * connect-users (#617): for a connect-users host this is the bearer
   * returned by `POST /cli/auth/login {username,password}` → it is NOT
   * the user's password. It's obtained at connect time (see
   * {@link loginToHost}) and cached in the keychain under
   * {@link K2_CONNECT_KEYCHAIN_SERVICE} via {@link rememberToken}.
   * A 401 from any `/cli/*` call means it expired → drop + re-login.
   *
   * Legacy hosts (no username) still carry a raw host token here.
   */
  token: string
  /**
   * "Remember password" toggle. When true the user's PASSWORD persists
   * to the keychain (under {@link K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE})
   * so connect-time auto-login needs no prompt; the session token is
   * also cached. When false, the password is entered each connect via
   * RemoteSignIn and never persisted.
   */
  remember: boolean
  /** Epoch ms of the last successful connect, or null if never. */
  lastConnectedAt: number | null
}

/** The non-secret shape we persist to localStorage (everything except
 *  `token`). */
type PersistedHost = Omit<ConnectHost, 'token'>

/** A loopback / LAN-localhost hostname speaks plain HTTP — never TLS.
 *  Used to pick the `secure` default when adding a host: non-local
 *  hostnames (e.g. `rosson.k2.dev`) default to secure (TLS via the
 *  tunnel), localhost/127.0.0.1/::1 default to plain. */
export function isLocalHostname(hostname: string): boolean {
  const h = hostname.trim().toLowerCase()
  return (
    h === 'localhost' ||
    h === '127.0.0.1' ||
    h === '::1' ||
    h === '[::1]' ||
    h === '0.0.0.0'
  )
}

export type ActiveHost = 'local' | ConnectHost

/**
 * A stable string identity for the active host — `'local'` for This Mac,
 * or `${id}:${hostname}:${port}` for a remote. Changes IFF the user
 * switches daemons. ConnectionGate keys the `<App>` remount on this, and
 * per-machine UI session stores (tabs.ts workspace sessions, ActiveBar
 * memory maps, active-agents pane state) subscribe to detect a host
 * CHANGE and reset their local-ID-keyed state so a remote host never
 * shows the local machine's workspaces (#625).
 *
 * Lives here (not ConnectionGate) so the session stores can reuse it
 * WITHOUT importing a React component — keeping the reset wiring free of
 * an import cycle (connect-host imports nothing app-side).
 */
export function activeHostKey(active: ActiveHost): string {
  return active === 'local' ? 'local' : `${active.id}:${active.hostname}:${active.port}`
}

/** Whether the active host carries a usable session: 'local' always does
 *  (per-boot daemon token, resolved by daemon-ws); a remote does IFF it
 *  holds a non-empty session token. `expireSession` flips an active
 *  remote to tokenless; a sign-in mint flips it back. */
function activeHostAuthed(active: ActiveHost): boolean {
  return active === 'local' || active.token.length > 0
}

/**
 * Subscribe to ACTIVE-HOST CHANGES (not every store mutation). `onChange`
 * fires when {@link activeHostKey} differs from the prior value — never on
 * the initial subscribe for the boot 'local' host, and never for unrelated
 * mutations (token refresh, host-list edits) that leave the active host
 * identity unchanged. Returns the zustand unsubscribe fn.
 *
 * It ALSO fires, with `nextKey === prevKey`, when the ACTIVE host's session
 * is MINTED — its token transitions empty → non-empty (RemoteSignIn /
 * loginToHost after `expireSession` dropped it). The host-switch fetch
 * burst every subscriber runs fires at `selectHost` time, when the new
 * host's session may be dead (a remote restart wiped it); those fetches
 * fail terminally with 'signin-required' and NOTHING else replays them —
 * the later sign-in re-activates the SAME host, so a key-only rule never
 * re-fires and every host-scoped store keeps the PREVIOUS host's data
 * while the top bar says the new host is connected (the switch-while-in-
 * Settings dashboard desync). A mint is the recovery point: replay the
 * burst. A token REFRESH (non-empty → non-empty, the revival path) does
 * NOT fire — daemon-cli's revive-and-replay already completes those
 * requests, and a mid-session full reset would churn live surfaces.
 *
 * This is the single seam the per-machine session stores hook to clear
 * their local-ID-keyed state on a host switch (#625). It runs AFTER
 * `activeHost` has already flipped, so any reload the callback triggers
 * (e.g. `loadWorkspaceSessionsFromDb`) targets the NEW host via the
 * host-aware daemon-cli layer.
 */
export function onActiveHostChange(
  onChange: (nextKey: string, prevKey: string) => void,
): () => void {
  let lastKey = activeHostKey(useConnectHostStore.getState().activeHost)
  let lastAuthed = activeHostAuthed(useConnectHostStore.getState().activeHost)
  return useConnectHostStore.subscribe((state) => {
    const nextKey = activeHostKey(state.activeHost)
    const nextAuthed = activeHostAuthed(state.activeHost)
    const keyChanged = nextKey !== lastKey
    // Same host, session minted (tokenless → token): the switch-time fetch
    // burst died against the dead session — replay it now that it can
    // succeed. The authed → UNAUTHED transition (expireSession) only
    // records state: firing there would launch a burst guaranteed to fail.
    const sessionMinted = !keyChanged && !lastAuthed && nextAuthed
    lastAuthed = nextAuthed
    if (!keyChanged && !sessionMinted) return
    const prevKey = lastKey
    lastKey = nextKey
    onChange(nextKey, prevKey)
  })
}

/**
 * Live connection state for the ACTIVE host, driven by ConnectionGate's
 * boot-status poll. The switcher renders a status dot from this.
 *   - 'connecting' → gate is polling / app not yet mounted (or switching)
 *   - 'connected'  → gate accepted; app mounted against this host
 *   - 'offline'    → reserved seam (step #5 latency layer flips this on
 *     repeated poll failure; for now the gate only sets connecting/connected)
 */
export type ConnectionStatus = 'connecting' | 'connected' | 'offline'

export interface ConnectHostState {
  activeHost: ActiveHost
  hosts: ConnectHost[]
  /** Live status of the active host's connection (set by the gate). */
  connectionStatus: ConnectionStatus
  /**
   * The ACTIVE remote host's recovery state machine (owner contract — see
   * lib/remote-recovery.ts): 'connected' baseline, or exactly one of
   * 'reconnecting' (host down / still booting — never a login prompt) /
   * 'reauthenticating' (silent re-login underway) / 'signin-required' (the
   * one state that needs the user). Driven by ConnectionGate's health poll
   * + lib/remote-session's revival; rendered by the gate's banner, the
   * top-bar host indicator, and the Connections panel. Meaningless while
   * `activeHost === 'local'` (stays 'connected').
   */
  recovery: RemoteRecoveryState
  /**
   * Remote-capability cache (#638): the ACTIVE host's marketing version +
   * wire protocol, learned from its `/boot-status` when the ConnectionGate
   * accepts it. `lib/server-capabilities.ts` reads `serverVersion` to gate
   * newer client features (and to build "update the host to vX" hints).
   *
   * null until a boot-status lands, and reset to null on every host switch
   * / disconnect so a stale version never leaks across hosts. For the
   * LOCAL host this is left null and treated as "supports everything" by
   * serverSupports() (local is always paired with this app).
   */
  serverVersion: string | null
  serverProtocol: number | null
  /**
   * A host the user picked that needs the full-screen sign-in before it
   * can become active — i.e. it has no remembered/in-memory token, or a
   * previously-remembered one was rejected/expired. Null = no sign-in
   * pending. The sign-in overlay (mounted by ConnectionGate) reads this;
   * a remembered host with a resolved token bypasses it (silent auto
   * sign-in via `selectHost`). */
  pendingSignIn: ConnectHost | null
  /** When the sign-in overlay succeeds, should it SWITCH the active daemon to
   *  the signed-in host (true — switcher / auto-sign-in), or just cache the
   *  session token and close, staying on the current view (false — tile
   *  "Sign in" for management)? `requestSignIn` sets true; `signInForManagement`
   *  sets false. */
  signInActivate: boolean
  /** Switch the active daemon. Pass 'local' for This Mac, or a saved
   *  ConnectHost. Sets `connectionStatus:'connecting'` then flips
   *  `activeHost` synchronously. The renderer routes daemon-data through
   *  host-aware `daemonCli*` calls that read `activeHost` from this store
   *  at call time, so there is no Rust-side override to await. */
  selectHost: (hostOrLocal: ActiveHost) => void
  /** Add (or replace by id) a host in the address book. */
  addHost: (host: ConnectHost) => void
  /** Remove a host by id. If it's the active host, fall back to 'local'.
   *  Also forgets any keychain token for that host. */
  removeHost: (id: string) => void
  /** Gate-driven: update the live connection status of the active host. */
  setConnectionStatus: (status: ConnectionStatus) => void
  /** Commit the active remote's recovery state (gate poll / revival). */
  setRecovery: (recovery: RemoteRecoveryState) => void
  /**
   * Gate-driven (#638): cache the ACTIVE host's version + protocol from its
   * accepted `/boot-status`. Pass `{version:null, protocol:null}` to clear
   * (the gate calls this on a host switch before re-polling). Called from
   * ConnectionGate's accept path.
   */
  setServerInfo: (info: { version: string | null; protocol: number | null }) => void
  /** Set a host's in-memory token (e.g. after a successful sign-in) so
   *  daemon-ws.ts can use it for the active connection. Does NOT touch
   *  the keychain — call `rememberToken`/`forgetToken` for that. When the
   *  updated host is the active remote, the `activeHost` object is swapped
   *  synchronously so host-aware `daemonCli*` calls pick up the new
   *  token on their next read. */
  setHostToken: (id: string, token: string) => void
  /** Boot-time hydration: load the durable host list from
   *  ~/.k2so/connect-hosts.json, then resolve each REMEMBERED host's
   *  token from the keychain into memory. Idempotent; call once from the
   *  app shell. No-op (resolves) outside Tauri. */
  hydrateFromDisk: () => Promise<void>
  /** Open the full-screen sign-in for `host` (no/expired token). */
  requestSignIn: (host: ConnectHost) => void
  /** Open the sign-in overlay to cache a host's session token for tile-based
   *  management WITHOUT switching the active daemon (stays on the current
   *  view). On success the overlay just closes. */
  signInForManagement: (host: ConnectHost) => void
  /**
   * connect-users (#617) session expiry: a `/cli/*` call to a remote host
   * returned 401, so its cached session token is stale. Drop the
   * in-memory + keychain session token (NOT the remembered password) and
   * re-trigger the full-screen sign-in for that host. No-op for 'local'
   * or a host id that isn't the active remote (a stale 401 from a prior
   * host must not interrupt the current one). */
  expireSession: (hostId: string) => void
  /** Clear a SPECIFIC host's cached session token (in-memory + keychain),
   *  regardless of whether it's the active host. Unlike `expireSession` (which
   *  only fires for the active remote and raises the full-screen overlay), this
   *  is for the Connections TILES: when a per-host op is rejected as
   *  unauthorized, the tile clears the dead token so it flips to signed-out and
   *  shows its inline Sign-in button. Keeps the remembered password. No-op if
   *  the host already has no token. */
  clearHostToken: (hostId: string) => void
  /** Dismiss the full-screen sign-in without switching. */
  cancelSignIn: () => void
  /**
   * Pick a host the user selected: if it already has a usable in-memory
   * token, switch to it directly (silent). Otherwise open the full-screen
   * sign-in. 'local' always switches directly (never needs auth). This is
   * the single entry point the switcher + auto-sign-in both call. */
  pickHost: (hostOrLocal: ActiveHost) => void
}

const STORAGE_KEY = 'k2so.connect-hosts.v1'

/** Best-effort access to localStorage. Returns null in non-browser
 *  (vitest node) contexts so the store still works headless. */
function getStorage(): Storage | null {
  try {
    if (typeof localStorage !== 'undefined') return localStorage
  } catch {
    /* SecurityError in some sandboxes — treat as unavailable */
  }
  return null
}

/** Load the persisted (token-less) host list. Tokens are NOT persisted,
 *  so loaded hosts start with an empty token + remember preserved. */
function loadHosts(): ConnectHost[] {
  const storage = getStorage()
  if (!storage) return []
  const raw = storage.getItem(STORAGE_KEY)
  if (!raw) return []
  try {
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed
      .filter((h): h is PersistedHost => isPersistedHost(h))
      // `secure` may be absent in entries persisted before step #4 —
      // default to false (plain http/ws) so old saved hosts keep their
      // prior behaviour. Token is never persisted; starts empty.
      .map((h) => ({ ...h, secure: h.secure ?? false, token: '' }))
  } catch {
    return []
  }
}

function isPersistedHost(h: unknown): h is PersistedHost {
  if (typeof h !== 'object' || h === null) return false
  const o = h as Record<string, unknown>
  return (
    typeof o.id === 'string' &&
    typeof o.label === 'string' &&
    typeof o.hostname === 'string' &&
    typeof o.port === 'number' &&
    // `username` optional (connect-users #617; absent on legacy raw-token
    // hosts). Reject only an explicitly wrong type.
    (o.username === undefined || typeof o.username === 'string') &&
    typeof o.remember === 'boolean' &&
    // `secure` optional for backward-compat (loadHosts defaults it to
    // false); reject only an explicitly wrong type.
    (o.secure === undefined || typeof o.secure === 'boolean') &&
    (o.lastConnectedAt === null || typeof o.lastConnectedAt === 'number')
  )
}

/** Persist the host list MINUS the token. This is the security
 *  invariant: secrets never touch localStorage OR connect-hosts.json.
 *
 *  Two backends:
 *    1. localStorage — synchronous, so the next boot has the list
 *       immediately (the switcher renders before async hydration lands).
 *    2. `~/.k2so/connect-hosts.json` via the `connect_hosts_write` Tauri
 *       command — the durable, daemon/CLI-visible source of truth. Fire-
 *       and-forget; a failure leaves localStorage as the fallback. */
function persistHosts(hosts: ConnectHost[]): void {
  const sansToken: PersistedHost[] = hosts.map(({ token: _token, ...rest }) => rest)
  const json = JSON.stringify(sansToken)

  const storage = getStorage()
  if (storage) {
    try {
      storage.setItem(STORAGE_KEY, json)
    } catch {
      /* quota / disabled storage — non-fatal, hosts stay in memory + file */
    }
  }

  // Durable file write (Tauri only). Never blocks the caller.
  void invoke('connect_hosts_write', { json }).catch(() => {
    /* non-Tauri (vitest/web) or IO failure — localStorage covers boot */
  })
}

// ── Keychain helpers (remembered-token persistence) ─────────────────────
// Tokens for "Remember password" hosts live in the OS keychain, NEVER in
// localStorage or connect-hosts.json. Keyed by host id. All three are
// best-effort + Tauri-only; they no-op (resolve) in the vitest/web env.

/** Store a host's token in the keychain under its id. */
export async function rememberToken(hostId: string, token: string): Promise<void> {
  try {
    await invoke('k2_secret_set', {
      service: K2_CONNECT_KEYCHAIN_SERVICE,
      account: hostId,
      secret: token,
    })
  } catch {
    /* keychain unavailable — caller keeps the token in memory */
  }
}

/** Resolve a host's remembered token from the keychain, or null. */
export async function resolveToken(hostId: string): Promise<string | null> {
  try {
    const secret = await invoke<string | null>('k2_secret_get', {
      service: K2_CONNECT_KEYCHAIN_SERVICE,
      account: hostId,
    })
    if (secret) return secret
    // 0.40.0 rebrand: migrate a pre-rename entry forward on first read.
    const legacy = await invoke<string | null>('k2_secret_get', {
      service: LEGACY_K2_CONNECT_KEYCHAIN_SERVICE,
      account: hostId,
    })
    if (legacy) await rememberToken(hostId, legacy)
    return legacy ?? null
  } catch {
    return null
  }
}

/** Forget a host's remembered token (toggle-off / host removal). */
export async function forgetToken(hostId: string): Promise<void> {
  for (const service of [K2_CONNECT_KEYCHAIN_SERVICE, LEGACY_K2_CONNECT_KEYCHAIN_SERVICE]) {
    try {
      await invoke('k2_secret_delete', { service, account: hostId })
    } catch {
      /* idempotent — a missing entry is fine */
    }
  }
}

// ── Keychain helpers (remembered-PASSWORD persistence) ──────────────────
// connect-users (#617): when "remember password" is on we cache the
// user's login password (NOT the session token) so connect-time
// auto-login needs no prompt. Same Tauri-only / best-effort contract as
// the token helpers, under a DISTINCT service so a host can have a
// remembered password and a separate cached session token.

/** Store a host's login password in the keychain under its id. */
export async function rememberPassword(hostId: string, password: string): Promise<void> {
  try {
    await invoke('k2_secret_set', {
      service: K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
      account: hostId,
      secret: password,
    })
  } catch {
    /* keychain unavailable — user re-enters the password next connect */
  }
}

/** Resolve a host's remembered login password from the keychain, or null. */
export async function resolvePassword(hostId: string): Promise<string | null> {
  try {
    const secret = await invoke<string | null>('k2_secret_get', {
      service: K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
      account: hostId,
    })
    if (secret) return secret
    // 0.40.0 rebrand: migrate a pre-rename entry forward on first read.
    const legacy = await invoke<string | null>('k2_secret_get', {
      service: LEGACY_K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
      account: hostId,
    })
    if (legacy) await rememberPassword(hostId, legacy)
    return legacy ?? null
  } catch {
    return null
  }
}

/** Forget a host's remembered login password (toggle-off / host removal). */
export async function forgetPassword(hostId: string): Promise<void> {
  for (const service of [K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE, LEGACY_K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE]) {
    try {
      await invoke('k2_secret_delete', { service, account: hostId })
    } catch {
      /* idempotent — a missing entry is fine */
    }
  }
}

// ── Login (connect-users #617) ───────────────────────────────────────────

/** Build `<scheme>://<host>[:<port>]` for a host, matching daemon-ws.ts:
 *  secure+443 omits the port; everything else carries it. */
function hostBaseUrl(host: Pick<ConnectHost, 'hostname' | 'port' | 'secure'>): string {
  const scheme = host.secure ? 'https' : 'http'
  const authority = host.secure && host.port === 443 ? host.hostname : `${host.hostname}:${host.port}`
  return `${scheme}://${authority}`
}

/** Result of {@link loginToHost}. On success the session token is also
 *  committed into the store (setHostToken) so daemon-ws.ts uses it.
 *
 *  Failure `kind` lets programmatic callers (runtime session revival,
 *  lib/remote-session.ts) branch without matching user-facing copy:
 *    - 'auth'        → the daemon REJECTED the credentials (401) — retrying
 *                      with the same password can never succeed.
 *    - 'unreachable' → connection-level failure after every retry — the
 *                      credentials were never evaluated.
 *    - 'server'      → the endpoint answered but isn't a usable login
 *                      (non-2xx, malformed body, missing username). */
export type LoginResult =
  | { ok: true; token: string }
  | { ok: false; kind: 'auth' | 'unreachable' | 'server'; reason: string }

/** Daemon `POST /cli/auth/login` success body. */
interface LoginResponse {
  token: string
  username: string
  // ISO-8601 string from the daemon (e.g. "2026-07-20T20:38:56+00:00"),
  // NOT an epoch number — parse with Date.parse() if you ever do date math.
  expiresAt: string
}

/**
 * Exchange `{username, password}` for a session token via
 * `POST <host>/cli/auth/login` (connect-users #617). On 200 the returned
 * session token is committed to the host (setHostToken) and the host's
 * lastConnectedAt is stamped, then cached to the keychain via
 * rememberToken so daemon-ws.ts / a fresh boot can reuse it. A 401 (or
 * any non-2xx) surfaces a friendly reason WITHOUT mutating state.
 *
 * Hosted web (`VITE_WEB`): also sends `web: true` + `X-K2-Client: web` +
 * `credentials: 'include'` so the daemon sets the HttpOnly `k2_session`
 * cookie (PRD §2.3). The JSON `token` is still stored in memory for
 * pragmatic WebSocket query auth.
 *
 * The PASSWORD is NOT persisted here — the caller (Add-server /
 * RemoteSignIn) decides whether to rememberPassword based on the
 * "remember" toggle. This helper only deals in the session token.
 */
export async function loginToHost(
  host: ConnectHost,
  password: string,
  timeoutMs = 8000,
): Promise<LoginResult> {
  const username = host.username?.trim() ?? ''
  if (!username) {
    return { ok: false, kind: 'server', reason: 'This server has no username configured.' }
  }
  const url = `${hostBaseUrl(host)}/cli/auth/login`
  // Issue #5: every *.k2.dev host shares ONE relay IP, so the webview pools a
  // single connection per origin. After relay churn (a remote update / E2E
  // cert re-provision) that pooled socket can go stale — cached GETs limp
  // through, but a fresh login POST reuses the dead socket and throws at the
  // network layer. The throw evicts the bad connection from the pool, so a
  // brief pause + retry opens a FRESH socket — recovering without the full app
  // relaunch that was previously the only fix. The shared withRemoteRetry
  // backoff (immediate first retry to evict+reopen, then widening pauses)
  // rides out the restart window; only surface an error if every attempt fails
  // (then it's likely a real connectivity/server problem). The per-attempt
  // AbortSignal.timeout stays inside the op so each attempt is freshly bounded.
  const web = isWebClient()
  // Daemon sets Set-Cookie: k2_session=… when web:true OR X-K2-Client: web.
  const loginBody: Record<string, unknown> = web
    ? { username, password, web: true }
    : { username, password }
  let resp: Response | undefined
  try {
    resp = await withRemoteRetry(() =>
      fetch(
        url,
        withDaemonFetch({
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(loginBody),
          signal: AbortSignal.timeout(timeoutMs),
        }),
      ),
    )
  } catch {
    /* every attempt threw a connection-level error — handled below */
  }
  if (!resp) {
    return {
      ok: false,
      kind: 'unreachable',
      reason: `Couldn't establish a connection to ${host.hostname}. The server may be reachable — try again, and if it keeps failing, quit and relaunch K2.`,
    }
  }
  if (resp.status === 401) {
    return { ok: false, kind: 'auth', reason: 'Invalid username or password.' }
  }
  if (!resp.ok) {
    return { ok: false, kind: 'server', reason: `Server returned ${resp.status}. It may not be a K2 server.` }
  }
  let body: LoginResponse
  try {
    body = (await resp.json()) as LoginResponse
  } catch {
    return { ok: false, kind: 'server', reason: 'Server response was not a valid login.' }
  }
  if (typeof body.token !== 'string' || body.token.length === 0) {
    return { ok: false, kind: 'server', reason: 'Server did not return a session token.' }
  }
  // Commit the session token into the store + stamp lastConnectedAt so
  // daemon-ws.ts (which reads activeHost.token) picks it up, and cache it
  // to the keychain for a silent next connect.
  const store = useConnectHostStore.getState()
  store.setHostToken(host.id, body.token)
  const existing = store.hosts.find((h) => h.id === host.id)
  if (existing) {
    store.addHost({ ...existing, token: body.token, lastConnectedAt: Date.now() })
  }
  await rememberToken(host.id, body.token)
  return { ok: true, token: body.token }
}

export const useConnectHostStore = create<ConnectHostState>((set, get) => ({
  activeHost: 'local',
  hosts: loadHosts(),
  connectionStatus: 'connecting',
  recovery: { kind: 'connected' },
  pendingSignIn: null,
  signInActivate: true,
  serverVersion: null,
  serverProtocol: null,

  selectHost: (hostOrLocal) => {
    // Hosted web client: never activate the Tauri local-daemon path
    // (`daemon_ws_url`). Boot seeds a same-origin ConnectHost; ignore
    // any attempt to switch to 'local'.
    if (isWebClient() && hostOrLocal === 'local') {
      return
    }
    // Enter 'connecting' + clear any pending sign-in, then flip
    // `activeHost`. The gate flips to 'connected' once the new host
    // accepts. Daemon-data flows through host-aware `daemonCli*` calls
    // that read `activeHost` from this store at call time, so the flip is
    // synchronous — there is no Rust-side proxy override to await.
    //
    // #638: a host switch invalidates the cached remote version/protocol —
    // null them so server-capabilities never reports the OLD host's
    // capabilities for the NEW one. The gate re-caches on the next accept.
    set({
      connectionStatus: 'connecting',
      pendingSignIn: null,
      activeHost: hostOrLocal,
      serverVersion: null,
      serverProtocol: null,
      // A host switch starts from the healthy baseline — the previous
      // host's recovery state must never bleed onto the new one.
      recovery: { kind: 'connected' },
    })
  },

  requestSignIn: (host) => {
    // Switcher / auto-sign-in path: on success, SWITCH the active daemon to
    // this host (the default).
    set({ pendingSignIn: host, signInActivate: true })
  },

  signInForManagement: (host) => {
    // Tile "Sign in" path: authenticate to cache this host's session token so
    // its owner-gated tile controls (Restart / Check-for-updates / federation
    // toggle) work — but DON'T switch the active daemon. The overlay closes on
    // success (no switch), so the user stays on the local K2 Connect view.
    set({ pendingSignIn: host, signInActivate: false })
  },

  expireSession: (hostId) => {
    const { activeHost, hosts } = get()
    // Only act when the EXPIRED host is the active remote — a late 401
    // from a host we already switched away from must not hijack the UI.
    if (activeHost === 'local' || activeHost.id !== hostId) return
    // Drop the dead session token in memory + keychain (keep the
    // remembered password so RemoteSignIn can offer auto-login).
    const cleared = hosts.map((h) => (h.id === hostId ? { ...h, token: '' } : h))
    const clearedActive: ActiveHost = { ...activeHost, token: '' }
    void forgetToken(hostId)
    // Web: revoke server-side session + clear k2_session cookie, and drop
    // the tab-session bearer so reload re-prompts login.
    if (isWebClient()) {
      void logoutWebSession(hostBaseUrl(activeHost))
    } else {
      persistWebSessionToken('')
    }
    // Reflect the dropped token in the host list, raise the sign-in
    // overlay, and flip `activeHost` to the tokenless host — all
    // synchronously. Host-aware `daemonCli*` calls read `activeHost.token`
    // at call time, so a tokenless active host simply can't fire authed
    // remote requests until the user re-signs-in.
    set({
      hosts: cleared,
      pendingSignIn: clearedActive,
      connectionStatus: 'connecting',
      activeHost: clearedActive,
      // #638: session expired → drop the cached capability info until the
      // re-authed host re-accepts and the gate re-caches it.
      serverVersion: null,
      serverProtocol: null,
      // Recovery contract: expiry means automatic re-auth is off the table
      // (no/rejected stored credentials) — the ONE state that needs the user.
      recovery: { kind: 'signin-required' },
    })
  },

  clearHostToken: (hostId) => {
    const { hosts } = get()
    // No-op if it's already tokenless — avoids a needless re-render/keychain hit.
    if (!hosts.some((h) => h.id === hostId && h.token)) return
    const cleared = hosts.map((h) => (h.id === hostId ? { ...h, token: '' } : h))
    void forgetToken(hostId)
    set({ hosts: cleared })
  },

  cancelSignIn: () => {
    set({ pendingSignIn: null })
  },

  pickHost: (hostOrLocal) => {
    if (hostOrLocal === 'local') {
      // Web never has a local daemon — selectHost no-ops; keep current.
      if (isWebClient()) return
      get().selectHost('local')
      return
    }
    // A usable in-memory session token (resolved from the keychain on
    // boot, or set during a prior login this session) → switch silently.
    if (hostOrLocal.token && hostOrLocal.token.length > 0) {
      get().selectHost(hostOrLocal)
      return
    }
    // connect-users (#617): no live session token, but if the user
    // remembered their PASSWORD we can auto-login without prompting.
    // Only applies to connect-users hosts (those have a username).
    if (hostOrLocal.username && hostOrLocal.username.length > 0) {
      void resolvePassword(hostOrLocal.id).then(async (pw) => {
        if (pw) {
          const result = await loginToHost(hostOrLocal, pw)
          if (result.ok) {
            // loginToHost committed the session token; re-read the host so
            // selectHost carries it, then switch silently.
            const refreshed = get().hosts.find((h) => h.id === hostOrLocal.id)
            get().selectHost(refreshed ?? { ...hostOrLocal, token: result.token })
            return
          }
        }
        // No remembered password, or it was rejected/expired → prompt.
        get().requestSignIn(hostOrLocal)
      })
      return
    }
    // Legacy raw-token host with no token → full-screen sign-in.
    get().requestSignIn(hostOrLocal)
  },

  setConnectionStatus: (status) => {
    set({ connectionStatus: status })
  },

  setRecovery: (recovery) => {
    // No-op on an identical state: the gate re-commits on every health tick
    // (~4s), and a fresh-but-equal object would re-render every subscriber.
    const prev = get().recovery
    if (
      prev.kind === recovery.kind &&
      (prev.kind !== 'reconnecting' ||
        recovery.kind !== 'reconnecting' ||
        prev.bootPhase === recovery.bootPhase)
    ) {
      return
    }
    set({ recovery })
  },

  setServerInfo: ({ version, protocol }) => {
    set({ serverVersion: version, serverProtocol: protocol })
  },

  addHost: (host) => {
    const existing = get().hosts
    // Replace-by-id so re-adding/editing an existing entry updates in
    // place rather than duplicating.
    const without = existing.filter((h) => h.id !== host.id)
    const next = [...without, host]
    persistHosts(next)
    set({ hosts: next })
  },

  removeHost: (id) => {
    const { hosts, activeHost } = get()
    const next = hosts.filter((h) => h.id !== id)
    persistHosts(next)
    // Drop any remembered session token AND password from the keychain
    // too (best-effort).
    void forgetToken(id)
    void forgetPassword(id)
    // If we removed the currently-active host, fall back to local.
    // Web: never fall back to 'local' (no Tauri daemon_ws_url). Keep the
    // current active host object even if it was removed from the book —
    // boot-host re-seeds same-origin on next load.
    const removedActive = activeHost !== 'local' && activeHost.id === id
    const nextActive: ActiveHost =
      removedActive && !isWebClient() ? 'local' : activeHost
    // #638: if the removed host was active, we fell back to 'local' — drop
    // the removed host's cached capability info.
    set(
      removedActive && !isWebClient()
        ? { hosts: next, activeHost: nextActive, serverVersion: null, serverProtocol: null }
        : { hosts: next, activeHost: nextActive },
    )
  },

  setHostToken: (id, token) => {
    const { hosts, activeHost } = get()
    const hosts2 = hosts.map((h) => (h.id === id ? { ...h, token } : h))
    // Keep the active host object in sync if it's the one being updated,
    // so daemon-ws.ts + host-aware `daemonCli*` calls (which read
    // activeHost.token) see the new token. All synchronous — there is no
    // Rust-side proxy override to refresh.
    const active2: ActiveHost =
      activeHost !== 'local' && activeHost.id === id
        ? { ...activeHost, token }
        : activeHost
    set({ hosts: hosts2, activeHost: active2 })
    // Web: mirror the bearer into sessionStorage so a reload can still
    // open WebSockets with ?token= (pragmatic; HTTP uses the k2_session
    // cookie). Cookie is set by login's Set-Cookie — not writable from JS.
    if (
      isWebClient() &&
      activeHost !== 'local' &&
      activeHost.id === id
    ) {
      persistWebSessionToken(token)
    }
  },

  hydrateFromDisk: async () => {
    // Hosted web: address book + active host are seeded by boot-host.ts
    // from window.location + session/local storage. The Tauri
    // connect_hosts_read shim returns `[]`, which would wipe that seed —
    // so skip disk/keychain hydration entirely on web.
    if (isWebClient()) return

    // 1. Load the durable host list. Falls back to whatever the
    //    constructor already loaded from localStorage on failure.
    let fileHosts: ConnectHost[] | null = null
    try {
      const raw = await invoke<string>('connect_hosts_read')
      const parsed = JSON.parse(raw) as unknown
      if (Array.isArray(parsed)) {
        fileHosts = parsed
          .filter((h): h is PersistedHost => isPersistedHost(h))
          .map((h) => ({ ...h, secure: h.secure ?? false, token: '' }))
      }
    } catch {
      /* non-Tauri / read failure — keep the localStorage-seeded list */
    }

    const base = fileHosts ?? get().hosts

    // 2. Resolve remembered tokens from the keychain into memory.
    const resolved = await Promise.all(
      base.map(async (h) => {
        if (!h.remember) return h
        const token = await resolveToken(h.id)
        return token ? { ...h, token } : h
      }),
    )

    // Mirror the resolved list back to localStorage (token-stripped) so a
    // subsequent synchronous boot has the file's view.
    persistHosts(resolved)
    set({ hosts: resolved })
  },
}))

/** Test-only: reset the store to defaults and clear persisted hosts.
 *  Mirrors the `__reset*ForTests` hooks other stores expose. */
export function __resetConnectHostStoreForTests(): void {
  const storage = getStorage()
  storage?.removeItem(STORAGE_KEY)
  useConnectHostStore.setState({ activeHost: 'local', hosts: [], connectionStatus: 'connecting', recovery: { kind: 'connected' }, pendingSignIn: null, serverVersion: null, serverProtocol: null })
}

/** The localStorage key, exported for tests. */
export const CONNECT_HOSTS_STORAGE_KEY = STORAGE_KEY
