// Settings → K2 Connect — the HOST / expose side (PRD §6, build order
// MVP). The OTHER direction from Connections (§1–5, the client connecting
// OUT): "K2 Connect" is where THIS device exposes its OWN daemon at a
// subdomain it owns under k2.dev via the frpc reverse tunnel.
//
// MVP (buildable now, no control-plane changes):
//   - tunnel config form: token + subdomain + an optional relay address
//     (Advanced; the daemon owns the default, relay.k2.dev since 0.46.6) →
//     Save (POST /cli/tunnel/config; the token is redacted on read →
//     tokenSet:bool, never echoed back). The relay address is sent only
//     when the person edited it, never on a subdomain bind.
//   - Start / Stop + live status (GET /cli/tunnel/status) + public URL
//     https://<sub>.k2.dev + a "frpc not installed" hint surfaced from the
//     connector's error.
//
// ACCOUNT FLOW (live): k2.dev account sign-in (Supabase Auth, email +
//   password) → list the subdomains the user owns → pick one → it auto-
//   fills + saves the tunnel config to the daemon. The refresh token is
//   persisted to the OS keychain (account service, distinct from the host-
//   token service); the access token stays in memory. See lib/k2-account.ts.
//   Per PRD §6.2 the subdomains are the $2.99/mo purchased ones. The manual
//   token + subdomain form remains behind an "Advanced / manual" toggle as
//   a fallback.
//
// OUT OF SCOPE here: the daemon "Users / Access" multi-user feature
// (task #617) — that's the daemon-owned access list (PRD §6.2), a
// separate area from this account/expose page.

import React, { useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { getDaemonWs, getLocalDaemonWs, daemonHttpBase, invalidateDaemonWs } from '@/kessel/daemon-ws'
import { withRemoteRetry } from '@/lib/remote-retry'
import type { SettingEntry } from '../searchManifest'
import { SettingRow, SettingsGroup, SettingDropdown } from '../controls/SettingControls'
import {
  ROLE_OPTIONS,
  needsEnableAsMember,
  parseK2Role,
  type K2Role,
  type K2User,
} from './connect-user-roles'
import { WasViewerBadge, WasViewerNotice } from './WasViewerNotice'
import { PERSON_NAME_MAX, realName } from '@/lib/person-name'
import { setPersonName } from '@/components/Presence/person-name-api'
import { CanCloneToggle, setCanCloneOptimistic } from './CanCloneToggle'
import {
  OwnerAccountTag,
  ownerLinkAccess,
  useOwnerAccounts,
} from './OwnerAccountToggle'
import { threadPushAvailable } from './useNotifyPrefs'
import { SewAdminAccessState } from './SewAccessParts'
import { Toggle } from '@/components/ui'
import { AllowRemoteInstructRow } from '../shared/AllowRemoteInstructRow'
import { DnsManageEnabledRow } from '../shared/DnsManageEnabledRow'
import { AgentsCanCreateConnectionsRow } from '../shared/AgentsCanCreateConnectionsRow'
import { ConnectLoginIngressRow } from '../shared/ConnectLoginIngressRow'
import { BootStatusPublicRow } from '../shared/BootStatusPublicRow'
import { TunnelUrlsPanel } from './TunnelUrlsPanel'
import { canManagePublishedSubdomains } from '@/lib/published-subdomains'
import {
  signIn as accountSignIn,
  signOut as accountSignOut,
  refreshSession,
  listSubdomains,
  claimSubdomain,
  releaseSubdomain,
  freshClaim,
  type K2Session,
  type K2Subdomain,
} from '../lib/k2-account'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useConnectHostStore } from '@/stores/connect-host'
import { useSettingsStore } from '@/stores/settings'
import { useServerSupports, featureMinVersion } from '@/lib/server-capabilities'
import { primaryScope } from '@/kessel/server-scope'
import { onTunnelIdentityChanged } from '@/stores/session-events'
import { runTunnelSwitch, type TunnelSwitchProgress } from '../lib/tunnel-switch'
import { isAirgap } from '@/lib/airgap'

// 0.46.6: no relay address lives here. The daemon owns the default
// (relay.k2.dev) and the relay list; the form shows what the daemon
// stores and only placeholders otherwise (an empty field is never sent).
const RELAY_PLACEHOLDER = 'relay.k2.dev'
const PORT_PLACEHOLDER = '7000'

/** POST /cli/tunnel/config body for a subdomain bind (account flow). No
 *  relay address: like `k2 connect login`, a bind never pins one, so the
 *  daemon keeps its own (relay.k2.dev, or the list k2.dev serves). */
export function bindConfigBody(
  sub: { label: string; tunnel_token: string },
  deviceId: string,
  deviceLabel: string,
): Record<string, unknown> {
  return {
    subdomain: sub.label,
    token: sub.tunnel_token,
    // Persist this device's claim identity now so a later auto-start /
    // headless boot can renew the lease without the renderer (K2 #674).
    deviceId,
    deviceLabel,
  }
}

/** POST /cli/tunnel/config body for the Advanced "Save". The relay
 *  address and port go only when the person edited them (`relayEdited`),
 *  so a save never re-pins what the field happened to show. */
export function manualConfigBody(form: {
  serverAddr: string
  serverPort: string
  subdomain: string
  token: string
  relayEdited: boolean
}): { ok: true; body: Record<string, unknown> } | { ok: false; error: string } {
  const body: Record<string, unknown> = { subdomain: form.subdomain.trim() }
  if (form.relayEdited) {
    const addr = form.serverAddr.trim()
    const port = form.serverPort.trim()
    if (addr) body.serverAddr = addr
    if (port) {
      const portNum = Number(port)
      if (!Number.isInteger(portNum) || portNum <= 0 || portNum > 65535) {
        return { ok: false, error: 'Server port must be 1–65535' }
      }
      body.serverPort = portNum
    }
  }
  // Only send the token when the user typed one (empty string would
  // CLEAR it server-side; undefined leaves the saved token untouched).
  if (form.token.trim()) body.token = form.token.trim()
  return { ok: true, body }
}

// Keychain coordinates for the k2.dev ACCOUNT session. Kept distinct from
// the host-token service (`com.k2so.connect.host-token`) so the two never
// collide. Only the refresh token + email are persisted; the access token
// is memory-only.
const ACCOUNT_KEYCHAIN_SERVICE = 'dev.k2.connect.account'
// Pre-0.40 service name — read fallback only; copied forward on first
// read, never deleted (an un-updated daemon may still read it).
const LEGACY_ACCOUNT_KEYCHAIN_SERVICE = 'com.k2so.connect.account'
// Double-prompt fix: the session is now ONE keychain item — a JSON blob of
// {refreshToken,email} under this single account key — instead of two
// separate items. macOS prompts once per keychain ITEM on first access, so
// one item = one prompt (was two). The OLD two-key layout is still READ as a
// migration fallback (see readAccountSession).
const SESSION_BLOB_KEY = 'session'
// Legacy two-item layout (pre-double-prompt-fix). Read-only fallback.
const SESSION_ACCOUNT_KEY = 'session-refresh-token'
const EMAIL_ACCOUNT_KEY = 'session-email'

interface AccountSession {
  refreshToken: string
  email: string
}

// A stable per-install device id used for the subdomain claim/lease. Created
// once and persisted to localStorage so the same machine always presents the
// same identity to the claim RPC (so its own claim never looks like "another
// device"). Falls back to a random id if crypto/localStorage are unavailable.
const DEVICE_ID_KEY = 'k2.connect.device-id'
function getDeviceId(): string {
  try {
    const existing = localStorage.getItem(DEVICE_ID_KEY)
    if (existing) return existing
    const fresh =
      typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
        ? crypto.randomUUID()
        : `dev-${Date.now()}-${Math.random().toString(36).slice(2)}`
    localStorage.setItem(DEVICE_ID_KEY, fresh)
    return fresh
  } catch {
    return `dev-${Date.now()}-${Math.random().toString(36).slice(2)}`
  }
}

// K2 #674: the lease-renewal cadence (re-claim well inside the 3-minute
// server TTL) now lives in the DAEMON
// (`crates/k2so-core/src/tunnel/lease.rs::RENEW_INTERVAL`) so it survives
// this panel closing and works headless. The renderer no longer schedules
// renewal — it only fires one-shot claims on explicit user actions.

export const K2_CONNECT_MANIFEST: SettingEntry[] = [
  { id: 'k2-connect.account-login', section: 'k2-connect', label: 'Sign in to K2 Connect', description: 'Sign in to k2.dev to pick a purchased subdomain', keywords: ['login', 'sign in', 'account', 'k2.dev', 'email', 'password'] },
  { id: 'k2-connect.subdomain-list', section: 'k2-connect', label: 'Your subdomains', description: 'Purchased k2.dev subdomains bound to this device', keywords: ['subdomain', 'purchased', 'list', 'bind', 'select'] },
  { id: 'k2-connect.subdomain', section: 'k2-connect', label: 'Subdomain', description: 'The k2.dev subdomain this device exposes as', keywords: ['subdomain', 'expose', 'tunnel', 'k2.dev', 'public url', 'host'] },
  { id: 'k2-connect.token', section: 'k2-connect', label: 'K2 Connect Token', description: 'Bearer token for the frpc tunnel', keywords: ['token', 'tunnel', 'frpc', 'auth', 'bearer'] },
  { id: 'k2-connect.server-addr', section: 'k2-connect', label: 'Tunnel Server Address', description: 'The K2 Connect frps endpoint', keywords: ['server', 'frps', 'address', 'hetzner', 'endpoint'] },
  { id: 'k2-connect.start-stop', section: 'k2-connect', label: 'Start / Stop Tunnel', description: 'Expose this device at a public URL', keywords: ['start', 'stop', 'tunnel', 'expose', 'connect'] },
  { id: 'k2-connect.auto-start', section: 'k2-connect', label: 'Re-launch tunnel on restart', description: 'Automatically start this tunnel when the daemon restarts', keywords: ['auto', 'autostart', 'restart', 'reconnect', 'boot', 'relaunch'] },
  { id: 'k2-connect.users', section: 'k2-connect', label: 'Users / Access', description: 'People you allow to connect in to this device’s daemon', keywords: ['users', 'access', 'people', 'login', 'password', 'connect in', 'multi-user', 'allow', 'invite'] },
  { id: 'k2-connect.urls', section: 'k2-connect', label: 'URLs', description: 'Tunnel status plus every nested subdomain URL and its workspace', keywords: ['urls', 'nested', 'subdomains', 'routing', 'targets', 'ports', 'workspace', 'attribution', 'claim'] },
  { id: 'k2-connect.federation', section: 'k2-connect', label: 'Enable federation', description: 'Allow cross-server agent communication on this server', keywords: ['federation', 'cross-server', 'peers', 'agents', 'connect', 'mesh', 'cortana'] },
  { id: 'k2-connect.allow-remote-instruct', section: 'k2-connect', label: 'Let agents on other servers message every agent', description: 'List every agent to connected servers and let their agents message it. People who log in here can always message agents.', keywords: ['remote', 'instruct', 'message', 'agents', 'other servers', 'consent', 'federation', 'inbound', 'roster', 'allow'] },
  { id: 'k2-connect.public-api', section: 'k2-connect', label: 'Enable public API (/v1)', description: 'Serve the external /v1 API from this daemon — takes effect immediately, no restart', keywords: ['api', 'v1', 'public api', 'rest', 'http', 'external', 'surface', 'K2_API', 'api keys', 'host sessions'] },
  { id: 'k2-connect.login-ingress', section: 'k2-connect', label: 'Password sign-in over the tunnel', description: 'Only via K2 edge (recommended), any client, or off — where the daemon answers password login on tunnel ingress', keywords: ['login', 'sign in', 'password', 'tunnel', 'edge', 'ingress', 'scanner', 'brute force', 'connect users', 'app.k2.dev'] },
  { id: 'k2-connect.boot-status-public', section: 'k2-connect', label: "Hide this server's details from the internet", description: 'Visitors without a sign-in see only that the server is up — not its version or features (boot-status ghost)', keywords: ['boot-status', 'ghost', 'hide', 'version', 'fingerprint', 'scanner', 'privacy', 'stealth', 'ping'] },
]

