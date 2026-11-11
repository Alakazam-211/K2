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

import React, { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { getLocalDaemonWs, daemonHttpBase, invalidateDaemonWs } from '@/kessel/daemon-ws'
import { withRemoteRetry } from '@/lib/remote-retry'
import type { SettingEntry } from '../searchManifest'
import {
  SettingRow,
  SettingsGroup,
  SettingDropdown,
  SettingsLoadState,
  SaveNote,
  SaveError,
  useSaveStatus,
  SETTING_FIELD_WIDTH,
} from '../controls/SettingControls'
import { summarizeTunnel, hostOf } from './tunnel-status'
import { HealthWord, StatusDot } from './ConnectionsParts'
import { parseK2Role, type K2Role } from './connect-user-roles'
import { AdminAccessPanel } from './AdminAccessPanel'
import { errText, whoamiGet } from './admin-access-api'
import { BetaTag, Toggle, Button, Callout, Input } from '@/components/ui'
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
  { id: 'k2-connect.start-stop', section: 'k2-connect', label: 'Tunnel status', description: 'Whether this server can be reached from the internet, at which address; turn the tunnel on or off', keywords: ['start', 'stop', 'turn on', 'turn off', 'tunnel', 'expose', 'connect', 'reachable', 'status'] },
  { id: 'k2-connect.auto-start', section: 'k2-connect', label: 'Turn on again after a restart', description: 'The tunnel comes back on by itself when K2 or this computer restarts', keywords: ['auto', 'autostart', 'restart', 'reconnect', 'boot', 'relaunch'] },
  { id: 'k2-connect.account-login', section: 'k2-connect', label: 'k2.dev account', description: 'Sign in to choose the address this server uses', keywords: ['login', 'sign in', 'account', 'k2.dev', 'email', 'password'] },
  { id: 'k2-connect.subdomain-list', section: 'k2-connect', label: 'Address', description: 'The k2.dev address this server uses on the internet', keywords: ['subdomain', 'address', 'purchased', 'list', 'bind', 'select', 'switch'] },
  { id: 'k2-connect.urls', section: 'k2-connect', label: 'Addresses', description: "This server's address and the sites it publishes under it", keywords: ['urls', 'nested', 'subdomains', 'sites', 'routing', 'targets', 'ports', 'workspace', 'publish'] },
  { id: 'k2-connect.subdomain', section: 'k2-connect', label: 'Set up by hand', description: 'Enter an address, token and relay yourself instead of signing in', keywords: ['subdomain', 'manual', 'advanced', 'tunnel', 'k2.dev', 'host'] },
  { id: 'k2-connect.token', section: 'k2-connect', label: 'Tunnel token', description: 'The token for setting the tunnel up by hand', keywords: ['token', 'tunnel', 'frpc', 'auth', 'manual'] },
  { id: 'k2-connect.server-addr', section: 'k2-connect', label: 'Relay and port', description: 'The relay the tunnel connects to, when set up by hand', keywords: ['server', 'frps', 'relay', 'address', 'port', 'endpoint'] },
  { id: 'k2-connect.release-device', section: 'k2-connect', label: 'Release or forget the tunnel', description: 'Remove the tunnel from this computer', keywords: ['release', 'forget', 'unpair', 'remove', 'delete', 'tunnel'] },
  { id: 'k2-connect.users', section: 'k2-access', label: 'People who can sign in', description: 'Add a person to this server, change their role, reset a password, or remove them', keywords: ['users', 'access', 'people', 'login', 'password', 'connect in', 'multi-user', 'allow', 'invite', 'add person', 'role', 'admin', 'owner', 'member', 'remove'] },
  { id: 'k2-access.password-rules', section: 'k2-access', label: 'Password rules', description: 'What a password on this server must have: length, a number, an uppercase letter, a special character', keywords: ['password', 'policy', 'requirements', 'length', 'rules', 'strength'] },
  { id: 'k2-connect.federation', section: 'k2-connect', label: 'Connect agents across servers', description: 'Agents here can message agents on the servers you add under Connected Servers', keywords: ['federation', 'cross-server', 'peers', 'agents', 'connect', 'mesh'] },
  { id: 'k2-connect.allow-remote-instruct', section: 'k2-connect', label: 'Let agents on other servers message every agent', description: 'List every agent to connected servers and let their agents message it. People who log in here can always message agents.', keywords: ['remote', 'instruct', 'message', 'agents', 'other servers', 'consent', 'federation', 'inbound', 'roster', 'allow'] },
  { id: 'k2-connect.public-api', section: 'k2-connect', label: 'Public API', description: 'Apps with a key from API Keys can use this server. Takes effect right away.', keywords: ['api', 'v1', 'public api', 'rest', 'http', 'external', 'surface', 'K2_API', 'api keys', 'host sessions'] },
  { id: 'k2-connect.login-ingress', section: 'k2-connect', label: 'Password sign-in from the internet', description: 'Only at its k2.dev address (recommended), from any app, or off', keywords: ['login', 'sign in', 'password', 'tunnel', 'edge', 'ingress', 'scanner', 'brute force', 'connect users', 'app.k2.dev'] },
  { id: 'k2-connect.boot-status-public', section: 'k2-connect', label: "Hide this server's details from the internet", description: 'Visitors who are not signed in see only that the server is up, not its version or features', keywords: ['boot-status', 'ghost', 'hide', 'version', 'fingerprint', 'scanner', 'privacy', 'stealth', 'ping'] },
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
// active remote host — when the user is remoted into another box (a customer's, etc.)
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
  // Why the last status read failed (the card shows it with Try again).
  const [statusError, setStatusError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // A Turn on in flight, and why the last one failed: the status card
  // shows both (Connecting…, Couldn't turn on).
  const [starting, setStarting] = useState(false)
  const [startError, setStartError] = useState<string | null>(null)
  // Any other tunnel action that failed: what failed, in plain words, and
  // the server's reason.
  const [error, setErrorState] = useState<{ what: string; detail: string } | null>(null)
  const setError = (detail: string | null, what = "Couldn't change the tunnel."): void =>
    setErrorState(detail === null ? null : { what, detail })
  const manualSave = useSaveStatus()
  const autoStartSave = useSaveStatus()

  // ── Account / subdomain-picker state ──────────────────────────────────
  const [session, setSession] = useState<K2Session | null>(null)
  const [subdomains, setSubdomains] = useState<K2Subdomain[]>([])
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [authBusy, setAuthBusy] = useState(false)
  const [authError, setAuthError] = useState<{ what: string; detail: string } | null>(null)
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
  // K2 #629: the viewer's role on the server this window is on (whoami).
  // It gates Policies and the published-sites actions on Tunnel. Admin
  // Access (AdminAccessPanel) reads its own.
  const [myRole, setMyRole] = useState<K2Role | null>(null)

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
    } else if (panel === 'policies') {
      // Active-host whoami (remote-aware): Members see a note instead.
      void refreshWhoami()
    }
    // panel === 'people': AdminAccessPanel loads its own whoami and list.

    return () => {
      cancelled = true
      if (interval) clearInterval(interval)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [panel, isRemote])

  const refreshStatus = async (): Promise<void> => {
    try {
      const res = await tunnelGet('status')
      if (res.ok) {
        setStatus((await res.json()) as TunnelStatus)
        setStatusError(null)
      } else {
        setStatusError(await errText(res))
      }
    } catch (e) {
      setStatusError(e instanceof Error ? e.message : String(e))
    }
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
        return role
      }
      setMyRole(null)
      return null
    } catch {
      setMyRole(null)
      return null
    }
  }

  const saveConfig = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    manualSave.reset()
    const built = manualConfigBody({ serverAddr, serverPort, subdomain, token, relayEdited })
    if (!built.ok) {
      manualSave.failed(built.error)
      setBusy(false)
      return
    }
    const body = built.body
    try {
      const res = await tunnelPost('config', body)
      if (!res.ok) {
        manualSave.failed(await errText(res))
        return
      }
      const cfg = (await res.json()) as TunnelConfigView
      setTokenSet(cfg.tokenSet)
      setToken('') // never keep the secret in the field after save
      setServerAddr(cfg.serverAddr || '')
      setServerPort(cfg.serverPort ? String(cfg.serverPort) : '')
      setRelayEdited(false)
      manualSave.saved()
    } catch (e) {
      manualSave.failed(e)
    } finally {
      setBusy(false)
    }
  }

  const startTunnel = async (): Promise<void> => {
    setBusy(true)
    setStarting(true)
    setError(null)
    setStartError(null)
    try {
      const sub = subdomain.trim()
      // Mirror the daemon's is_connectable() locally so an unconfigured Start
      // fails with a clear, actionable message instead of a round-trip
      // "tunnel not configured: no token". Picking a subdomain above is what
      // binds its token — with neither a picked subdomain nor a manually-set
      // token, there is nothing to start.
      if (!sub && !tokenSet) {
        setStartError('Choose an address first, or set one up by hand under Advanced.')
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
            setStartError(`${sub}.k2.dev is in use on another computer. Turn its tunnel off there first.`)
            return
          }
        } catch (e) {
          setStartError(e instanceof Error ? e.message : String(e))
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
        setStartError(await errText(res))
        // Don't keep the lease we just took if the tunnel didn't start.
        if (sub && accessToken) void releaseSubdomain(accessToken, sub, deviceId).catch(() => undefined)
        return
      }
      setBoundMsg(null)
      await refreshStatus()
      // Lease renewal is now the DAEMON's job (tied to the tunnel being up).
    } catch (e) {
      setStartError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
      setStarting(false)
    }
  }

  const stopTunnel = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    setStartError(null)
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
        setError(await errText(res), "Couldn't turn the tunnel off.")
        return
      }
      setBoundMsg(null)
      await refreshStatus()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e), "Couldn't turn the tunnel off.")
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
    const label = sub ? `${sub}.k2.dev` : 'its address'
    const ok =
      mode === 'forget'
        ? await confirm({
            title: 'Forget the tunnel on this computer?',
            message:
              `This computer stops using ${label} and deletes its tunnel settings. ` +
              `Nothing is sent to k2.dev, so another computer using ${label} keeps it. ` +
              `Use this for settings copied from another computer.`,
            confirmLabel: 'Forget',
            destructive: true,
          })
        : await confirm({
            title: `Release ${label} from this computer?`,
            message:
              `${label} stops reaching this computer, and these tunnel settings can't be used again. ` +
              (running
                ? `k2.dev is told this computer let go of it. `
                : `The tunnel is off, so nothing is sent to k2.dev. `) +
              `Your account keeps the address for another computer. To pause instead, turn the tunnel off.`,
            confirmLabel: 'Release',
            destructive: true,
          })
    if (!ok) return
    const what = mode === 'forget' ? "Couldn't forget the tunnel settings." : `Couldn't release ${label}.`
    setBusy(true)
    setError(null)
    setSwitchProgress(null)
    try {
      const res = await tunnelPost(mode === 'forget' ? 'release?confirm=1&mode=forget' : 'release?confirm=1')
      if (!res.ok) {
        setError(await errText(res), what)
        return
      }
      const report = (await res.json()) as { note?: string; localOnly?: boolean; upstreamReported?: boolean }
      setBoundMsg(
        mode === 'forget'
          ? 'Forgot the tunnel settings on this computer.'
          : report.upstreamReported
            ? `Released ${label}. k2.dev knows this computer let go of it.`
            : (report.note ?? `Released ${label} on this computer.`),
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
      setError(e instanceof Error ? e.message : String(e), what)
    } finally {
      setBusy(false)
    }
  }

  // Toggle the daemon-side "re-launch on restart" opt-in. POSTs only
  // { autoStart } so the saved token / subdomain are untouched. Optimistic
  // flip with revert-on-failure.
  const toggleAutoStart = async (next: boolean): Promise<void> => {
    setAutoStart(next)
    autoStartSave.reset()
    try {
      const res = await tunnelPost('config', { autoStart: next })
      if (!res.ok) {
        setAutoStart(!next) // revert
        autoStartSave.failed(await errText(res))
        return
      }
      const cfg = (await res.json()) as TunnelConfigView
      setAutoStart(cfg.autoStart ?? next)
      autoStartSave.saved()
    } catch (e) {
      setAutoStart(!next) // revert
      autoStartSave.failed(e)
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
        setAuthError({ what: "Signed in, but couldn't load your addresses.", detail: e instanceof Error ? e.message : String(e) })
      }
    } catch (e) {
      setAuthError({ what: "Couldn't sign in. Check your email and password.", detail: e instanceof Error ? e.message : String(e) })
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
    setStartError(null)
    const what = `Couldn't switch to ${sub.label}.k2.dev.`
    try {
      const res = await tunnelPost('config', bindConfigBody(sub, deviceId, deviceLabel ?? ''))
      if (!res.ok) {
        setError(await errText(res), what)
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
        setError(cfg.identityChange.error ?? 'a step failed', `${sub.label}.k2.dev is saved, but moving the tunnel to it didn't finish.`)
        return
      }
      setBoundMsg(running ? `This server now uses ${sub.label}.k2.dev.` : `This server will use ${sub.label}.k2.dev. Turn the tunnel on to reach it there.`)
      void refreshStatus()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e), what)
    } finally {
      setBusy(false)
    }
  }


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
        title: `Switch to ${value}.k2.dev?`,
        message: `The tunnel moves from ${subdomain.trim()}.k2.dev to ${value}.k2.dev. Apps signed in at the old address need the new one.`,
        confirmLabel: 'Switch',
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
        setBoundMsg(`This server now uses ${row.label}.k2.dev.`)
      } else {
        setError(outcome.error, `Couldn't switch to ${row.label}.k2.dev. The tunnel still uses ${oldLabel}.k2.dev.`)
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

  // ── Tunnel page: what the status card says, and its parts ─────────────
  const configured = tokenSet || !!subdomain.trim()
  const summary = summarizeTunnel({
    loaded: status !== null,
    readError: statusError,
    running,
    enabled: status?.enabled,
    released: status?.released,
    frpcInstalled: status ? status.frpc_installed !== false : true,
    tokenSet,
    subdomain,
    signedIn: !!session,
    hasAddresses: subdomains.length > 0,
    starting: starting || swapBusy,
    startError,
    listenerName: listenerMismatch ? (status?.e2eListener?.subdomain ?? null) : null,
  })
  const showAddress = !!publicUrl && (summary.state === 'reachable' || summary.state === 'repair')
  const [copied, setCopied] = useState(false)
  const copyAddress = async (url: string): Promise<void> => {
    try {
      await navigator.clipboard.writeText(url)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e), "Couldn't copy the address.")
    }
  }
  const reloadTunnel = async (): Promise<void> => {
    setStatusError(null)
    await loadConfig()
    await refreshStatus()
  }
  const errorCallout = error ? (
    <Callout tone="error" role="alert">
      <p className="type-hint">{error.what}</p>
      <p className="type-hint text-[var(--color-text-secondary)]">
        The server said: {error.detail.replace(/[.\s]+$/, '')}.
      </p>
    </Callout>
  ) : null
  const doneNote = boundMsg ? (
    <p role="status" className="inline-flex items-center gap-1 type-hint text-[var(--color-status-ok-soft)]">
      <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3 flex-shrink-0">
        <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
      </svg>
      {boundMsg}
    </p>
  ) : null
  // Under the status: what failed, or what the last change did.
  const statusBelow = errorCallout ?? doneNote ?? undefined
  // Under the address: a switch's live steps.
  const addressBelow =
    switchProgress && switchProgress.phase !== 'done' ? (
      <div className="type-hint space-y-0.5" data-testid="tunnel-switch-progress" aria-live="polite">
        <p className={switchProgress.phase === 'failed' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-text-secondary)]'}>
          {switchProgress.phase === 'failed' ? "The switch didn't finish." : switchProgress.message}
        </p>
        {switchProgress.steps.map((s, i) => (
          <p key={`${s.step}-${i}`} className={s.ok ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-status-error-soft)]'}>
            {s.ok ? '✓' : '✗'} {s.detail}
          </p>
        ))}
      </div>
    ) : undefined
  // Advanced → Set up by hand: the address, token and relay as rows.
  const manualRows = (
    <div className="space-y-3" data-testid="tunnel-manual">
      <div className="flex items-center justify-between gap-6" data-settings-id="k2-connect.subdomain">
        <label htmlFor="tunnel-manual-sub" className="type-row text-[var(--color-text-primary)]">Address</label>
        <div className={`${SETTING_FIELD_WIDTH} flex items-center gap-1.5 flex-shrink-0`}>
          <Input id="tunnel-manual-sub" className="flex-1 min-w-0 type-control" placeholder="alex" value={subdomain} onChange={(e) => setSubdomain(e.target.value)} />
          <span className="type-hint text-[var(--color-text-secondary)]">.k2.dev</span>
        </div>
      </div>
      <div className="flex items-center justify-between gap-6" data-settings-id="k2-connect.token">
        <label htmlFor="tunnel-manual-token" className="type-row text-[var(--color-text-primary)]">
          Token{tokenSet && <span className="type-hint text-[var(--color-text-secondary)] ml-2">saved</span>}
        </label>
        <Input
          id="tunnel-manual-token"
          className={`${SETTING_FIELD_WIDTH} flex-shrink-0 type-control`}
          type="password"
          placeholder={tokenSet ? 'Leave blank to keep the saved one' : 'From your k2.dev account'}
          value={token}
          onChange={(e) => setToken(e.target.value)}
        />
      </div>
      <div className="flex items-center justify-between gap-6" data-settings-id="k2-connect.server-addr">
        <label htmlFor="tunnel-manual-relay" className="type-row text-[var(--color-text-primary)]">Relay and port</label>
        <div className={`${SETTING_FIELD_WIDTH} flex items-center gap-1.5 flex-shrink-0`}>
          <Input
            id="tunnel-manual-relay"
            className="flex-1 min-w-0 type-control"
            placeholder={RELAY_PLACEHOLDER}
            value={serverAddr}
            onChange={(e) => {
              setServerAddr(e.target.value)
              setRelayEdited(true)
            }}
          />
          <Input
            aria-label="Relay port"
            className="w-16 flex-shrink-0 type-control"
            placeholder={PORT_PLACEHOLDER}
            value={serverPort}
            onChange={(e) => {
              setServerPort(e.target.value)
              setRelayEdited(true)
            }}
          />
        </div>
      </div>
      <SaveError state={manualSave.state} />
      <div className="flex items-center justify-end gap-2">
        <SaveNote state={manualSave.state} />
        <Button onClick={() => void saveConfig()} disabled={busy}>
          {busy ? 'Saving…' : 'Save'}
        </Button>
      </div>
    </div>
  )

  // Policies: what other servers, apps and agents may do on the server
  // you're viewing. On Tunnel, and alone on Connected Servers under air-gap.
  const policiesGroup = (
    <SettingsGroup
      title="Policies"
      hint={`What other servers, apps and agents may do on ${isRemote ? 'the server you are viewing' : 'this server'}.`}
      card
      testId="tunnel-policies"
    >
      {myRole !== 'member' ? (
        <>
          <SettingRow
            settingId="k2-connect.federation"
            label="Connect agents across servers"
            hint="Agents here can message agents on the servers you add under Connected Servers."
          >
            <Toggle
              checked={federationEnabled}
              onChange={(next) => void setFederationEnabled(next)}
              aria-label="Connect agents across servers"
            />
          </SettingRow>
          <AllowRemoteInstructRow />
          <SettingRow
            settingId="k2-connect.public-api"
            label="Public API"
            hint="Apps with a key from Settings → API Keys can use this server. Takes effect right away."
          >
            <Toggle checked={apiEnabled} onChange={(next) => void setApiEnabled(next)} aria-label="Public API" />
          </SettingRow>
          <DnsManageEnabledRow />
          <AgentsCanCreateConnectionsRow />
          <ConnectLoginIngressRow />
          <BootStatusPublicRow />
        </>
      ) : (
        <SettingRow label="Only an admin can change these." hint="Ask the owner of this server if something here needs to change.">
          {null}
        </SettingRow>
      )}
    </SettingsGroup>
  )
  return (
    <div className="w-full h-full min-h-0 flex flex-col">
      {/* Scroll owned by parent shell panes; avoid nested overflow here. */}
      <div className="space-y-6 pb-4">
        {/* ── Policies alone (Connected Servers under air-gap) ── */}
        {panel === 'policies' && policiesGroup}

        {/* ── Tunnel: status first (can this server be reached, where, and
            the one next step), then the k2.dev account and address, the
            addresses it serves, the policies, and Advanced last. ── */}
        {panel === 'tunnel' && (
        <>
        {isRemote ? (
          <SettingsLoadState
            kind="empty"
            title="Set up a tunnel on its own computer"
            testId="tunnel-remote"
          >
            You&apos;re viewing {activeHost.label}. To set up this computer&apos;s tunnel, switch to This computer in the
            server menu.
          </SettingsLoadState>
        ) : summary.state === 'loading' ? (
          <SettingsLoadState kind="loading" title="Checking the tunnel…" testId="tunnel-status-loading">
            Looking up whether this server can be reached from the internet.
          </SettingsLoadState>
        ) : summary.state === 'read-failed' ? (
          <SettingsLoadState
            kind="error"
            title="Couldn't check the tunnel"
            detail={summary.detail}
            testId="tunnel-status-error"
            action={<Button onClick={() => void reloadTunnel()}>Try again</Button>}
          >
            K2 couldn&apos;t read this computer&apos;s tunnel status.
          </SettingsLoadState>
        ) : (
          <>
        <SettingsGroup title="Status" card testId="tunnel-status">
          <SettingRow
            settingId="k2-connect.start-stop"
            testId="tunnel-status-row"
            label={
              <span className="inline-flex items-center gap-2 flex-wrap" aria-live="polite">
                <StatusDot tone={summary.tone} />
                <HealthWord tone={summary.tone}>{summary.word}</HealthWord>
                {showAddress && (
                  <a
                    href={publicUrl as string}
                    target="_blank"
                    rel="noreferrer"
                    data-k2-focus="" className="relative px-0.5 -mx-0.5 font-mono text-[var(--color-accent-text)] hover:underline no-drag"
                  >
                    {publicUrl}
                  </a>
                )}
              </span>
            }
            hint={
              <>
                {summary.hint}
                {summary.detail && (
                  <span className="block text-[var(--color-text-secondary)]">
                    The server said: {summary.detail.replace(/[.\s]+$/, '')}.
                  </span>
                )}
                {summary.state === 'missing-helper' && (
                  <a
                    href="https://github.com/fatedier/frp/releases"
                    target="_blank"
                    rel="noreferrer"
                    data-k2-focus="" className="relative block px-0.5 -mx-0.5 text-[var(--color-accent-text)] hover:underline no-drag"
                  >
                    Open the frp releases page
                  </a>
                )}
              </>
            }
            below={statusBelow}
          >
            {summary.state === 'reachable' && publicUrl && (
              <Button variant="quiet" className="flex-shrink-0" onClick={() => void copyAddress(publicUrl)}>
                {copied ? 'Copied' : 'Copy address'}
              </Button>
            )}
            {summary.action === 'turn-off' && (
              <Button className="flex-shrink-0" onClick={() => void stopTunnel()} disabled={busy || swapBusy}>
                {busy ? 'Turning off…' : 'Turn off'}
              </Button>
            )}
            {(summary.action === 'turn-on' || summary.action === 'try-again') && (
              <Button variant="accent" className="flex-shrink-0" onClick={() => void startTunnel()} disabled={busy || swapBusy}>
                {summary.action === 'try-again' ? 'Try again' : 'Turn on'}
              </Button>
            )}
            {summary.state === 'connecting' && (
              <Button variant="accent" className="flex-shrink-0" disabled>
                Turning on…
              </Button>
            )}
            {summary.action === 'repair' && (
              <Button variant="accent" className="flex-shrink-0" onClick={() => void swapTunnel(subdomain.trim())} disabled={busy || swapBusy}>
                Repair
              </Button>
            )}
            {summary.action === 'check-again' && (
              <Button className="flex-shrink-0" onClick={() => void refreshStatus()}>
                Check again
              </Button>
            )}
          </SettingRow>
          {configured && !tunnelReleased && summary.state !== 'paused' && (
            <SettingRow
              settingId="k2-connect.auto-start"
              label="Turn on again after a restart"
              hint={
                <>
                  When K2 or this computer restarts, the tunnel comes back on by itself.
                  <SaveError state={autoStartSave.state} />
                </>
              }
            >
              <SaveNote state={autoStartSave.state} />
              <Toggle
                checked={autoStart}
                onChange={(next) => void toggleAutoStart(next)}
                aria-label="Turn on again after a restart"
              />
            </SettingRow>
          )}
        </SettingsGroup>

        {/* ── k2.dev account: sign in, then choose this server's address ── */}
        <SettingsGroup
          title="k2.dev account"
          badge={<BetaTag />}
          hint={
            session
              ? "Your account holds the addresses you've bought for your servers."
              : "Your account holds the addresses you've bought. Sign in to choose the one this server uses."
          }
          card
          testId="tunnel-account"
        >
          {!session ? (
            <form
              data-settings-id="k2-connect.account-login"
              aria-label="Sign in to k2.dev"
              className="divide-y divide-[var(--color-border)]"
              onSubmit={(e) => {
                e.preventDefault()
                void handleSignIn()
              }}
            >
              <SettingRow label="Email" field>
                <Input
                  className="w-full type-control"
                  type="email"
                  autoComplete="email"
                  aria-label="Email"
                  placeholder="you@example.com"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                />
              </SettingRow>
              <SettingRow label="Password" field>
                <Input
                  className="w-full type-control"
                  type="password"
                  autoComplete="current-password"
                  aria-label="Password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                />
              </SettingRow>
              <SettingRow
                label="Sign in to choose an address"
                hint={
                  <>
                    No account yet? Create one at{' '}
                    <a href="https://k2.dev" target="_blank" rel="noreferrer" data-k2-focus="" className="relative px-0.5 -mx-0.5 text-[var(--color-accent-text)] hover:underline no-drag">
                      k2.dev
                    </a>
                    .
                  </>
                }
                below={
                  authError ? (
                    <Callout tone="error" role="alert">
                      <p className="type-hint">{authError.what}</p>
                      <p className="type-hint text-[var(--color-text-secondary)]">The server said: {authError.detail.replace(/[.\s]+$/, '')}.</p>
                    </Callout>
                  ) : undefined
                }
              >
                <Button type="submit" variant={configured ? 'default' : 'accent'} disabled={authBusy || !email.trim() || !password}>
                  {authBusy ? 'Signing in…' : 'Sign in'}
                </Button>
              </SettingRow>
            </form>
          ) : (
            <>
              <SettingRow
                settingId="k2-connect.account-login"
                label={<>Signed in as {session.email || 'your account'}</>}
                hint="Signing out keeps the tunnel as it is."
                below={
                  authError ? (
                    <Callout tone="error" role="alert">
                      <p className="type-hint">{authError.what}</p>
                      <p className="type-hint text-[var(--color-text-secondary)]">The server said: {authError.detail.replace(/[.\s]+$/, '')}.</p>
                    </Callout>
                  ) : undefined
                }
              >
                <Button variant="quiet" onClick={() => void handleSignOut()}>
                  Sign out
                </Button>
              </SettingRow>
              <SettingRow
                settingId="k2-connect.subdomain-list"
                label="Address"
                hint={
                  subdomains.length === 0 ? (
                    <>
                      Your account has no addresses yet. Buy one at{' '}
                      <a href="https://k2.dev" target="_blank" rel="noreferrer" data-k2-focus="" className="relative px-0.5 -mx-0.5 text-[var(--color-accent-text)] hover:underline no-drag">
                        k2.dev
                      </a>
                      , then come back.
                    </>
                  ) : running ? (
                    'The address this server uses. Choosing another moves the tunnel in one step.'
                  ) : (
                    'The address this server uses on the internet.'
                  )
                }
                field={subdomains.length > 0}
                below={addressBelow}
              >
                {subdomains.length > 0 ? (
                  <SettingDropdown
                    value={selectedLabel ?? subdomain ?? ''}
                    options={subdomainOptions}
                    // Start EMPTY after sign-in — never pre-fill a default the
                    // user didn't choose. Picking is what binds the subdomain's
                    // token into config; a phantom "selected" default let users
                    // hit Start with no token bound ("tunnel not configured").
                    placeholder="Choose an address…"
                    ariaLabel="Address"
                    fullWidth
                    onChange={(v) => void handleSubdomainPick(v)}
                  />
                ) : null}
              </SettingRow>
            </>
          )}
        </SettingsGroup>
          </>
        )}

        {/* Addresses: this server's and the sites published under it (This
            computer or the server you're viewing). */}
        <SettingsGroup
          title="Addresses"
          hint="This server's address and the sites it publishes under it."
          card
          settingId="k2-connect.urls"
          testId="tunnel-addresses"
        >
          <TunnelUrlsPanel
            canManage={canManagePublishedSubdomains(isRemote, myRole)}
            // This computer: "This server" follows the Status card above.
            serverStatus={isRemote ? undefined : status}
          />
        </SettingsGroup>

        {policiesGroup}

        {/* ── Advanced: manual setup and removing the tunnel ── */}
        {!isRemote && summary.state !== 'loading' && summary.state !== 'read-failed' && (
          <SettingsGroup
            title="Advanced"
            hint="Set the tunnel up by hand, or remove it from this computer."
            card
            testId="tunnel-advanced"
          >
            <SettingRow
              label="Set up by hand"
              hint="Enter an address and token yourself instead of signing in."
              below={showAdvanced ? manualRows : undefined}
            >
              <Button variant="quiet" aria-expanded={showAdvanced} onClick={() => setShowAdvanced((v) => !v)}>
                {showAdvanced ? 'Hide' : 'Set up by hand'}
              </Button>
            </SettingRow>
            {tokenSet && !tunnelReleased && (
              <SettingRow
                settingId="k2-connect.release-device"
                label={`Release ${subdomain.trim() ? hostOf(subdomain.trim()) : 'the address'} from this computer`}
                hint="It stops reaching this computer. Your account keeps the address for another computer."
              >
                <Button variant="danger-quiet" onClick={() => void releaseFromDevice('release')} disabled={busy || swapBusy}>
                  Release…
                </Button>
              </SettingRow>
            )}
            {tokenSet && !tunnelReleased && (
              <SettingRow
                label="Forget the tunnel settings"
                hint="Deletes this computer's tunnel settings without telling k2.dev. For settings copied from another computer."
              >
                <Button variant="danger-quiet" onClick={() => void releaseFromDevice('forget')} disabled={busy || swapBusy}>
                  Forget…
                </Button>
              </SettingRow>
            )}
          </SettingsGroup>
        )}
        </>
        )}

        {/* ── Admin Access: the people who can sign in, and the password rules. ── */}
        {panel === 'people' && <AdminAccessPanel />}
      </div>
    </div>
  )
}
