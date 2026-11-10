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
import {
  addUserBody,
  temporaryDefaultFor,
  canSetRole,
  needsEnableAsMember,
  parseK2Role,
  parseShares,
  roleOptions,
  splitShareList,
  VOICE_CHAT_LABEL,
  type K2Role,
  type K2User,
} from './connect-user-roles'
import { VoiceChatSharesEditor } from './VoiceChatSharesEditor'
import { VOICE_CHAT_ROLE_HELP } from '@/lib/voice-chat-only'
import { WasViewerBadge, WasViewerNotice } from './WasViewerNotice'
import { MUST_CHANGE_NOTE } from './AppUserDialogs'
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
import { BetaTag, Toggle, Checkbox, Tag, Button, Callout, Input } from '@/components/ui'
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
import { PresenceChip } from '@/components/Presence/PresenceChip'
import { PersonPictureControl } from '@/components/Presence/PersonPictureControl'
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
  { id: 'k2-connect.users', section: 'k2-connect', label: 'Users / Access', description: 'People you allow to connect in to this computer', keywords: ['users', 'access', 'people', 'login', 'password', 'connect in', 'multi-user', 'allow', 'invite'] },
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

// Whether a viewer of `role` may see + use the Users/Access management UI
// (add/remove/enable/disable). Admin + Owner only.
function canManageUsers(role: K2Role | null): boolean {
  return role === 'admin' || role === 'owner'
}

// Whether a viewer of `role` may change other users' roles. Owner only.
function canChangeRoles(role: K2Role | null): boolean {
  return role === 'owner'
}

/** The role picker's options for `actor` on this server (§5.3, Q1): the
 *  Owner every role (Voice & Chat only when the server knows it); an Admin
 *  only Member and Voice & Chat only, and only on a server that knows it. */
function pickerOptions(actor: K2Role | null, voiceChatRole: boolean): Array<{ value: K2Role; label: string }> {
  const all = roleOptions(voiceChatRole)
  if (actor === 'owner') return [...all]
  if (actor === 'admin' && voiceChatRole) return all.filter((o) => o.value === 'member' || o.value === 'voice_chat')
  return []
}

/** The presence chip's role: Voice & Chat only draws as a member. */
function chipRole(raw: string | undefined): 'owner' | 'admin' | 'member' {
  const r = parseK2Role(raw)
  return r === 'owner' || r === 'admin' ? r : 'member'
}