interface TunnelStatus {
  running: boolean
  public_url: string | null
  frpc_installed: boolean
  /** PRD tunnel-disable-unpair §2A — persisted pause flag (false = user
   *  disabled the tunnel on this device). Optional: pre-PRD daemons omit it. */
  enabled?: boolean
  /** PRD §2B — this device released (unpaired) its tunnel identity. */
  released?: boolean
  /** 0.45.2 — the configured subdomain while running. */
  subdomain?: string | null
  /** 0.45.2 — the name the daemon's encrypted listener serves (null = no
   *  listener). Must equal `subdomain` while running. */
  e2eListener?: { subdomain: string; port: number } | null
}

// K2 #629 — connect-user permission tier. There is no Viewer tier any
// more (prd-remove-viewer-role-v1.md); see ./connect-user-roles.

// Whether a viewer of `role` may see + use the Users/Access management UI
// (add/remove/enable/disable). Admin + Owner only.
function canManageUsers(role: K2Role | null): boolean {
  return role === 'admin' || role === 'owner'
}

// Whether a viewer of `role` may change other users' roles. Owner only.
function canChangeRoles(role: K2Role | null): boolean {
  return role === 'owner'
}

interface PasswordPolicyView {
  minLength: number
  requireSpecial: boolean
  requireNumber: boolean
  requireUppercase: boolean
}

interface TunnelConfigView {
  serverAddr: string
  serverPort: number
  subdomain: string
  tokenSet: boolean
  publicUrl: string | null
  autoStart: boolean
  /** Stored opt-out preference (true = E2E on, the default). */
  e2e: boolean
  /** Effective state after any K2_E2E env override (may differ from `e2e`). */
  e2eEffective: boolean
  /** PRD tunnel-disable-unpair §2A — persisted pause flag. Optional: pre-PRD daemons omit it. */
  enabled?: boolean
  /** PRD §2B — this device is in the released (unpaired) state. */
  released?: boolean
}

// The daemon rotates ~/.k2so/daemon.token on every restart; getDaemonWs()
// caches the local creds per app session and only re-fetches on WS
// failure, so HTTP /cli/* calls can keep sending a stale token → 403
// until a manual window reload. Wrap each request so a 403 invalidates
// the creds cache, re-resolves fresh creds, and retries exactly once.
// withRemoteRetry wraps the whole send+403-retry so a REMOTE active host
// restarting/updating (which leaves a DEAD pooled WKWebView socket → a thrown
// network error) self-heals without an app relaunch. The 403 path (stale
// token, a Response not a throw) is orthogonal and stays inside the op;
// invalidateDaemonWs is the between-retries hook so a rotated port/token is
// re-read.
// Tunnel is always THIS Mac's daemon (expose local). Never use the
// active remote host — when the user is remoted into iascm.k2.dev (etc.)
// getDaemonWs() would hit the remote and (a) fire cross-origin POSTs
// with Content-Type that flaky preflights reject, and (b) thrash the
// connection story while Access/Users correctly talks to the remote.
async function tunnelGet(suffix: string): Promise<Response> {
  return withRemoteRetry(async () => {
    const send = async (): Promise<Response> => {
      const creds = await getLocalDaemonWs()
      const sep = suffix.includes('?') ? '&' : '?'
      return fetch(`${daemonHttpBase(creds)}/cli/tunnel/${suffix}${sep}token=${creds.token}`, { method: 'GET' })
    }
    const res = await send()
    if (res.status !== 403) return res
    invalidateDaemonWs()
    return send()
  }, { onRetry: invalidateDaemonWs })
}

async function tunnelPost(suffix: string, body?: unknown): Promise<Response> {
  return withRemoteRetry(async () => {
    const send = async (): Promise<Response> => {
      const creds = await getLocalDaemonWs()
      const sep = suffix.includes('?') ? '&' : '?'
      return fetch(`${daemonHttpBase(creds)}/cli/tunnel/${suffix}${sep}token=${creds.token}`, {
        method: 'POST',
        headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
        body: body !== undefined ? JSON.stringify(body) : undefined,
      })
    }
    const res = await send()
    if (res.status !== 403) return res
    invalidateDaemonWs()
    return send()
  }, { onRetry: invalidateDaemonWs })
}

// ── Users / Access (owner-gated /cli/users/*) ──────────────────────────
// Same stale-token-retry shape as tunnelGet/tunnelPost: the LOCAL daemon
// creds from getDaemonWs() carry the owner token these routes require.
async function userGet(suffix: string): Promise<Response> {
  return withRemoteRetry(async () => {
    const send = async (): Promise<Response> => {
      const creds = await getDaemonWs(primaryScope())
      return fetch(`${daemonHttpBase(creds)}/cli/users${suffix}?token=${creds.token}`, { method: 'GET' })
    }
    const res = await send()
    if (res.status !== 403) return res
    invalidateDaemonWs()
    return send()
  }, { onRetry: invalidateDaemonWs })
}

async function userPost(suffix: string, body?: unknown): Promise<Response> {
  return withRemoteRetry(async () => {
    const send = async (): Promise<Response> => {
      const creds = await getDaemonWs(primaryScope())
      return fetch(`${daemonHttpBase(creds)}/cli/users${suffix}?token=${creds.token}`, {
        method: 'POST',
        headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
        body: body !== undefined ? JSON.stringify(body) : undefined,
      })
    }
    const res = await send()
    if (res.status !== 403) return res
    invalidateDaemonWs()
    return send()
  }, { onRetry: invalidateDaemonWs })
}

// GET /cli/auth/whoami — resolve the LOCAL viewer's identity + role
// (K2 #629). With the local daemon owner token this returns
// {owner:true, role:'owner'}; the role gates the management UI below.
async function whoamiGet(): Promise<Response> {
  return withRemoteRetry(async () => {
    const send = async (): Promise<Response> => {
      const creds = await getDaemonWs(primaryScope())
      return fetch(`${daemonHttpBase(creds)}/cli/auth/whoami?token=${creds.token}`, { method: 'GET' })
    }
    const res = await send()
    if (res.status !== 403) return res
    invalidateDaemonWs()
    return send()
  }, { onRetry: invalidateDaemonWs })
}

async function errText(res: Response): Promise<string> {
  const text = await res.text()
  try {
    const parsed = JSON.parse(text)
    if (parsed && typeof parsed.error === 'string') return parsed.error
  } catch {
    /* raw text */
  }
  return text || `request failed (${res.status})`
}

// ── Account keychain helpers (refresh token + email only) ───────────────
// Best-effort + Tauri-only; they no-op (resolve) in the vitest/web env.

// Double-prompt fix: persist the WHOLE session as ONE keychain item (a JSON
// blob) under SESSION_BLOB_KEY. One item ⇒ macOS prompts once on first
// access (was two — refresh-token + email — so two prompts). On a fresh
// write we also clear any stale OLD two-item layout so a later read can't
// resurrect a superseded refresh token.
async function saveAccountSession(refreshToken: string, email: string): Promise<void> {
  try {
    // Route through the dedicated command that stamps the trusted-application
    // ACL up front (macOS) so the daemon's first read of this item never
    // raises a login-keychain prompt. On non-macOS it writes the identical
    // blob via keyring. Either way the stored bytes match readAccountSession's
    // expected shape ({ refreshToken, email } under the current service/blob).
    await invoke('k2_account_session_set', { refreshToken, email })
    // Best-effort cleanup of the superseded two-item layout (both services).
    for (const service of [ACCOUNT_KEYCHAIN_SERVICE, LEGACY_ACCOUNT_KEYCHAIN_SERVICE]) {
      try { await invoke('k2_secret_delete', { service, account: SESSION_ACCOUNT_KEY }) } catch { /* idempotent */ }
      try { await invoke('k2_secret_delete', { service, account: EMAIL_ACCOUNT_KEY }) } catch { /* idempotent */ }
    }
  } catch {
    /* keychain unavailable — session stays in memory for this run */
  }
}

// Read the persisted session. The PRIMARY read goes through
// k2_account_session_get — on macOS that's the daemon's `security`-CLI
// reader, the only prompt-free way to read an item the `security` CLI
// created: the CLI-created item's `apple-tool:` partition list blocks
// in-process keyring reads from the signed app REGARDLESS of the -T ACL,
// and the rotation writer re-creates the item so an "Always Allow" grant
// never sticks (the 0.40.31 visit-the-page-reprompt regression). It also
// already probes every layout (current blob → legacy blob → bare keys).
// The keyring fallbacks below only matter off-macOS / for app-created
// legacy items, both of which read silently.
async function readAccountSession(): Promise<AccountSession | null> {
  try {
    // 1. Primary: the dedicated command (CLI-read on macOS, keyring blob
    // read elsewhere). Same JSON blob shape either way.
    const primary = await invoke<string | null>('k2_account_session_get')
    if (primary) {
      try {
        const parsed = JSON.parse(primary) as Partial<AccountSession>
        if (parsed && typeof parsed.refreshToken === 'string' && parsed.refreshToken) {
          return { refreshToken: parsed.refreshToken, email: parsed.email ?? '' }
        }
      } catch { /* corrupt blob — fall through to the layouts below */ }
    }
    // 2. Legacy-service single-item blob via keyring (app-created in the
    // pre-rebrand era ⇒ silent). The CURRENT service is deliberately NOT
    // probed via keyring here: on macOS that item is CLI-created and a
    // keyring read of it is exactly the prompt this function exists to
    // avoid — and step 1 already covered it on every platform.
    {
      const raw = await invoke<string | null>('k2_secret_get', { service: LEGACY_ACCOUNT_KEYCHAIN_SERVICE, account: SESSION_BLOB_KEY })
      if (raw) {
        try {
          const parsed = JSON.parse(raw) as Partial<AccountSession>
          if (parsed && typeof parsed.refreshToken === 'string' && parsed.refreshToken) {
            const sess: AccountSession = { refreshToken: parsed.refreshToken, email: parsed.email ?? '' }
            // Migrate the legacy-service blob forward to the current service.
            await saveAccountSession(sess.refreshToken, sess.email)
            return sess
          }
        } catch { /* corrupt blob — fall through to the two-item layout */ }
      }
    }
    // 3. OLD two-item layout, current service then legacy rebrand service
    // (all app-created via keyring in their era ⇒ silent reads).
    for (const service of [ACCOUNT_KEYCHAIN_SERVICE, LEGACY_ACCOUNT_KEYCHAIN_SERVICE]) {
      const refreshToken = await invoke<string | null>('k2_secret_get', { service, account: SESSION_ACCOUNT_KEY })
      if (refreshToken) {
        const email = (await invoke<string | null>('k2_secret_get', { service, account: EMAIL_ACCOUNT_KEY })) ?? ''
        const sess: AccountSession = { refreshToken, email }
        // Migrate forward: write the single blob + delete the old items.
        await saveAccountSession(sess.refreshToken, sess.email)
        return sess
      }
    }
    return null
  } catch {
    return null
  }
}