/** A users row's role as shown read-only. */
function roleLabel(raw: string | undefined): string {
  return raw === 'voice_chat' ? VOICE_CHAT_LABEL : raw ?? 'member'
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

/** 0.46.8: whether a users/add or users/set-password answer says the
 *  password is temporary. An older daemon may not answer the field; then
 *  `fallback` (what this client asked for) stands. */
export async function answeredTemporary(res: Response, fallback: boolean): Promise<boolean> {
  const text = await res.text()
  try {
    const parsed = JSON.parse(text) as { mustChangePassword?: unknown }
    if (typeof parsed.mustChangePassword === 'boolean') return parsed.mustChangePassword
  } catch {
    /* an empty or non-JSON 200 from an older daemon */
  }
  return fallback
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
  // prd-voice-chat-only-members-v1: shares typed with a new Voice & Chat
  // only login (comma separated agent addresses and Garden ids).
  const [newShareAgents, setNewShareAgents] = useState('')
  const [newShareGardens, setNewShareGardens] = useState('')
  const voiceChatRole = primaryScope().serverSupports('voice-chat-role-v1')
  // 0.46.8: the password you type for someone else is temporary (they
  // choose their own at their first sign-in). On by default.
  const [newTemporary, setNewTemporary] = useState(true)
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
      // §5.3 / V5: a Voice & Chat only login is made at its role in the
      // add itself (never a Member first), with its shares.
      const res = await userPost(
        '/add',
        addUserBody({
          username,
          password: pw,
          mustChangePassword: newTemporary,
          role: newRole,
          firstName,
          lastName,
          shareAgents: splitShareList(newShareAgents),
          shareGardens: splitShareList(newShareGardens),
        }),
      )
      if (!res.ok) {
        setAddError(await errText(res))
        return
      }
      const temporary = await answeredTemporary(res, newTemporary)
      // add makes a Member or a Voice & Chat only login. An Owner can pick
      // Admin or Owner as the starting role; it's applied with the same
      // set-role path as the per-row selector.
      if (canChangeRoles(myRole) && (newRole === 'admin' || newRole === 'owner')) {
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
      setNewShareAgents('')
      setNewShareGardens('')
      setNewTemporary(true)
      setAddedMsg(
        temporary
          ? `Added — share these credentials with ${username}. ${MUST_CHANGE_NOTE}`
          : `Added — share these credentials with ${username}.`,
      )
      setTimeout(() => setAddedMsg(null), 8000)
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
      // No flag: the daemon decides (0.46.8: temporary, except the
      // owner's own linked login) and says which in its answer.
      const res = await userPost('/set-password', { username, password: resetPassword })
      if (!res.ok) {
        setUsersError(await errText(res))
        return
      }
      const temporary = await answeredTemporary(res, false)
      setResetFor(null)
      setResetPassword('')
      setResetMsg(temporary ? `Password reset for ${username}. ${MUST_CHANGE_NOTE}` : `Password reset for ${username}.`)
      setTimeout(() => setResetMsg(null), 8000)
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

  // Voice & Chat only shares: POST /cli/users/set-shares replaces both
  // lists. Resolves to the error to show, or null.
  const saveShares = async (username: string, agents: string[], gardens: string[]): Promise<string | null> => {
    try {
      const res = await userPost('/set-shares', { username, agents, gardens })
      if (!res.ok) return await errText(res)
      await refreshUsers()
      return null
    } catch (e) {
      return e instanceof Error ? e.message : 'Failed to save shares'
    }
  }

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

  const inputCls =
    'w-full px-2 py-1 type-control bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

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
                    className="k2-focus font-mono text-[var(--color-accent-text)] hover:underline no-drag"
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
                    className="k2-focus block text-[var(--color-accent-text)] hover:underline no-drag"
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
                    <a href="https://k2.dev" target="_blank" rel="noreferrer" className="k2-focus text-[var(--color-accent-text)] hover:underline no-drag">
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
                      <a href="https://k2.dev" target="_blank" rel="noreferrer" className="k2-focus text-[var(--color-accent-text)] hover:underline no-drag">
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
          <TunnelUrlsPanel canManage={canManagePublishedSubdomains(isRemote, myRole)} />
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

        {/* ── People: Users / Access — role-gated multi-user list (#617 / #629) ─── */}
        {panel === 'people' && (
        <SettingsGroup title="Users / Access">
          <div data-settings-id="k2-connect.users" className="space-y-3">
            {/* K2 #629: only viewers who can manage users (Owner|Admin)
                see the management surface. A Member viewer (or an
                unresolved/forbidden whoami) gets a clean note — never the
                raw token/403 error. */}
            {whoamiLoaded && !canManageUsers(myRole) ? (
              <p className="type-hint text-[var(--color-text-muted)] leading-relaxed py-1">
                {oldHostNoRoles
                  ? `This host (v${serverVersion}) predates roles — update it to v${featureMinVersion('roles')} to manage users here.`
                  : 'User management is handled by an administrator.'}
              </p>
            ) : !whoamiLoaded ? (
              <p className="type-hint text-[var(--color-text-muted)] py-1">Loading…</p>
            ) : (
            <>
            <p className="type-hint text-[var(--color-text-muted)] leading-relaxed">
              People you allow to connect in to this computer. You set each
              person&apos;s username + initial password; they sign in from their K2 app (or
              change their own password at <span className="font-mono">https://&lt;sub&gt;.k2.dev</span>).
              You can&apos;t view a password after setting it — only reset it.
            </p>

            {/* Password requirements (K2 #620) — server-enforced policy */}
            <div className="border border-[var(--color-border)] p-2.5 space-y-2">
              <div className="flex items-center justify-between gap-3">
                <span className="type-hint text-[var(--color-text-secondary)] ">
                  Password requirements
                </span>
                <span className="flex items-center gap-2">
                  <label className="flex items-center gap-1.5 type-hint text-[var(--color-text-muted)]">
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
                    <Checkbox
                      checked={checked}
                      onChange={(e) => setVal(e.target.checked)}
                    />
                    <span className="type-hint text-[var(--color-text-secondary)]">Require {label}</span>
                  </label>
                ))}
              </div>
              <div className="flex items-center gap-2 pt-0.5">
                <button
                  onClick={() => void savePolicy()}
                  className="px-3 py-1 type-control border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer"
                >
                  Save requirements
                </button>
                {policySavedMsg && <span className="type-hint text-[var(--color-status-ok-soft)]">{policySavedMsg}</span>}
                {policyError && <span className="type-hint text-[var(--color-status-error-soft)]">{policyError}</span>}
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
                {pickerOptions(myRole, voiceChatRole).length > 0 && (
                  <SettingDropdown
                    value={newRole}
                    options={pickerOptions(myRole, voiceChatRole)}
                    ariaLabel="New user role"
                    onChange={(value) => {
                      const r = parseK2Role(value)
                      if (r) {
                        setNewRole(r)
                        setNewTemporary(temporaryDefaultFor(r))
                      }
                    }}
                    menuAlign="left"
                  />
                )}
                <button
                  type="submit"
                  disabled={addBusy || !newUsername.trim() || !newPassword}
                  className="flex-shrink-0 px-3 py-1 type-control text-[var(--color-on-accent)] bg-[var(--color-accent-fill)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
                >
                  {addBusy ? 'Adding…' : 'Add user'}
                </button>
              </div>
              {voiceChatRole && pickerOptions(myRole, voiceChatRole).length > 0 && (
                <div className="type-hint text-[var(--color-text-muted)]" data-testid="voice-chat-role-help">
                  {VOICE_CHAT_ROLE_HELP}
                </div>
              )}
              {newRole === 'voice_chat' && (
                <div className="space-y-1" data-testid="new-user-shares">
                  <div className="flex items-center gap-2">
                    <span className="w-16 flex-shrink-0 type-hint text-[var(--color-text-muted)]">Agents</span>
                    <input
                      className="flex-1 min-w-0 px-2 py-1 type-control bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
                      placeholder="Shared agents (addresses, comma separated)"
                      aria-label="New user shared agents"
                      value={newShareAgents}
                      onChange={(e) => setNewShareAgents(e.target.value)}
                    />
                  </div>
                  <div className="flex items-center gap-2">
                    <span className="w-16 flex-shrink-0 type-hint text-[var(--color-text-muted)]">Gardens</span>
                    <input
                      className="flex-1 min-w-0 px-2 py-1 type-control bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
                      placeholder="Shared Gardens (ids, comma separated)"
                      aria-label="New user shared Gardens"
                      value={newShareGardens}
                      onChange={(e) => setNewShareGardens(e.target.value)}
                    />
                  </div>
                </div>
              )}
              <label className="flex items-start gap-1.5 cursor-pointer select-none no-drag" data-testid="new-user-temporary">
                {/* Square peer-checked checkbox (matches "Enabled" below). */}
                <Checkbox
                  checked={newTemporary}
                  onChange={(e) => setNewTemporary(e.target.checked)}
                  aria-label="Temporary password"
                  className="mt-0.5"
                />
                <span className="type-hint text-[var(--color-text-secondary)]">
                  Temporary password
                  <span className="text-[var(--color-text-muted)]">
                    {newTemporary ? ` — ${MUST_CHANGE_NOTE}` : ' — off: they keep the password you type.'}
                  </span>
                </span>
              </label>
              {addError && (
                <div className="type-hint text-[var(--color-status-error-soft)] px-2 py-1 border border-[color-mix(in_srgb,var(--color-status-error-soft)_20%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error-soft)_5%,transparent)]">{addError}</div>
              )}
              {addedMsg && <div className="type-hint text-[var(--color-status-ok-soft)]" data-testid="added-user-msg">{addedMsg}</div>}
            </form>

            {usersError && (
              <div className="type-hint text-[var(--color-status-error-soft)] px-2 py-1 border border-[color-mix(in_srgb,var(--color-status-error-soft)_20%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error-soft)_5%,transparent)]">{usersError}</div>
            )}
            {resetMsg && <div className="type-hint text-[var(--color-status-ok-soft)]" data-testid="reset-password-msg">{resetMsg}</div>}

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
              <div className="type-hint text-[var(--color-status-error-soft)] px-2 py-1 border border-[color-mix(in_srgb,var(--color-status-error-soft)_20%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error-soft)_5%,transparent)]">{ownerAccounts.error}</div>
            )}
            {usersLoaded && users.length === 0 ? (
              <div className="type-hint text-[var(--color-text-muted)] py-1">
                No users yet — add one above.
              </div>
            ) : usersLoaded && visibleUsers.length === 0 ? (
              <div className="type-hint text-[var(--color-text-muted)] py-1">
                No users match.
              </div>
            ) : (
              <div className="divide-y divide-[var(--color-border)]">
                {visibleUsers.map((u) => (
                  <div key={u.username} className="py-2 space-y-2">
                    <div className="flex items-center justify-between gap-3">
                      <span className="flex items-center gap-2 min-w-0">
                        {/* 0.46.8: the person's picture on this server
                            (initials until they have one). */}
                        <span data-testid={`user-chip-${u.username}`} className="flex flex-none">
                          <PresenceChip
                            name={u.username}
                            role={chipRole(u.role)}
                            size={20}
                            color={u.color ?? null}
                            names={u}
                            pictureScope={primaryScope()}
                            title=""
                          />
                        </span>
                        {realName(u) ? (
                          <span className="type-row text-[var(--color-text-primary)] truncate" data-testid={`user-real-name-${u.username}`}>
                            {realName(u)}
                          </span>
                        ) : null}
                        <span
                          className={`type-row font-mono truncate ${realName(u) ? 'text-[var(--color-text-muted)]' : 'text-[var(--color-text-primary)]'}`}
                        >
                          {u.username}
                        </span>
                        {u.disabled && (
                          <Tag tone="muted">
                            disabled
                          </Tag>
                        )}
                        {u.wasViewer && <WasViewerBadge />}
                        {u.mustChangePassword === true && (
                          <Tag
                            tone="warn"
                            data-testid={`temporary-password-${u.username}`}
                            title={MUST_CHANGE_NOTE}
                          >
                            Temporary password
                          </Tag>
                        )}
                      </span>
                      <div className="flex items-center gap-3 flex-shrink-0">
                        {/* K2 #629 — role selector. Editable only for an
                            Owner viewer; Admins see the role read-only. */}
                        {canChangeRoles(myRole) ||
                        (myRole === 'admin' && voiceChatRole && canSetRole(myRole, parseK2Role(u.role), 'voice_chat')) ? (
                          <SettingDropdown
                            value={u.role ?? 'member'}
                            // No Viewer option. An older server can still
                            // report "viewer"; it shows as the placeholder
                            // and can be changed to a real role.
                            placeholder={roleLabel(u.role)}
                            options={pickerOptions(myRole, voiceChatRole)}
                            ariaLabel={`${u.username} role`}
                            onChange={(value) => {
                              const r = parseK2Role(value)
                              if (r) void changeRole(u.username, r)
                            }}
                          />
                        ) : (
                          <Tag tone="neutral">
                            {roleLabel(u.role)}
                          </Tag>
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
                          <Checkbox
                            checked={!u.disabled}
                            onChange={(e) => void toggleDisabled(u.username, !e.target.checked)}
                          />
                          <span className="type-hint text-[var(--color-text-secondary)]">Enabled</span>
                        </label>
                        <button
                          data-testid={`edit-name-${u.username}`}
                          onClick={() => {
                            setNameFor((cur) => (cur === u.username ? null : u.username))
                            setNameDraft({ firstName: u.firstName ?? '', lastName: u.lastName ?? '' })
                          }}
                          className="type-hint text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:underline no-drag cursor-pointer"
                        >
                          Edit name
                        </button>
                        {/* 0.46.8: set or remove their picture; the daemon
                            applies the names' rule (Admin+ on a Member or
                            an Admin, Owner on an Owner). */}
                        <PersonPictureControl
                          scope={primaryScope()}
                          login={u.username}
                          person={u.username}
                          look="links"
                          testId={`user-picture-${u.username}`}
                        />
                        <button
                          onClick={() => {
                            setResetFor((cur) => (cur === u.username ? null : u.username))
                            setResetPassword('')
                          }}
                          className="type-hint text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:underline no-drag cursor-pointer"
                        >
                          Reset password
                        </button>
                        {removeConfirm === u.username ? (
                          <span className="flex items-center gap-1.5">
                            <button
                              onClick={() => void removeUser(u.username)}
                              className="type-hint text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
                            >
                              Confirm remove
                            </button>
                            <button
                              onClick={() => setRemoveConfirm(null)}
                              className="type-hint text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer"
                            >
                              Cancel
                            </button>
                          </span>
                        ) : (
                          <button
                            onClick={() => setRemoveConfirm(u.username)}
                            className="type-hint text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
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
                    {u.role === 'voice_chat' && canManageUsers(myRole) && (
                      <VoiceChatSharesEditor
                        username={u.username}
                        shares={parseShares(u.shares)}
                        onSave={(agents, gardens) => saveShares(u.username, agents, gardens)}
                      />
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
                          className="px-3 py-1 type-control border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-60"
                        >
                          {nameBusy ? 'Saving…' : 'Save name'}
                        </button>
                        <button
                          type="button"
                          onClick={() => setNameFor(null)}
                          className="type-hint text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer"
                        >
                          Cancel
                        </button>
                      </form>
                    )}
                    {resetFor === u.username && (
                      <form
                        className="flex flex-wrap items-center gap-1.5"
                        data-testid={`reset-form-${u.username}`}
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
                          className="px-3 py-1 type-control border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-60"
                        >
                          {resetBusy ? 'Saving…' : 'Set password'}
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            setResetFor(null)
                            setResetPassword('')
                          }}
                          className="type-hint text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer"
                        >
                          Cancel
                        </button>
                        <span className="basis-full type-hint text-[var(--color-text-muted)]">
                          {ownerAccounts.loaded && ownerAccounts.isLinked(u.username)
                            ? 'Your own login: you keep this password.'
                            : MUST_CHANGE_NOTE}
                        </span>
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