/**
 * A fresh k2.dev access token from the stored account session, or null when
 * no one is signed in (or the refresh fails, or air-gap is on). Used by
 * Settings → Compute's one-click pairing. Persists a rotated refresh token
 * exactly like the Tunnel panel's restore does. Never clears the stored
 * session: signing out stays the Tunnel panel's job.
 */
export async function getAccountAccessToken(): Promise<string | null> {
  if (isAirgap()) return null
  const stored = await readAccountSession()
  if (!stored) return null
  let fresh: K2Session
  try {
    fresh = await refreshSession(stored.refreshToken)
  } catch {
    return null
  }
  if (fresh.refreshToken !== stored.refreshToken || fresh.email !== stored.email) {
    await saveAccountSession(fresh.refreshToken, fresh.email)
  }
  return fresh.accessToken
}

async function clearAccountSession(): Promise<void> {
  // Sign-out clears the single blob AND the legacy two-item layout under
  // BOTH service names — leaving any copy behind would silently re-sign-in
  // via the migration fallback above. The dedicated command deletes via the
  // `security` CLI on macOS (a keyring delete of the CLI-created blob could
  // prompt — same partition-list story as readAccountSession) and via
  // keyring elsewhere; the keyring sweep after it is belt-and-braces for
  // any stray app-created leftovers and is silent once the command ran.
  try { await invoke('k2_account_session_clear') } catch { /* fall through to the sweep */ }
  for (const service of [ACCOUNT_KEYCHAIN_SERVICE, LEGACY_ACCOUNT_KEYCHAIN_SERVICE]) {
    for (const account of [SESSION_BLOB_KEY, SESSION_ACCOUNT_KEY, EMAIL_ACCOUNT_KEY]) {
      try {
        await invoke('k2_secret_delete', { service, account })
      } catch { /* idempotent */ }
    }
  }
}

/** Host-side panels rendered under the flat K2 Connect tab strip (no nested tabs). */
export type K2ConnectHostPanel = 'tunnel' | 'people' | 'policies'

export function K2ConnectSection({
  panel = 'tunnel',
}: {
  /** Which host panel to show. Shell owns the peer tabs. */
  panel?: K2ConnectHostPanel
}): React.JSX.Element {
  const [serverAddr, setServerAddr] = useState('')
  const [serverPort, setServerPort] = useState('')
  // True once the person typed in the relay fields; only then does Save
  // send them (0.46.6: a save never re-pins the address it was shown).
  const [relayEdited, setRelayEdited] = useState(false)
  const [subdomain, setSubdomain] = useState('')
  const [token, setToken] = useState('')
  const [tokenSet, setTokenSet] = useState(false)
  const [autoStart, setAutoStart] = useState(false)
  const [status, setStatus] = useState<TunnelStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [savedMsg, setSavedMsg] = useState<string | null>(null)

  // ── Account / subdomain-picker state ──────────────────────────────────
  const [session, setSession] = useState<K2Session | null>(null)
  const [subdomains, setSubdomains] = useState<K2Subdomain[]>([])
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [authBusy, setAuthBusy] = useState(false)
  const [authError, setAuthError] = useState<string | null>(null)
  const [selectedLabel, setSelectedLabel] = useState<string | null>(null)
  const [boundMsg, setBoundMsg] = useState<string | null>(null)
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [swapBusy, setSwapBusy] = useState(false)
  // 0.45.2: live progress of a subdomain switch (claim → daemon steps).
  const [switchProgress, setSwitchProgress] = useState<TunnelSwitchProgress | null>(null)

  // ── Subdomain claim / lease ───────────────────────────────────────────
  const deviceIdRef = useRef<string>(getDeviceId())
  const deviceId = deviceIdRef.current
  // navigator.platform is deprecated but still the simplest stable hint;
  // it's purely cosmetic ("MacIntel" etc.) — the holder UI falls back to
  // "another device" when absent.
  const deviceLabel = typeof navigator !== 'undefined' ? navigator.platform || undefined : undefined
  // K2 #674: the PERIODIC lease renewal now lives in the DAEMON
  // (`crates/k2so-core/src/tunnel/lease.rs`), tied to the tunnel's
  // lifecycle, so the lease survives this panel being closed and works
  // headless. The renderer keeps only the ONE-SHOT claims on explicit user
  // actions (start / swap, below) and the status DISPLAY. It pushes this
  // device's claim identity (`deviceId`/`deviceLabel`) to the daemon via
  // `/cli/tunnel/config` so the daemon renews under the SAME device.
  // Ref the one-shot claim paths read so they never close over stale state.
  const accessTokenRef = useRef<string | null>(null)
  const confirm = useConfirmDialogStore((s) => s.confirm)

  // K2 #628: the tunnel EXPOSE controls only make sense for THIS Mac's
  // own daemon — you can't expose someone else's daemon through your local
  // tunnel. When the active host is a remote K2 Connect host, hide the
  // account-login / tunnel-config / start-stop sub-panels and show a short
  // note instead. The Users/Access panel below stays (it manages the
  // remote daemon's users, already role-gated via /cli/auth/whoami).
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const isRemote = activeHost !== 'local'
  // Federation master switch for the ACTIVE host. persistAndApply is
  // host-aware, so toggling while connected to a remote server writes to THAT
  // server's daemon — that's the per-server enable.
  const federationEnabled = useSettingsStore((s) => s.federationEnabled)
  const setFederationEnabled = useSettingsStore((s) => s.setFederationEnabled)
  // 0.40.43 (1c): public /v1 API switch for the ACTIVE host. Same
  // host-aware persist as the federation master above; the daemon checks
  // the flag PER REQUEST, so this toggle needs no restart and therefore
  // NO confirm+reboot dialog — flipping it is immediately effective and
  // immediately reversible.
  const apiEnabled = useSettingsStore((s) => s.apiEnabled)
  const setApiEnabled = useSettingsStore((s) => s.setApiEnabled)
  // #638: when viewing a REMOTE host whose version is known but predates
  // roles (#629), whoami returns no role → the generic "handled by an
  // administrator" note would mislead. Swap in a version-aware hint instead.
  const serverVersion = useConnectHostStore((s) => s.serverVersion)
  const supportsRoles = useServerSupports('roles')
  const oldHostNoRoles = isRemote && serverVersion !== null && !supportsRoles

  // ── Users / Access state ──────────────────────────────────────────────
  // K2 #629: the LOCAL viewer's role (from whoami). The desktop app talks
  // to its OWN daemon with the owner token, so this is normally 'owner';
  // null until resolved. It gates the whole management panel + the role
  // selector below.
  const [myRole, setMyRole] = useState<K2Role | null>(null)
  const [whoamiLoaded, setWhoamiLoaded] = useState(false)
  // The daemon's owner token (whoami `owner:true`), not a Connect login —
  // even an Owner-role one. Only it may mark "This is me (owner)".
  const [ownerToken, setOwnerToken] = useState(false)
  const [users, setUsers] = useState<K2User[]>([])
  const [usersLoaded, setUsersLoaded] = useState(false)
  const [newUsername, setNewUsername] = useState('')
  const [newPassword, setNewPassword] = useState('')
  // 0.46.0: optional first/last name on the new account.
  const [newFirstName, setNewFirstName] = useState('')
  const [newLastName, setNewLastName] = useState('')
  // 0.46.0: the row whose first/last name is being edited.
  const [nameFor, setNameFor] = useState<string | null>(null)
  const [nameDraft, setNameDraft] = useState<{ firstName: string; lastName: string }>({ firstName: '', lastName: '' })
  const [nameBusy, setNameBusy] = useState(false)
  const [newRole, setNewRole] = useState<K2Role>('member')
  const [userQuery, setUserQuery] = useState('')
  const [showNewPassword, setShowNewPassword] = useState(false)
  const [addBusy, setAddBusy] = useState(false)
  const [addError, setAddError] = useState<string | null>(null)
  const [addedMsg, setAddedMsg] = useState<string | null>(null)
  const [usersError, setUsersError] = useState<string | null>(null)
  // username currently revealing its reset-password input
  const [resetFor, setResetFor] = useState<string | null>(null)
  const [resetPassword, setResetPassword] = useState('')
  const [resetBusy, setResetBusy] = useState(false)
  const [resetMsg, setResetMsg] = useState<string | null>(null)
  // username currently pending a remove confirm
  const [removeConfirm, setRemoveConfirm] = useState<string | null>(null)
  // username whose "Enable as Member" (former Viewer) is in flight
  const [enableBusy, setEnableBusy] = useState<string | null>(null)

  // ── Password policy state (K2 #620) ─────────────────────────────────
  const [policyMinLength, setPolicyMinLength] = useState('8')
  const [policyRequireSpecial, setPolicyRequireSpecial] = useState(false)
  const [policyRequireNumber, setPolicyRequireNumber] = useState(false)
  const [policyRequireUppercase, setPolicyRequireUppercase] = useState(false)
  const [policySavedMsg, setPolicySavedMsg] = useState<string | null>(null)
  const [policyError, setPolicyError] = useState<string | null>(null)

  // (Re)load the stored tunnel config into the form state. Shared by the
  // mount effect and the release flow (which must show the identity GONE).
  const loadConfig = async (): Promise<void> => {
    try {
      const res = await tunnelGet('config')
      if (res.ok) {
        const cfg = (await res.json()) as TunnelConfigView
        setServerAddr(cfg.serverAddr || '')
        setServerPort(cfg.serverPort ? String(cfg.serverPort) : '')
        setRelayEdited(false)
        setSubdomain(cfg.subdomain || '')
        setTokenSet(cfg.tokenSet)
        setAutoStart(cfg.autoStart ?? false)
      }
    } catch { /* ignore */ }
  }

  // Load per-panel. Tunnel I/O is LOCAL-only (see tunnelGet). Users /
  // policies talk to the ACTIVE host (getDaemonWs). Do not poll tunnel
  // status while the Access panel is open on a remote — that used to
  // hammer the remote tunnel routes and amplify reconnect spam.
  useEffect(() => {
    let cancelled = false
    let interval: ReturnType<typeof setInterval> | null = null

    if (panel === 'tunnel') {
      void (async () => {
        if (isRemote) return // expose UI is local-only; see isRemote note
        // 0.45.7: the role gates Add / Change target / Remove on URLs.
        void refreshWhoami()
        await loadConfig()
        if (cancelled) return
        void refreshStatus()
      })()
      // Restore the k2.dev account session from the keychain (local).
      void (async () => {
        const stored = await readAccountSession()
        if (!stored || cancelled) return
        try {
          const fresh = await refreshSession(stored.refreshToken)
          if (cancelled) return
          setSession(fresh)
          if (fresh.refreshToken !== stored.refreshToken || fresh.email !== stored.email) {
            await saveAccountSession(fresh.refreshToken, fresh.email)
          }
          const subs = await listSubdomains(fresh.accessToken)
          if (!cancelled) setSubdomains(subs)
        } catch {
          await clearAccountSession()
          if (!cancelled) setSession(null)
        }
      })()
      if (!isRemote) {
        interval = setInterval(() => void refreshStatus(), 5000)
      }
    } else if (panel === 'people' || panel === 'policies') {
      // Active-host whoami + users (remote-aware).
      void (async () => {
        const role = await refreshWhoami()
        if (cancelled) return
        if (canManageUsers(role)) {
          void refreshUsers()
          void refreshPolicy()
        }
      })()
    }

    return () => {
      cancelled = true
      if (interval) clearInterval(interval)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [panel, isRemote])

  const refreshStatus = async (): Promise<void> => {
    try {
      const res = await tunnelGet('status')
      if (res.ok) setStatus((await res.json()) as TunnelStatus)
    } catch { /* ignore */ }
  }

  // 0.45.2: the daemon announces every tunnel identity change (a switch or
  // release from here, the CLI, or another window) — reload instead of
  // waiting for the 5 s poll.
  useEffect(() => {
    if (panel !== 'tunnel' || isRemote) return
    return onTunnelIdentityChanged(primaryScope(), () => {
      void loadConfig()
      void refreshStatus()
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [panel, isRemote])

  // Keep the access-token ref current so the one-shot claim paths (start /
  // swap) can read it without closing over stale state.
  useEffect(() => {
    accessTokenRef.current = session?.accessToken ?? null
  }, [session])

  // Push this device's claim identity to the daemon so its lease-renewal
  // loop (K2 #674) renews under the SAME device the renderer claimed
  // with. Best-effort — a failure just means the daemon falls back to
  // whatever identity it already had stored; the one-shot claim still
  // happened. POSTs only the device fields so the token/subdomain are
  // untouched.
  const pushDeviceIdentity = async (): Promise<void> => {
    try {
      await tunnelPost('config', { deviceId, deviceLabel: deviceLabel ?? '' })
    } catch {
      /* best-effort — daemon renews with its stored identity */
    }
  }

  // Refresh the owned-subdomain list (incl. live claim columns) so the
  // dropdown grey-out reflects the latest holders.
  const refreshSubdomains = async (): Promise<void> => {
    const accessToken = accessTokenRef.current
    if (!accessToken) return
    try {
      const subs = await listSubdomains(accessToken)
      setSubdomains(subs)
    } catch { /* ignore — keep the current list */ }
  }

  // K2 #674: the renderer no longer drives the periodic lease renewal —
  // the daemon owns it now (see `crates/k2so-core/src/tunnel/lease.rs`), so
  // the lease survives this panel unmounting and a headless daemon. When a
  // tunnel is already running with an account session + bound subdomain
  // (resumed session / auto-start), just (re)assert this device's claim
  // identity on the daemon so its renewal uses the right device.
  useEffect(() => {
    const live = status?.running ?? false
    if (live && session && subdomain.trim()) {
      void pushDeviceIdentity()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status?.running, session, subdomain])

  // ── Users / Access actions ────────────────────────────────────────────
  // K2 #629: resolve the local viewer's role. Returns the role (also
  // stored in state) so the mount effect can decide whether to load the
  // user list. Defaults to null on any failure (UI then shows the
  // "handled by an administrator" note rather than a raw token error).
  const refreshWhoami = async (): Promise<K2Role | null> => {
    try {
      const res = await whoamiGet()
      if (res.ok) {
        const data = (await res.json()) as { role?: string; owner?: boolean }
        const role: K2Role | null = parseK2Role(data.role) ?? (data.owner ? 'owner' : null)
        setMyRole(role)
        setOwnerToken(data.owner === true)
        return role
      }
      setMyRole(null)
      setOwnerToken(false)
      return null
    } catch {
      setMyRole(null)
      setOwnerToken(false)
      return null
    } finally {
      setWhoamiLoaded(true)
    }
  }

  const refreshUsers = async (): Promise<void> => {
    try {
      const res = await userGet('')
      if (res.ok) {
        const data = (await res.json()) as { users?: K2User[] }
        setUsers(Array.isArray(data.users) ? data.users : [])
        setUsersError(null)
      } else {
        setUsersError(await errText(res))
      }
    } catch (e) {
      setUsersError(e instanceof Error ? e.message : 'Failed to load users')
    } finally {
      setUsersLoaded(true)
    }
  }

  // ── Password policy (K2 #620) ───────────────────────────────────────
  const refreshPolicy = async (): Promise<void> => {
    try {
      const res = await userGet('/policy')
      if (res.ok) {
        const p = (await res.json()) as PasswordPolicyView
        setPolicyMinLength(String(p.minLength ?? 8))
        setPolicyRequireSpecial(!!p.requireSpecial)
        setPolicyRequireNumber(!!p.requireNumber)
        setPolicyRequireUppercase(!!p.requireUppercase)
        setPolicyError(null)
      }
    } catch { /* ignore — keep defaults */ }
  }

  const savePolicy = async (): Promise<void> => {
    setPolicyError(null)
    setPolicySavedMsg(null)
    const min = Number(policyMinLength)
    if (!Number.isInteger(min) || min < 4 || min > 128) {
      setPolicyError('Minimum length must be 4–128.')
      return
    }
    try {
      const res = await userPost('/policy', {
        minLength: min,
        requireSpecial: policyRequireSpecial,
        requireNumber: policyRequireNumber,
        requireUppercase: policyRequireUppercase,
      })
      if (!res.ok) {
        setPolicyError(await errText(res))
        return
      }
      setPolicySavedMsg('Saved')
      setTimeout(() => setPolicySavedMsg(null), 1500)
      // Re-read so the clamped min length is reflected.
      await refreshPolicy()
    } catch (e) {
      setPolicyError(e instanceof Error ? e.message : 'Failed to save policy')
    }
  }

  const addUser = async (): Promise<void> => {
    const username = newUsername.trim().toLowerCase()
    const pw = newPassword
    if (!username || !pw) return
    setAddBusy(true)
    setAddError(null)
    setAddedMsg(null)
    try {
      const firstName = newFirstName.trim()
      const lastName = newLastName.trim()
      const res = await userPost('/add', {
        username,
        password: pw,
        ...(firstName ? { firstName } : {}),
        ...(lastName ? { lastName } : {}),
      })
      if (!res.ok) {
        setAddError(await errText(res))
        return
      }
      // add_user always creates Member. Owner can pick a starting role
      // here; apply it with the same set-role path as the per-row selector.
      if (canChangeRoles(myRole) && newRole !== 'member') {
        const roleRes = await userPost('/set-role', { username, role: newRole })
        if (!roleRes.ok) {
          setAddError(`Added as member, but role could not be set: ${await errText(roleRes)}`)
          await refreshUsers()
          return
        }
      }
      setNewUsername('')
      setNewPassword('')
      setNewFirstName('')
      setNewLastName('')
      setNewRole('member')
      setAddedMsg(`Added — share these credentials with the user.`)
      setTimeout(() => setAddedMsg(null), 4000)
      await refreshUsers()
    } catch (e) {
      setAddError(e instanceof Error ? e.message : 'Failed to add user')
    } finally {
      setAddBusy(false)
    }
  }

  // 0.46.0: an Admin+ sets someone's first/last name (POST /cli/people/name
  // with `person`; the daemon applies the colour floors). Empty clears.
  const submitName = async (username: string): Promise<void> => {
    setNameBusy(true)
    setUsersError(null)
    try {
      await setPersonName(primaryScope(), {
        person: username,
        firstName: nameDraft.firstName.trim() || null,
        lastName: nameDraft.lastName.trim() || null,
      })
      setNameFor(null)
      await refreshUsers()
    } catch (e) {
      setUsersError(e instanceof Error ? e.message : 'Failed to save the name')
    } finally {
      setNameBusy(false)
    }
  }

  const submitReset = async (username: string): Promise<void> => {
    if (!resetPassword) return
    setResetBusy(true)
    setUsersError(null)
    try {
      const res = await userPost('/set-password', { username, password: resetPassword })
      if (!res.ok) {
        setUsersError(await errText(res))
        return
      }
      setResetFor(null)
      setResetPassword('')
      setResetMsg(`Password reset for ${username}.`)
      setTimeout(() => setResetMsg(null), 4000)
      await refreshUsers()
    } catch (e) {
      setUsersError(e instanceof Error ? e.message : 'Failed to reset password')
    } finally {
      setResetBusy(false)
    }
  }

  const toggleDisabled = async (username: string, disabled: boolean): Promise<void> => {
    // Optimistic flip with revert-on-failure.
    setUsers((prev) => prev.map((u) => (u.username === username ? { ...u, disabled } : u)))
    setUsersError(null)
    try {
      const res = await userPost('/set-disabled', { username, disabled })
      if (!res.ok) {
        setUsers((prev) => prev.map((u) => (u.username === username ? { ...u, disabled: !disabled } : u)))
        setUsersError(await errText(res))
        return
      }
      await refreshUsers()
    } catch (e) {
      setUsers((prev) => prev.map((u) => (u.username === username ? { ...u, disabled: !disabled } : u)))
      setUsersError(e instanceof Error ? e.message : 'Failed to update user')
    }
  }

  // Former Viewer → Member (prd-remove-viewer-role-v1.md RV16). The daemon
  // already stores the account as a disabled Member; enabling it is the
  // owner's explicit choice and clears `wasViewer` server-side.
  const enableAsMember = async (username: string): Promise<void> => {
    setEnableBusy(username)
    setUsersError(null)
    try {
      const res = await userPost('/set-disabled', { username, disabled: false })
      if (!res.ok) {
        setUsersError(await errText(res))
        return
      }
      await refreshUsers()
    } catch (e) {
      setUsersError(e instanceof Error ? e.message : 'Failed to enable user')
    } finally {
      setEnableBusy(null)
    }
  }

  const removeUser = async (username: string): Promise<void> => {
    setUsersError(null)
    try {
      const res = await userPost('/remove', { username })
      if (!res.ok) {
        setUsersError(await errText(res))
        return
      }
      setRemoveConfirm(null)
      await refreshUsers()
    } catch (e) {
      setUsersError(e instanceof Error ? e.message : 'Failed to remove user')
    }
  }

  // Settings → Admin Access → "Can clone" (0.46.0, Owner-only;
  // POST /cli/users/set-can-clone). Optimistic with revert-on-failure.
  const changeCanClone = (username: string, canClone: boolean): Promise<void> =>
    setCanCloneOptimistic({
      username,
      next: canClone,
      post: userPost,
      apply: (u, v) => setUsers((prev) => prev.map((x) => (x.username === u ? { ...x, canClone: v } : x))),
      errText,
      onError: (msg) => setUsersError(msg),
      refresh: refreshUsers,
    })

  // K2 #629: change a user's role (Owner-only; POST /cli/users/set-role).
  // Optimistic with revert-on-failure, mirroring toggleDisabled.
  const changeRole = async (username: string, role: K2Role): Promise<void> => {
    const prevRole = users.find((u) => u.username === username)?.role
    setUsers((prev) => prev.map((u) => (u.username === username ? { ...u, role } : u)))
    setUsersError(null)
    try {
      const res = await userPost('/set-role', { username, role })
      if (!res.ok) {
        setUsers((prev) => prev.map((u) => (u.username === username ? { ...u, role: prevRole } : u)))
        setUsersError(await errText(res))
        return
      }
      await refreshUsers()
    } catch (e) {
      setUsers((prev) => prev.map((u) => (u.username === username ? { ...u, role: prevRole } : u)))
      setUsersError(e instanceof Error ? e.message : 'Failed to change role')
    }
  }

  const saveConfig = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    setSavedMsg(null)
    const built = manualConfigBody({ serverAddr, serverPort, subdomain, token, relayEdited })
    if (!built.ok) {
      setError(built.error)
      setBusy(false)
      return
    }
    const body = built.body
    try {
      const res = await tunnelPost('config', body)
      if (!res.ok) {
        setError(await errText(res))
        return
      }
      const cfg = (await res.json()) as TunnelConfigView
      setTokenSet(cfg.tokenSet)
      setToken('') // never keep the secret in the field after save
      setServerAddr(cfg.serverAddr || '')
      setServerPort(cfg.serverPort ? String(cfg.serverPort) : '')
      setRelayEdited(false)
      setSavedMsg('Saved')
      setTimeout(() => setSavedMsg(null), 1500)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Save failed')
    } finally {
      setBusy(false)
    }
  }

  const startTunnel = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      const sub = subdomain.trim()
      // Mirror the daemon's is_connectable() locally so an unconfigured Start
      // fails with a clear, actionable message instead of a round-trip
      // "tunnel not configured: no token". Picking a subdomain above is what
      // binds its token — with neither a picked subdomain nor a manually-set
      // token, there is nothing to start.
      if (!sub && !tokenSet) {
        setError('Select a subdomain first — pick one of your purchased subdomains above (or set one under Advanced), then Start.')
        return
      }
      // Claim the lease BEFORE starting (one-shot, explicit user action).
      // If a *different* device holds a fresh claim, refuse to start and
      // surface who holds it.
      const accessToken = session?.accessToken ?? accessTokenRef.current
      if (sub && accessToken) {
        try {
          const result = await claimSubdomain(accessToken, sub, deviceId, deviceLabel)
          if (!result.claimed && result.holder && result.holder !== deviceId) {
            setError(`${sub}.k2.dev is in use on another device — stop it there first.`)
            return
          }
        } catch (e) {
          setError(e instanceof Error ? e.message : 'Failed to claim subdomain')
          return
        }
      }
      // Make sure the daemon renews under THIS device (K2 #674) before we
      // start — its renewal loop spins up on tunnel start and reads the
      // stored identity.
      await pushDeviceIdentity()
      // PRD tunnel-disable-unpair §2A: Start goes through /enable so the
      // PERSISTED pause flag is raised by the same click — all surfaces
      // (this button, `k2 tunnel enable`, the route) write the SAME flag.
      const res = await tunnelPost(`enable${sub ? `?subdomain=${encodeURIComponent(sub)}` : ''}`)
      if (!res.ok) {
        // The connector surfaces the frpc-not-installed hint verbatim here.
        setError(await errText(res))
        // Don't keep the lease we just took if the tunnel didn't start.
        if (sub && accessToken) void releaseSubdomain(accessToken, sub, deviceId).catch(() => undefined)
        return
      }
      await refreshStatus()
      // Lease renewal is now the DAEMON's job (tied to the tunnel being up).
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Start failed')
    } finally {
      setBusy(false)
    }
  }

  const stopTunnel = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    // Release the lease (best-effort) regardless of the stop call's
    // outcome. The DAEMON stops its own renewal loop when the tunnel is
    // torn down (K2 #674) — `/cli/tunnel/stop` flips the lifecycle flag
    // the renewal thread watches.
    const accessToken = session?.accessToken ?? accessTokenRef.current
    const bound = subdomain.trim()
    if (accessToken && bound) void releaseSubdomain(accessToken, bound, deviceId).catch(() => undefined)
    try {
      // PRD tunnel-disable-unpair §2A: Stop goes through /disable — the
      // pause is PERSISTED on disk, so a daemon restart / reboot /
      // orphaned daemon can never quietly bring the tunnel back (the
      // incident this PRD was born from). Start symmetrically re-enables.
      const res = await tunnelPost('disable')
      if (!res.ok) {
        setError(await errText(res))
        return
      }
      await refreshStatus()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Stop failed')
    } finally {
      setBusy(false)
    }
  }

  // PRD tunnel-disable-unpair §2B — Release subdomain from this device
  // (unpair). DESTRUCTIVE: deletes this device's tunnel identity
  // (tunnel.json + rendered frpc config + E2E keypair), tombstones
  // ~/.k2/unpaired.json, and notifies the relay so the old identity is
  // refused forever. The ACCOUNT keeps the subdomain — the confirm text
  // must say "this device", never "your subdomain".
  //
  // 0.45.2: K2 Connect is told only when this device's tunnel is RUNNING
  // (it holds the name). `forget` never tells it anything: for a stale
  // config copied from another machine whose subdomain a live box holds.
  // Moving to another subdomain does not need either — pick it in the
  // dropdown above (one switch, no restart).
  const releaseFromDevice = async (mode: 'release' | 'forget'): Promise<void> => {
    const sub = subdomain.trim()
    const label = sub ? `${sub}.k2.dev` : 'its subdomain'
    const ok =
      mode === 'forget'
        ? await confirm({
            title: 'Forget this tunnel on this device?',
            message:
              `This device stops using ${label} and deletes its local tunnel settings. ` +
              `Nothing is sent to K2 Connect, so whichever device holds ${label} now keeps it. ` +
              `Use this for settings copied from another machine.`,
            confirmLabel: 'Forget on this device',
          })
        : await confirm({
            title: 'Release subdomain from this device?',
            message:
              `${label} will stop routing to THIS DEVICE — this copy of the tunnel identity ` +
              `is refused from now on. ` +
              (running
                ? `K2 Connect is told this device let go of it. `
                : `This device's tunnel is not running, so it is not the holder: nothing is sent to K2 Connect. `) +
              `Your account keeps the subdomain; you can pair it on any device any time. ` +
              `(Moving to another subdomain? Pick it above instead. Just want to pause? Use Stop tunnel.)`,
            confirmLabel: 'Release this device',
          })
    if (!ok) return
    setBusy(true)
    setError(null)
    setSwitchProgress(null)
    try {
      const res = await tunnelPost(mode === 'forget' ? 'release?confirm=1&mode=forget' : 'release?confirm=1')
      if (!res.ok) {
        setError(await errText(res))
        return
      }
      const report = (await res.json()) as { note?: string; localOnly?: boolean; upstreamReported?: boolean }
      setBoundMsg(
        report.note ??
          (report.upstreamReported ? `Released ${label}; K2 Connect recorded it.` : `Released ${label} on this device.`),
      )
      // Lease: only the holder had one; free it (best-effort).
      const accessToken = session?.accessToken ?? accessTokenRef.current
      if (mode === 'release' && running && accessToken && sub) {
        void releaseSubdomain(accessToken, sub, deviceId).catch(() => undefined)
      }
      // Identity is gone — reload config (tokenSet drops) + status
      // (released: true drives the tri-state card).
      await loadConfig()
      await refreshStatus()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Release failed')
    } finally {
      setBusy(false)
    }
  }

  // Toggle the daemon-side "re-launch on restart" opt-in. POSTs only
  // { autoStart } so the saved token / subdomain are untouched. Optimistic
  // flip with revert-on-failure.
  const toggleAutoStart = async (next: boolean): Promise<void> => {
    setAutoStart(next)
    setError(null)
    try {
      const res = await tunnelPost('config', { autoStart: next })
      if (!res.ok) {
        setAutoStart(!next) // revert
        setError(await errText(res))
        return
      }
      const cfg = (await res.json()) as TunnelConfigView
      setAutoStart(cfg.autoStart ?? next)
    } catch (e) {
      setAutoStart(!next) // revert
      setError(e instanceof Error ? e.message : 'Failed to update auto-start')
    }
  }

  const handleSignIn = async (): Promise<void> => {
    setAuthBusy(true)
    setAuthError(null)
    try {
      const sess = await accountSignIn(email.trim(), password)
      setSession(sess)
      setPassword('')
      await saveAccountSession(sess.refreshToken, sess.email)
      try {
        const subs = await listSubdomains(sess.accessToken)
        setSubdomains(subs)
      } catch (e) {
        setAuthError(e instanceof Error ? e.message : 'Failed to load subdomains')
      }
    } catch (e) {
      setAuthError(e instanceof Error ? e.message : 'Sign in failed')
    } finally {
      setAuthBusy(false)
    }
  }

  const handleSignOut = async (): Promise<void> => {
    const token = session?.accessToken
    setSession(null)
    setSubdomains([])
    setSelectedLabel(null)
    setBoundMsg(null)
    setAuthError(null)
    await clearAccountSession()
    if (token) void accountSignOut(token)
  }

  // Bind a chosen subdomain: fill the manual-config state AND persist the
  // chosen subdomain + token straight to the daemon so tunnel.json is
  // updated immediately. Uses the row's values directly (state setters are
  // async, so we can't read `subdomain`/`token` back in this tick).
  const bindSubdomain = async (sub: K2Subdomain): Promise<void> => {
    setSelectedLabel(sub.label)
    setBoundMsg(null)
    setSubdomain(sub.label)
    setToken(sub.tunnel_token)
    setBusy(true)
    setError(null)
    try {
      const res = await tunnelPost('config', bindConfigBody(sub, deviceId, deviceLabel ?? ''))
      if (!res.ok) {
        setError(await errText(res))
        return
      }
      const cfg = (await res.json()) as TunnelConfigView & {
        identityChange?: { ok: boolean; error?: string }
      }
      setTokenSet(cfg.tokenSet)
      setToken('') // don't keep the secret in the manual field after save
      // 0.45.2: the daemon moved off the old name as part of this write
      // (listener, cert renewer, routes); a failed step is shown, not lost.
      if (cfg.identityChange && !cfg.identityChange.ok) {
        setError(`Bound ${sub.label}.k2.dev, but: ${cfg.identityChange.error ?? 'a step failed'}`)
        return
      }
      setBoundMsg(`Bound ${sub.label}.k2.dev — Start the tunnel below.`)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to bind subdomain')
    } finally {
      setBusy(false)
    }
  }

  const inputCls =
    'w-full px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

  const visibleUsers = useMemo(() => {
    const q = userQuery.trim().toLowerCase()
    if (!q) return users
    return users.filter((u) => {
      const role = (u.role ?? 'member').toLowerCase()
      const real = (realName(u) ?? '').toLowerCase()
      return u.username.toLowerCase().includes(q) || role.includes(q) || real.includes(q)
    })
  }, [users, userQuery])

  // Owner-linked accounts ("This is me (owner)"): identity only, never
  // privilege. Edit = owner token; read-only = other Admin+; Members never
  // reach this panel. Needs a server with Thread push (`thread-push-v1`).
  const ownerLink = threadPushAvailable() ? ownerLinkAccess(ownerToken, myRole) : 'hidden'
  const ownerAccounts = useOwnerAccounts(panel === 'people' && whoamiLoaded && ownerLink !== 'hidden')

  const running = status?.running ?? false
  // PRD tunnel-disable-unpair tri-state. Optional-safe: a pre-PRD daemon
  // omits both fields → plain running/stopped, exactly the old card.
  const tunnelDisabled = status?.enabled === false
  const tunnelReleased = status?.released === true
  const publicUrl = status?.public_url ?? (subdomain.trim() ? `https://${subdomain.trim()}.k2.dev` : null)
  const frpcMissing = status ? !status.frpc_installed : false
  // 0.45.2: a running tunnel whose encrypted listener serves another name
  // (the 0.45.0 switch bug) — the daemon reports both since 0.45.2.
  const listenerMismatch =
    running &&
    !!status?.e2eListener &&
    !!subdomain.trim() &&
    status.e2eListener.subdomain.toLowerCase() !== subdomain.trim().toLowerCase()

  // The custom dropdown's option values are the subdomain labels; map a
  // chosen value back to its full row so bindSubdomain gets the token.
  // Greys out (disables) any subdomain held by a *different* device whose
  // claim is still fresh; tags this device's own active claim.
  const subdomainOptions = subdomains.map((s) => {
    const heldByOther = !!s.claimed_by && s.claimed_by !== deviceId && freshClaim(s.claimed_at)
    const heldBySelf = s.claimed_by === deviceId && freshClaim(s.claimed_at)
    const base = s.status === 'active' ? `${s.label}.k2.dev` : `${s.label}.k2.dev (${s.status})`
    const suffix = heldByOther ? ' (in use on another device)' : heldBySelf ? ' (this device)' : ''
    return { value: s.label, label: `${base}${suffix}`, disabled: heldByOther }
  })

  // Bind the chosen subdomain into config WITHOUT starting (used when no
  // tunnel is running, and as the bind step inside the swap flow).
  const handleSubdomainPick = async (value: string): Promise<void> => {
    // Swap-confirm when a tunnel is already running and the pick differs
    // from the currently-bound subdomain.
    if (running && value !== subdomain.trim()) {
      const ok = await confirm({
        title: 'Swap tunnel?',
        message: `This will end your current tunnel at ${subdomain.trim()}.k2.dev and start ${value}.k2.dev.`,
        confirmLabel: 'Confirm',
      })
      if (!ok) {
        // Revert: nudge the controlled dropdown back to the current value.
        setSelectedLabel(subdomain.trim() || null)
        return
      }
      await swapTunnel(value)
      return
    }
    // Re-picking the running name while its listener holds another name's
    // certificate re-runs the switch for the same name (repairs in place).
    if (running && listenerMismatch && value === subdomain.trim()) {
      await swapTunnel(value)
      return
    }
    const row = subdomains.find((s) => s.label === value)
    if (row) await bindSubdomain(row)
  }

  // Swap an already-running tunnel to a different subdomain (0.45.2): claim
  // the new name FIRST (a refusal changes nothing), then ONE daemon call
  // (`/cli/tunnel/switch`) moves tunnel.json, the encrypted listener and its
  // certificate + renewer, the nested routes and frpc — and rolls back to the
  // old name if any step fails. Before 0.45.2 this stopped the tunnel and
  // overwrote the config before claiming, and the daemon kept serving the
  // old certificate until it was restarted.
  const swapTunnel = async (value: string): Promise<void> => {
    const row = subdomains.find((s) => s.label === value)
    if (!row) return
    const oldLabel = subdomain.trim()
    const accessToken = session?.accessToken ?? accessTokenRef.current
    setSwapBusy(true)
    setError(null)
    setBoundMsg(null)
    setSwitchProgress(null)
    try {
      const outcome = await runTunnelSwitch(
        {
          from: oldLabel,
          to: row.label,
          token: row.tunnel_token,
          deviceId,
          deviceLabel,
          start: true,
        },
        {
          claim: accessToken
            ? (label) => claimSubdomain(accessToken, label, deviceId, deviceLabel)
            : null,
          releaseLease: accessToken
            ? (label) => releaseSubdomain(accessToken, label, deviceId)
            : null,
          postSwitch: (body) => tunnelPost('switch', body),
        },
        setSwitchProgress,
      )
      if (outcome.ok) {
        setSelectedLabel(row.label)
        setBoundMsg(`Swapped to ${row.label}.k2.dev.`)
      } else {
        setError(outcome.error)
        // Nothing changed, or the daemon put the old name back.
        setSelectedLabel(oldLabel || null)
      }
      await loadConfig()
      await refreshStatus()
      void refreshSubdomains()
    } finally {
      setSwapBusy(false)
    }
  }

  return (
    <div className="w-full h-full min-h-0 flex flex-col">
      {/* Scroll owned by parent shell panes; avoid nested overflow here. */}
      <div className="space-y-5 pb-4">
        {/* ── Policies: host-scoped remote access (active daemon) ──
            Same SettingsGroup chrome as Account / URLs (title + left rule). */}
        {panel === 'policies' && (
          <SettingsGroup title="Policies">
            {myRole !== 'member' ? (
              <div className="space-y-1">
                <div className="flex items-center justify-between gap-3" data-settings-id="k2-connect.federation">
                  <div className="min-w-0">
                    <span className="text-xs text-[var(--color-text-secondary)]">
                      Enable federation (cross-server agents)
                    </span>
                    <span className="text-[10px] text-[var(--color-text-muted)] ml-2">{isRemote ? 'this server' : 'this device'}</span>
                  </div>
                  <Toggle
                    checked={federationEnabled}
                    onChange={(next) => void setFederationEnabled(next)}
                    aria-label="Enable federation"
                  />
                </div>
                <AllowRemoteInstructRow />
                <div
                  className="flex items-center justify-between gap-3"
                  data-settings-id="k2-connect.public-api"
                >
                  <div className="min-w-0">
                    <span className="text-xs text-[var(--color-text-secondary)]">
                      Enable public API (/v1)
                    </span>
                    <span className="text-[10px] text-[var(--color-text-muted)] ml-2">
                      {isRemote ? 'this server' : 'this device'} · takes effect immediately, no restart
                    </span>
                  </div>
                  <Toggle
                    checked={apiEnabled}
                    onChange={(next) => void setApiEnabled(next)}
                    aria-label="Enable public API"
                  />
                </div>
                <DnsManageEnabledRow />
                <AgentsCanCreateConnectionsRow />
                <ConnectLoginIngressRow />
                <BootStatusPublicRow />
              </div>
            ) : (
              <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
                Host policies are managed by an administrator.
              </p>
            )}
          </SettingsGroup>
        )}

        {/* ── Tunnel: account + expose + URLs nested map ── */}
        {panel === 'tunnel' && (
        <>
        {isRemote ? (
          <div className="px-3 py-2 border border-[var(--color-border)] text-[10px] text-[var(--color-text-muted)] leading-relaxed">
            K2 Connect tunneling is managed on the machine that owns the daemon. You&apos;re
            viewing{' '}
            <span className="text-[var(--color-text-secondary)]">
              {activeHost.label}
            </span>
            . Switch to <span className="text-[var(--color-text-secondary)]">This Mac</span> in
            the server switcher to configure your own tunnel.
          </div>
        ) : (
          <>
        {/* ── Account: sign-in + purchased-subdomain picker ──────────── */}
        <SettingsGroup
          title="Account"
          badge={
            <span className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]">
              beta
            </span>
          }
        >
          {!session ? (
            <div data-settings-id="k2-connect.account-login">
              <div className="flex flex-col mb-2">
                <span className="text-xs text-[var(--color-text-secondary)]">Sign in to K2 Connect</span>
                <span className="text-[10px] text-[var(--color-text-muted)]">
                  Sign in with your k2.dev account to pick a purchased subdomain automatically.
                </span>
              </div>
              <form
                className="space-y-2"
                onSubmit={(e) => {
                  e.preventDefault()
                  void handleSignIn()
                }}
              >
                <input
                  className={inputCls}
                  type="email"
                  autoComplete="email"
                  placeholder="you@example.com"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                />
                <input
                  className={inputCls}
                  type="password"
                  autoComplete="current-password"
                  placeholder="Password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                />
                {authError && (
                  <div className="text-[10px] text-[var(--color-status-error-soft)] px-2 py-1 border border-[color-mix(in_srgb,var(--color-status-error-soft)_20%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error-soft)_5%,transparent)]">{authError}</div>
                )}
                <button
                  type="submit"
                  disabled={authBusy || !email.trim() || !password}
                  className="px-3 py-1 text-[11px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
                >
                  {authBusy ? 'Signing in…' : 'Sign in'}
                </button>
              </form>
            </div>
          ) : (
            <>
              <SettingRow
                settingId="k2-connect.account-login"
                label={
                  <>
                    Signed in as{' '}
                    <span className="text-[var(--color-text-primary)]">{session.email || 'your account'}</span>
                  </>
                }
              >
                <button
                  onClick={() => void handleSignOut()}
                  className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:underline no-drag cursor-pointer"
                >
                  Sign out
                </button>
              </SettingRow>

              <SettingRow settingId="k2-connect.subdomain-list" label="Your subdomains">
                {subdomains.length === 0 ? (
                  <span className="text-[10px] text-[var(--color-text-muted)]">
                    No subdomains yet — buy one at{' '}
                    <a
                      href="https://k2.dev"
                      target="_blank"
                      rel="noreferrer"
                      className="text-[var(--color-accent)] hover:underline"
                    >
                      k2.dev
                    </a>
                    .
                  </span>
                ) : (
                  <SettingDropdown
                    value={selectedLabel ?? subdomain ?? ''}
                    options={subdomainOptions}
                    // Start EMPTY after sign-in — never pre-fill a default the
                    // user didn't choose. Picking is what binds the subdomain's
                    // token into config; a phantom "selected" default let users
                    // hit Start with no token bound ("tunnel not configured").
                    placeholder="Select a subdomain…"
                    onChange={(v) => void handleSubdomainPick(v)}
                  />
                )}
              </SettingRow>
              {boundMsg && (
                <div className="text-[10px] text-[var(--color-status-ok-soft)] py-1">{boundMsg}</div>
              )}
              {switchProgress && switchProgress.phase !== 'done' && (
                <div
                  className="text-[10px] py-1 space-y-0.5"
                  data-testid="tunnel-switch-progress"
                  aria-live="polite"
                >
                  <div
                    className={
                      switchProgress.phase === 'failed'
                        ? 'text-[var(--color-status-error-soft)]'
                        : 'text-[var(--color-text-secondary)]'
                    }
                  >
                    {switchProgress.phase === 'failed' ? 'Switch failed' : switchProgress.message}
                  </div>
                  {switchProgress.steps.map((s, i) => (
                    <div
                      key={`${s.step}-${i}`}
                      className={
                        s.ok
                          ? 'text-[var(--color-text-muted)]'
                          : 'text-[var(--color-status-error-soft)]'
                      }
                    >
                      {s.ok ? '✓' : '✗'} {s.detail}
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </SettingsGroup>

        {/* ── Live status — tri-state, never a spinner that hides which
            (PRD tunnel-disable-unpair §4): Connected / Disabled (by you)
            / Released (this device unpaired) / plain stopped. ────────── */}
        <div className="flex items-center gap-2 px-3 py-2 border border-[var(--color-border)]">
          <span
            className="w-2 h-2 flex-shrink-0 rounded-full"
            style={{
              backgroundColor: running
                ? 'var(--color-status-ok)'
                : tunnelReleased || tunnelDisabled
                  ? 'var(--color-status-warn-amber)'
                  : 'var(--color-neutral)',
            }}
          />
          <span className="text-xs text-[var(--color-text-secondary)]">
            {running
              ? 'Tunnel running'
              : tunnelReleased
                ? 'Released — this device is unpaired (re-pair to use a subdomain here)'
                : tunnelDisabled
                  ? 'Tunnel disabled (by you) — persists across restarts'
                  : 'Tunnel stopped'}
          </span>
          {running && publicUrl && (
            <a
              href={publicUrl}
              target="_blank"
              rel="noreferrer"
              className="ml-auto text-[10px] text-[var(--color-accent)] hover:underline font-mono truncate max-w-[220px]"
            >
              {publicUrl}
            </a>
          )}
        </div>

        {listenerMismatch && (
          <div className="text-[10px] px-3 py-1.5 border border-[color-mix(in_srgb,var(--color-status-warn-amber)_30%,transparent)] text-[var(--color-status-warn-amber-soft)]">
            The tunnel is {subdomain.trim()}.k2.dev but the encrypted listener still holds the{' '}
            {status?.e2eListener?.subdomain}.k2.dev certificate, so direct connections fail. Pick{' '}
            {subdomain.trim()} in the list above again to repair it (no restart needed).
          </div>
        )}

        {frpcMissing && (
          <div className="flex items-start gap-2 px-3 py-2 border border-[color-mix(in_srgb,var(--color-status-warn-amber)_30%,transparent)] bg-[color-mix(in_srgb,var(--color-status-warn-amber)_5%,transparent)]">
            <span className="text-[var(--color-status-warn-amber)] text-sm leading-none flex-shrink-0 mt-0.5">&#9888;</span>
            <div className="text-[10px] text-[color-mix(in_srgb,var(--color-status-warn-amber-soft)_80%,transparent)] leading-relaxed">
              <strong className="text-[var(--color-status-warn-amber-soft)]">frpc not installed.</strong>{' '}
              K2 Connect needs the <span className="font-mono">frpc</span> client on your PATH.
              Install it from <span className="font-mono">github.com/fatedier/frp/releases</span>{' '}
              (or via <span className="font-mono">brew install frpc</span>) and try again.
            </div>
          </div>
        )}

        {error && <div className="text-[10px] text-[var(--color-status-error-soft)] px-3 py-1.5 border border-[color-mix(in_srgb,var(--color-status-error-soft)_20%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error-soft)_5%,transparent)]">{error}</div>}

        {/* ── Auto-start toggle (left) + Start/Stop (right), one row,
            above the Advanced/manual config ──────────────────────────── */}
        <div className="flex items-center justify-between gap-3" data-settings-id="k2-connect.start-stop">
          <label className="flex items-center gap-2 cursor-pointer select-none no-drag">
            <input
              type="checkbox"
              checked={autoStart}
              onChange={(e) => void toggleAutoStart(e.target.checked)}
              className="peer sr-only"
            />
            <span
              aria-hidden="true"
              className="w-3 h-3 flex-shrink-0 flex items-center justify-center border transition-colors border-[var(--color-border)] bg-[var(--color-bg-elevated)] peer-checked:bg-[var(--color-accent)] peer-checked:border-[var(--color-accent)] peer-focus-visible:ring-1 peer-focus-visible:ring-[var(--color-accent)]"
            >
              {autoStart && (
                <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="var(--color-on-accent)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M2.5 6.5 L5 9 L9.5 3.5" />
                </svg>
              )}
            </span>
            <span className="text-xs text-[var(--color-text-secondary)]">Re-launch this tunnel on restart</span>
          </label>
          {running ? (
            <button
              onClick={() => void stopTunnel()}
              disabled={busy || swapBusy}
              className="px-3 py-1 text-[11px] text-[var(--color-on-accent)] bg-[color-mix(in_srgb,var(--color-status-error)_80%,transparent)] hover:bg-[var(--color-status-error)] no-drag cursor-pointer disabled:opacity-60"
            >
              {swapBusy ? 'Swapping…' : 'Stop tunnel'}
            </button>
          ) : (
            <button
              onClick={() => void startTunnel()}
              disabled={busy || swapBusy}
              className="px-3 py-1 text-[11px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
            >
              Start tunnel
            </button>
          )}
        </div>

        {/* ── Release subdomain from this device (PRD tunnel-disable-
            unpair §2B) — destructive, confirm-gated. Only offered while
            an identity exists to release (token bound, not yet
            released). ─────────────────────────────────────────────────── */}
        {tokenSet && !tunnelReleased && (
          <div className="flex items-center justify-end gap-4" data-settings-id="k2-connect.release-device">
            <button
              onClick={() => void releaseFromDevice('forget')}
              disabled={busy || swapBusy}
              title="Delete this device's tunnel settings without telling K2 Connect (settings copied from another machine)"
              className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:underline no-drag cursor-pointer disabled:opacity-60"
            >
              Forget this tunnel on this device…
            </button>
            <button
              onClick={() => void releaseFromDevice('release')}
              disabled={busy || swapBusy}
              className="text-[10px] text-[var(--color-status-error-soft)] hover:text-[var(--color-status-error)] hover:underline no-drag cursor-pointer disabled:opacity-60"
            >
              Release subdomain from this device…
            </button>
          </div>
        )}

        <div className="text-[10px] text-[var(--color-text-muted)] space-y-1">
          <p>1. Sign in to k2.dev above and pick a subdomain you own (e.g. <span className="font-mono">alice</span>).</p>
          <p>2. Start the tunnel — your daemon becomes reachable at <span className="font-mono">https://&lt;sub&gt;.k2.dev</span>.</p>
          <p>3. Add that address as a server on another computer (Settings → Connections), or pair the K2 Companion app.</p>
        </div>

        {/* ── Advanced / manual config (fallback) — below the instructions ── */}
        <div>
          <button
            onClick={() => setShowAdvanced((v) => !v)}
            className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] no-drag cursor-pointer mb-2"
          >
            {showAdvanced ? '▾' : '▸'} Advanced / manual config
          </button>
          {showAdvanced && (
            <SettingsGroup title="Advanced / manual config">
              <SettingRow settingId="k2-connect.subdomain" label="Subdomain">
                <input
                  className={inputCls}
                  style={{ maxWidth: 200 }}
                  placeholder="e.g. alice"
                  value={subdomain}
                  onChange={(e) => setSubdomain(e.target.value)}
                />
              </SettingRow>
              <SettingRow
                settingId="k2-connect.token"
                label={
                  <>
                    Token
                    {tokenSet && <span className="ml-2 text-[10px] text-[var(--color-status-ok-soft)]">Set</span>}
                  </>
                }
              >
                <input
                  className={inputCls}
                  style={{ maxWidth: 200 }}
                  type="password"
                  placeholder={tokenSet ? '•••••••• (leave blank to keep)' : 'K2 Connect token'}
                  value={token}
                  onChange={(e) => setToken(e.target.value)}
                />
              </SettingRow>
              <SettingRow settingId="k2-connect.server-addr" label="Tunnel Server">
                <div className="flex gap-1.5">
                  <input
                    className={inputCls}
                    style={{ maxWidth: 150 }}
                    placeholder={RELAY_PLACEHOLDER}
                    value={serverAddr}
                    onChange={(e) => {
                      setServerAddr(e.target.value)
                      setRelayEdited(true)
                    }}
                  />
                  <input
                    className={inputCls}
                    style={{ maxWidth: 64 }}
                    placeholder={PORT_PLACEHOLDER}
                    value={serverPort}
                    onChange={(e) => {
                      setServerPort(e.target.value)
                      setRelayEdited(true)
                    }}
                  />
                </div>
              </SettingRow>
              <div className="flex items-center gap-2 pt-2">
                <button
                  onClick={() => void saveConfig()}
                  disabled={busy}
                  className="px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-60"
                >
                  {busy ? 'Saving…' : 'Save'}
                </button>
                {savedMsg && <span className="text-[10px] text-[var(--color-status-ok-soft)]">{savedMsg}</span>}
              </div>
            </SettingsGroup>
          )}
        </div>
          </>
        )}

        {/* URLs on Tunnel for both This Mac and remote active host */}
        <SettingsGroup title="URLs">
          <div data-settings-id="k2-connect.urls">
            <TunnelUrlsPanel canManage={canManagePublishedSubdomains(isRemote, myRole)} />
          </div>
        </SettingsGroup>
        </>
        )}

        {/* ── People: Users / Access — role-gated multi-user list (#617 / #629) ─── */}
        {panel === 'people' && (
        <SettingsGroup title="Users / Access">
          <div data-settings-id="k2-connect.users" className="space-y-3">
            {/* K2 #629: only viewers who can manage users (Owner|Admin)
                see the management surface. A Member viewer (or an
                unresolved/forbidden whoami) gets a clean note — never the
                raw token/403 error. */}
            {whoamiLoaded && !canManageUsers(myRole) ? (
              <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed py-1">
                {oldHostNoRoles
                  ? `This host (v${serverVersion}) predates roles — update it to v${featureMinVersion('roles')} to manage users here.`
                  : 'User management is handled by an administrator.'}
              </p>
            ) : !whoamiLoaded ? (
              <p className="text-[10px] text-[var(--color-text-muted)] py-1">Loading…</p>
            ) : (
            <>
            <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
              People you allow to connect IN to this device&apos;s daemon. You set each
              person&apos;s username + initial password; they sign in from their K2 app (or
              change their own password at <span className="font-mono">https://&lt;sub&gt;.k2.dev</span>).
              You can&apos;t view a password after setting it — only reset it.
            </p>

            {/* Password requirements (K2 #620) — server-enforced policy */}
            <div className="border border-[var(--color-border)] p-2.5 space-y-2">
              <div className="flex items-center justify-between gap-3">
                <span className="text-[11px] text-[var(--color-text-secondary)] font-medium">
                  Password requirements
                </span>
                <span className="flex items-center gap-2">
                  <label className="flex items-center gap-1.5 text-[10px] text-[var(--color-text-muted)]">
                    Min length
                    <input
                      className={inputCls}
                      // color-scheme: dark renders the native number-stepper
                      // carrots light/white instead of the default dark.
                      style={{ maxWidth: 56, colorScheme: 'dark' }}
                      type="number"
                      min={4}
                      max={128}
                      value={policyMinLength}
                      onChange={(e) => setPolicyMinLength(e.target.value)}
                    />
                  </label>
                </span>
              </div>
              <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
                {([
                  ['special character', policyRequireSpecial, setPolicyRequireSpecial],
                  ['number', policyRequireNumber, setPolicyRequireNumber],
                  ['uppercase letter', policyRequireUppercase, setPolicyRequireUppercase],
                ] as [string, boolean, (v: boolean) => void][]).map(([label, checked, setVal]) => (
                  <label key={label} className="flex items-center gap-1.5 cursor-pointer select-none no-drag">
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={(e) => setVal(e.target.checked)}
                      className="peer sr-only"
                    />
                    <span
                      aria-hidden="true"
                      className="w-3 h-3 flex-shrink-0 flex items-center justify-center border transition-colors border-[var(--color-border)] bg-[var(--color-bg-elevated)] peer-checked:bg-[var(--color-accent)] peer-checked:border-[var(--color-accent)] peer-focus-visible:ring-1 peer-focus-visible:ring-[var(--color-accent)]"
                    >
                      {checked && (
                        <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="var(--color-on-accent)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                          <path d="M2.5 6.5 L5 9 L9.5 3.5" />
                        </svg>
                      )}
                    </span>
                    <span className="text-[10px] text-[var(--color-text-secondary)]">Require {label}</span>
                  </label>
                ))}
              </div>
              <div className="flex items-center gap-2 pt-0.5">
                <button
                  onClick={() => void savePolicy()}
                  className="px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer"
                >
                  Save requirements
                </button>
                {policySavedMsg && <span className="text-[10px] text-[var(--color-status-ok-soft)]">{policySavedMsg}</span>}
                {policyError && <span className="text-[10px] text-[var(--color-status-error-soft)]">{policyError}</span>}
              </div>
            </div>

            {/* Add-user form */}
            <form
              className="flex flex-col gap-2"
              onSubmit={(e) => {
                e.preventDefault()
                void addUser()
              }}
            >
              <div className="flex flex-wrap gap-1.5 w-full items-center">
                <input
                  className={`${inputCls} flex-1 min-w-[8rem]`}
                  placeholder="username"
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  value={newUsername}
                  onChange={(e) => setNewUsername(e.target.value)}
                />
                <input
                  className={`${inputCls} flex-1 min-w-[7rem]`}
                  placeholder="first name (optional)"
                  aria-label="New user first name"
                  maxLength={PERSON_NAME_MAX}
                  value={newFirstName}
                  onChange={(e) => setNewFirstName(e.target.value)}
                />
                <input
                  className={`${inputCls} flex-1 min-w-[7rem]`}
                  placeholder="last name (optional)"
                  aria-label="New user last name"
                  maxLength={PERSON_NAME_MAX}
                  value={newLastName}
                  onChange={(e) => setNewLastName(e.target.value)}
                />
                <div className="relative flex-1 min-w-[8rem]">
                  <input
                    className={`${inputCls} w-full pr-7`}
                    type={showNewPassword ? 'text' : 'password'}
                    autoComplete="new-password"
                    placeholder="initial password"
                    value={newPassword}
                    onChange={(e) => setNewPassword(e.target.value)}
                  />
                  <button
                    type="button"
                    onClick={() => setShowNewPassword((v) => !v)}
                    title={showNewPassword ? 'Hide password' : 'Show password'}
                    aria-label={showNewPassword ? 'Hide password' : 'Show password'}
                    className="absolute inset-y-0 right-1.5 flex items-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
                  >
                    {showNewPassword ? (
                      <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
                    ) : (
                      <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>
                    )}
                  </button>
                </div>
                {canChangeRoles(myRole) && (
                  <SettingDropdown
                    value={newRole}
                    options={[...ROLE_OPTIONS]}
                    ariaLabel="New user role"
                    onChange={(value) => {
                      const r = parseK2Role(value)
                      if (r) setNewRole(r)
                    }}
                    menuAlign="left"
                  />
                )}
                <button
                  type="submit"
                  disabled={addBusy || !newUsername.trim() || !newPassword}
                  className="flex-shrink-0 px-3 py-1 text-[11px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
                >
                  {addBusy ? 'Adding…' : 'Add user'}
                </button>
              </div>
              {addError && (
                <div className="text-[10px] text-[var(--color-status-error-soft)] px-2 py-1 border border-[color-mix(in_srgb,var(--color-status-error-soft)_20%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error-soft)_5%,transparent)]">{addError}</div>
              )}
              {addedMsg && <div className="text-[10px] text-[var(--color-status-ok-soft)]">{addedMsg}</div>}
            </form>

            {usersError && (
              <div className="text-[10px] text-[var(--color-status-error-soft)] px-2 py-1 border border-[color-mix(in_srgb,var(--color-status-error-soft)_20%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error-soft)_5%,transparent)]">{usersError}</div>
            )}
            {resetMsg && <div className="text-[10px] text-[var(--color-status-ok-soft)]">{resetMsg}</div>}

            {/* User list */}
            <input
              type="search"
              value={userQuery}
              onChange={(e) => setUserQuery(e.target.value)}
              placeholder="Search users"
              aria-label="Search users"
              className={inputCls}
            />
            {ownerAccounts.error && (
              <div className="text-[10px] text-[var(--color-status-error-soft)] px-2 py-1 border border-[color-mix(in_srgb,var(--color-status-error-soft)_20%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error-soft)_5%,transparent)]">{ownerAccounts.error}</div>
            )}
            {usersLoaded && users.length === 0 ? (
              <div className="text-[10px] text-[var(--color-text-muted)] py-1">
                No users yet — add one above.
              </div>
            ) : usersLoaded && visibleUsers.length === 0 ? (
              <div className="text-[10px] text-[var(--color-text-muted)] py-1">
                No users match.
              </div>
            ) : (
              <div className="divide-y divide-[var(--color-border)]">
                {visibleUsers.map((u) => (
                  <div key={u.username} className="py-2 space-y-2">
                    <div className="flex items-center justify-between gap-3">
                      <span className="flex items-center gap-2 min-w-0">
                        {realName(u) ? (
                          <span className="text-xs text-[var(--color-text-primary)] truncate" data-testid={`user-real-name-${u.username}`}>
                            {realName(u)}
                          </span>
                        ) : null}
                        <span
                          className={`text-xs font-mono truncate ${realName(u) ? 'text-[var(--color-text-muted)]' : 'text-[var(--color-text-primary)]'}`}
                        >
                          {u.username}
                        </span>
                        {u.disabled && (
                          <span className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-text-muted)]/15 text-[var(--color-text-muted)]">
                            disabled
                          </span>
                        )}
                        {u.wasViewer && <WasViewerBadge />}
                      </span>
                      <div className="flex items-center gap-3 flex-shrink-0">
                        {/* K2 #629 — role selector. Editable only for an
                            Owner viewer; Admins see the role read-only. */}
                        {canChangeRoles(myRole) ? (
                          <SettingDropdown
                            value={u.role ?? 'member'}
                            // No Viewer option. An older server can still
                            // report "viewer"; it shows as the placeholder
                            // and can be changed to a real role.
                            placeholder={u.role ?? 'member'}
                            options={[...ROLE_OPTIONS]}
                            ariaLabel={`${u.username} role`}
                            onChange={(value) => {
                              const r = parseK2Role(value)
                              if (r) void changeRole(u.username, r)
                            }}
                          />
                        ) : (
                          <span className="text-[9px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-text-secondary)]">
                            {u.role ?? 'member'}
                          </span>
                        )}
                        <CanCloneToggle
                          username={u.username}
                          rowRole={u.role}
                          canClone={u.canClone}
                          myRole={myRole}
                          onChange={(v) => {
                            setUsersError(null)
                            void changeCanClone(u.username, v)
                          }}
                        />
                        {/* Disable / Enable — peer-checked checkbox (matches auto-start) */}
                        <label className="flex items-center gap-1.5 cursor-pointer select-none no-drag">
                          <input
                            type="checkbox"
                            checked={!u.disabled}
                            onChange={(e) => void toggleDisabled(u.username, !e.target.checked)}
                            className="peer sr-only"
                          />
                          <span
                            aria-hidden="true"
                            className="w-3 h-3 flex-shrink-0 flex items-center justify-center border transition-colors border-[var(--color-border)] bg-[var(--color-bg-elevated)] peer-checked:bg-[var(--color-accent)] peer-checked:border-[var(--color-accent)] peer-focus-visible:ring-1 peer-focus-visible:ring-[var(--color-accent)]"
                          >
                            {!u.disabled && (
                              <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="var(--color-on-accent)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                                <path d="M2.5 6.5 L5 9 L9.5 3.5" />
                              </svg>
                            )}
                          </span>
                          <span className="text-[10px] text-[var(--color-text-secondary)]">Enabled</span>
                        </label>
                        <button
                          data-testid={`edit-name-${u.username}`}
                          onClick={() => {
                            setNameFor((cur) => (cur === u.username ? null : u.username))
                            setNameDraft({ firstName: u.firstName ?? '', lastName: u.lastName ?? '' })
                          }}
                          className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:underline no-drag cursor-pointer"
                        >
                          Edit name
                        </button>
                        <button
                          onClick={() => {
                            setResetFor((cur) => (cur === u.username ? null : u.username))
                            setResetPassword('')
                          }}
                          className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:underline no-drag cursor-pointer"
                        >
                          Reset password
                        </button>
                        {removeConfirm === u.username ? (
                          <span className="flex items-center gap-1.5">
                            <button
                              onClick={() => void removeUser(u.username)}
                              className="text-[10px] text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
                            >
                              Confirm remove
                            </button>
                            <button
                              onClick={() => setRemoveConfirm(null)}
                              className="text-[10px] text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer"
                            >
                              Cancel
                            </button>
                          </span>
                        ) : (
                          <button
                            onClick={() => setRemoveConfirm(u.username)}
                            className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
                          >
                            Remove
                          </button>
                        )}
                      </div>
                    </div>
                    {/* 0.46.0: the owner picks their login in Settings →
                        General → Your account; the row only shows the tag. */}
                    {ownerLink !== 'hidden' && ownerAccounts.loaded && (
                      <OwnerAccountTag username={u.username} linked={ownerAccounts.isLinked(u.username)} />
                    )}
                    {needsEnableAsMember(u) && (
                      <WasViewerNotice
                        username={u.username}
                        busy={enableBusy === u.username}
                        onEnableAsMember={(name) => void enableAsMember(name)}
                      />
                    )}
                    {nameFor === u.username && (
                      <form
                        className="flex flex-wrap items-center gap-1.5"
                        data-testid={`name-form-${u.username}`}
                        onSubmit={(e) => {
                          e.preventDefault()
                          void submitName(u.username)
                        }}
                      >
                        <input
                          className={inputCls}
                          style={{ maxWidth: 160 }}
                          aria-label={`${u.username} first name`}
                          placeholder="first name"
                          maxLength={PERSON_NAME_MAX}
                          value={nameDraft.firstName}
                          onChange={(e) => setNameDraft((d) => ({ ...d, firstName: e.target.value }))}
                        />
                        <input
                          className={inputCls}
                          style={{ maxWidth: 160 }}
                          aria-label={`${u.username} last name`}
                          placeholder="last name"
                          maxLength={PERSON_NAME_MAX}
                          value={nameDraft.lastName}
                          onChange={(e) => setNameDraft((d) => ({ ...d, lastName: e.target.value }))}
                        />
                        <button
                          type="submit"
                          disabled={nameBusy}
                          className="px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-60"
                        >
                          {nameBusy ? 'Saving…' : 'Save name'}
                        </button>
                        <button
                          type="button"
                          onClick={() => setNameFor(null)}
                          className="text-[10px] text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer"
                        >
                          Cancel
                        </button>
                      </form>
                    )}
                    {resetFor === u.username && (
                      <form
                        className="flex items-center gap-1.5"
                        onSubmit={(e) => {
                          e.preventDefault()
                          void submitReset(u.username)
                        }}
                      >
                        <input
                          className={inputCls}
                          style={{ maxWidth: 180 }}
                          type="password"
                          autoComplete="new-password"
                          placeholder="new password"
                          value={resetPassword}
                          onChange={(e) => setResetPassword(e.target.value)}
                        />
                        <button
                          type="submit"
                          disabled={resetBusy || !resetPassword}
                          className="px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-60"
                        >
                          {resetBusy ? 'Saving…' : 'Set password'}
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            setResetFor(null)
                            setResetPassword('')
                          }}
                          className="text-[10px] text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer"
                        >
                          Cancel
                        </button>
                      </form>
                    )}
                  </div>
                ))}
              </div>
            )}
            {/* Sew S32 K28: read-only Sew state per Connect login, only where
                its own `connect` chat grant exists. No provisioning here. */}
            <SewAdminAccessState users={users} />
            </>
            )}
          </div>
        </SettingsGroup>
        )}
      </div>
    </div>
  )
}
