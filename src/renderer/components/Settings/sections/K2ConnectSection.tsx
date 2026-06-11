// Settings → K2 Connect — the HOST / expose side (PRD §6, build order
// MVP). The OTHER direction from Connections (§1–5, the client connecting
// OUT): "K2 Connect" is where THIS device exposes its OWN daemon at a
// subdomain it owns under k2.dev via the frpc reverse tunnel.
//
// MVP (buildable now, no control-plane changes):
//   - tunnel config form: token + subdomain + serverAddr (default
//     178.156.232.105:7000) → Save (POST /cli/tunnel/config; the token is
//     redacted on read → tokenSet:bool, never echoed back)
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
import { getDaemonWs, daemonHttpBase, invalidateDaemonWs } from '@/kessel/daemon-ws'
import type { SettingEntry } from '../searchManifest'
import { SettingRow, SettingsGroup, SettingDropdown } from '../controls/SettingControls'
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
import { useServerSupports, featureMinVersion } from '@/lib/server-capabilities'

const DEFAULT_SERVER_ADDR = '178.156.232.105'
const DEFAULT_SERVER_PORT = 7000

// Keychain coordinates for the k2.dev ACCOUNT session. Kept distinct from
// the host-token service (`com.k2so.connect.host-token`) so the two never
// collide. Only the refresh token + email are persisted; the access token
// is memory-only.
const ACCOUNT_KEYCHAIN_SERVICE = 'dev.k2.connect.account'
// Pre-0.40 service name — read fallback only; copied forward on first
// read, never deleted (an un-updated daemon may still read it).
const LEGACY_ACCOUNT_KEYCHAIN_SERVICE = 'com.k2so.connect.account'
const SESSION_ACCOUNT_KEY = 'session-refresh-token'
const EMAIL_ACCOUNT_KEY = 'session-email'

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
]

interface TunnelStatus {
  running: boolean
  public_url: string | null
  frpc_installed: boolean
}

// K2 #629 — connect-user permission tier.
type K2Role = 'owner' | 'admin' | 'member'

interface K2User {
  username: string
  createdAt?: string | null
  disabled: boolean
  // K2 #629. Pre-#629 daemons omit it → treat as 'member'.
  role?: K2Role
}

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
}

// The daemon rotates ~/.k2so/daemon.token on every restart; getDaemonWs()
// caches the local creds per app session and only re-fetches on WS
// failure, so HTTP /cli/* calls can keep sending a stale token → 403
// until a manual window reload. Wrap each request so a 403 invalidates
// the creds cache, re-resolves fresh creds, and retries exactly once.
async function tunnelGet(suffix: string): Promise<Response> {
  const send = async (): Promise<Response> => {
    const creds = await getDaemonWs()
    const sep = suffix.includes('?') ? '&' : '?'
    return fetch(`${daemonHttpBase(creds)}/cli/tunnel/${suffix}${sep}token=${creds.token}`, { method: 'GET' })
  }
  const res = await send()
  if (res.status !== 403) return res
  invalidateDaemonWs()
  return send()
}

async function tunnelPost(suffix: string, body?: unknown): Promise<Response> {
  const send = async (): Promise<Response> => {
    const creds = await getDaemonWs()
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
}

// ── Users / Access (owner-gated /cli/users/*) ──────────────────────────
// Same stale-token-retry shape as tunnelGet/tunnelPost: the LOCAL daemon
// creds from getDaemonWs() carry the owner token these routes require.
async function userGet(suffix: string): Promise<Response> {
  const send = async (): Promise<Response> => {
    const creds = await getDaemonWs()
    return fetch(`${daemonHttpBase(creds)}/cli/users${suffix}?token=${creds.token}`, { method: 'GET' })
  }
  const res = await send()
  if (res.status !== 403) return res
  invalidateDaemonWs()
  return send()
}

async function userPost(suffix: string, body?: unknown): Promise<Response> {
  const send = async (): Promise<Response> => {
    const creds = await getDaemonWs()
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
}

// GET /cli/auth/whoami — resolve the LOCAL viewer's identity + role
// (K2 #629). With the local daemon owner token this returns
// {owner:true, role:'owner'}; the role gates the management UI below.
async function whoamiGet(): Promise<Response> {
  const send = async (): Promise<Response> => {
    const creds = await getDaemonWs()
    return fetch(`${daemonHttpBase(creds)}/cli/auth/whoami?token=${creds.token}`, { method: 'GET' })
  }
  const res = await send()
  if (res.status !== 403) return res
  invalidateDaemonWs()
  return send()
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

async function saveAccountSession(refreshToken: string, email: string): Promise<void> {
  try {
    await invoke('k2_secret_set', { service: ACCOUNT_KEYCHAIN_SERVICE, account: SESSION_ACCOUNT_KEY, secret: refreshToken })
    await invoke('k2_secret_set', { service: ACCOUNT_KEYCHAIN_SERVICE, account: EMAIL_ACCOUNT_KEY, secret: email })
  } catch {
    /* keychain unavailable — session stays in memory for this run */
  }
}

async function readAccountRefreshToken(): Promise<string | null> {
  try {
    const secret = await invoke<string | null>('k2_secret_get', { service: ACCOUNT_KEYCHAIN_SERVICE, account: SESSION_ACCOUNT_KEY })
    if (secret) return secret
    // 0.40.0 rebrand: migrate a pre-rename session forward on first read.
    const legacy = await invoke<string | null>('k2_secret_get', { service: LEGACY_ACCOUNT_KEYCHAIN_SERVICE, account: SESSION_ACCOUNT_KEY })
    if (legacy) {
      await invoke('k2_secret_set', { service: ACCOUNT_KEYCHAIN_SERVICE, account: SESSION_ACCOUNT_KEY, secret: legacy })
      const legacyEmail = await invoke<string | null>('k2_secret_get', { service: LEGACY_ACCOUNT_KEYCHAIN_SERVICE, account: EMAIL_ACCOUNT_KEY })
      if (legacyEmail) await invoke('k2_secret_set', { service: ACCOUNT_KEYCHAIN_SERVICE, account: EMAIL_ACCOUNT_KEY, secret: legacyEmail })
    }
    return legacy ?? null
  } catch {
    return null
  }
}

async function clearAccountSession(): Promise<void> {
  // Sign-out clears BOTH service names — leaving the legacy copy behind
  // would silently re-sign-in via the migration fallback.
  for (const service of [ACCOUNT_KEYCHAIN_SERVICE, LEGACY_ACCOUNT_KEYCHAIN_SERVICE]) {
    try {
      await invoke('k2_secret_delete', { service, account: SESSION_ACCOUNT_KEY })
    } catch { /* idempotent */ }
    try {
      await invoke('k2_secret_delete', { service, account: EMAIL_ACCOUNT_KEY })
    } catch { /* idempotent */ }
  }
}

export function K2ConnectSection(): React.JSX.Element {
  const [serverAddr, setServerAddr] = useState(DEFAULT_SERVER_ADDR)
  const [serverPort, setServerPort] = useState(String(DEFAULT_SERVER_PORT))
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
  const [viewerRole, setViewerRole] = useState<K2Role | null>(null)
  const [whoamiLoaded, setWhoamiLoaded] = useState(false)
  const [users, setUsers] = useState<K2User[]>([])
  const [usersLoaded, setUsersLoaded] = useState(false)
  const [newUsername, setNewUsername] = useState('')
  const [newPassword, setNewPassword] = useState('')
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

  // ── Password policy state (K2 #620) ─────────────────────────────────
  const [policyMinLength, setPolicyMinLength] = useState('8')
  const [policyRequireSpecial, setPolicyRequireSpecial] = useState(false)
  const [policyRequireNumber, setPolicyRequireNumber] = useState(false)
  const [policyRequireUppercase, setPolicyRequireUppercase] = useState(false)
  const [policySavedMsg, setPolicySavedMsg] = useState<string | null>(null)
  const [policyError, setPolicyError] = useState<string | null>(null)

  // Load config + status on mount, then poll status while mounted.
  useEffect(() => {
    void (async () => {
      try {
        const res = await tunnelGet('config')
        if (res.ok) {
          const cfg = (await res.json()) as TunnelConfigView
          setServerAddr(cfg.serverAddr || DEFAULT_SERVER_ADDR)
          setServerPort(String(cfg.serverPort || DEFAULT_SERVER_PORT))
          setSubdomain(cfg.subdomain || '')
          setTokenSet(cfg.tokenSet)
          setAutoStart(cfg.autoStart ?? false)
        }
      } catch { /* ignore */ }
      void refreshStatus()
      // K2 #629: resolve the viewer's role FIRST so the management UI
      // gates correctly; only load the user list + policy when the viewer
      // can actually manage users (else those routes 403).
      void (async () => {
        const role = await refreshWhoami()
        if (canManageUsers(role)) {
          void refreshUsers()
          void refreshPolicy()
        }
      })()
    })()
    // Restore the account session from the keychain (independent of the
    // tunnel-config load above — must NOT block it).
    void (async () => {
      const stored = await readAccountRefreshToken()
      if (!stored) return
      try {
        const fresh = await refreshSession(stored)
        setSession(fresh)
        // The refresh token may have rotated — persist the new one.
        await saveAccountSession(fresh.refreshToken, fresh.email)
        const subs = await listSubdomains(fresh.accessToken)
        setSubdomains(subs)
      } catch {
        // Expired / revoked — drop the stale credentials, stay logged out.
        await clearAccountSession()
        setSession(null)
      }
    })()
    const interval = setInterval(() => void refreshStatus(), 5000)
    return () => clearInterval(interval)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const refreshStatus = async (): Promise<void> => {
    try {
      const res = await tunnelGet('status')
      if (res.ok) setStatus((await res.json()) as TunnelStatus)
    } catch { /* ignore */ }
  }

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
        const role: K2Role | null =
          data.role === 'owner' || data.role === 'admin' || data.role === 'member'
            ? data.role
            : data.owner
              ? 'owner'
              : null
        setViewerRole(role)
        return role
      }
      setViewerRole(null)
      return null
    } catch {
      setViewerRole(null)
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
      const res = await userPost('/add', { username, password: pw })
      if (!res.ok) {
        setAddError(await errText(res))
        return
      }
      setNewUsername('')
      setNewPassword('')
      setAddedMsg(`Added — share these credentials with the user.`)
      setTimeout(() => setAddedMsg(null), 4000)
      await refreshUsers()
    } catch (e) {
      setAddError(e instanceof Error ? e.message : 'Failed to add user')
    } finally {
      setAddBusy(false)
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
    const portNum = Number(serverPort)
    if (!Number.isInteger(portNum) || portNum <= 0 || portNum > 65535) {
      setError('Server port must be 1–65535')
      setBusy(false)
      return
    }
    // Only send the token when the user typed one (empty string would
    // CLEAR it server-side; undefined leaves the saved token untouched).
    const body: Record<string, unknown> = {
      serverAddr: serverAddr.trim(),
      serverPort: portNum,
      subdomain: subdomain.trim(),
    }
    if (token.trim()) body.token = token.trim()
    try {
      const res = await tunnelPost('config', body)
      if (!res.ok) {
        setError(await errText(res))
        return
      }
      const cfg = (await res.json()) as TunnelConfigView
      setTokenSet(cfg.tokenSet)
      setToken('') // never keep the secret in the field after save
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
      const res = await tunnelPost(`start${sub ? `?subdomain=${encodeURIComponent(sub)}` : ''}`)
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
      const res = await tunnelPost('stop')
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
      const res = await tunnelPost('config', {
        serverAddr: serverAddr.trim() || DEFAULT_SERVER_ADDR,
        serverPort: Number(serverPort) || DEFAULT_SERVER_PORT,
        subdomain: sub.label,
        token: sub.tunnel_token,
        // Persist this device's claim identity now so a later auto-start /
        // headless boot can renew the lease without the renderer (K2 #674).
        deviceId,
        deviceLabel: deviceLabel ?? '',
      })
      if (!res.ok) {
        setError(await errText(res))
        return
      }
      const cfg = (await res.json()) as TunnelConfigView
      setTokenSet(cfg.tokenSet)
      setToken('') // don't keep the secret in the manual field after save
      setBoundMsg(`Bound ${sub.label}.k2.dev — Start the tunnel below.`)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to bind subdomain')
    } finally {
      setBusy(false)
    }
  }

  const inputCls =
    'w-full px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

  const running = status?.running ?? false
  const publicUrl = status?.public_url ?? (subdomain.trim() ? `https://${subdomain.trim()}.k2.dev` : null)
  const frpcMissing = status ? !status.frpc_installed : false

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
    const row = subdomains.find((s) => s.label === value)
    if (row) await bindSubdomain(row)
  }

  // Swap an already-running tunnel to a different subdomain:
  // stop → release(old) → bind(new) → claim(new) → start.
  const swapTunnel = async (value: string): Promise<void> => {
    const row = subdomains.find((s) => s.label === value)
    if (!row) return
    const oldLabel = subdomain.trim()
    const accessToken = session?.accessToken ?? accessTokenRef.current
    setSwapBusy(true)
    setError(null)
    setBoundMsg(null)
    try {
      // 1. Stop the current tunnel (the daemon stops its lease renewal for
      // the old subdomain here — K2 #674).
      const stopRes = await tunnelPost('stop')
      if (!stopRes.ok) {
        setError(await errText(stopRes))
        return
      }
      // 2. Release the old lease (best-effort).
      if (accessToken && oldLabel) {
        void releaseSubdomain(accessToken, oldLabel, deviceId).catch(() => undefined)
      }
      // 3. Bind the new subdomain (config + state).
      setSelectedLabel(row.label)
      setSubdomain(row.label)
      setToken(row.tunnel_token)
      const cfgRes = await tunnelPost('config', {
        serverAddr: serverAddr.trim() || DEFAULT_SERVER_ADDR,
        serverPort: Number(serverPort) || DEFAULT_SERVER_PORT,
        subdomain: row.label,
        token: row.tunnel_token,
        // Persist this device's claim identity so the daemon renews the new
        // subdomain's lease under the same device (K2 #674).
        deviceId,
        deviceLabel: deviceLabel ?? '',
      })
      if (!cfgRes.ok) {
        setError(await errText(cfgRes))
        return
      }
      const cfg = (await cfgRes.json()) as TunnelConfigView
      setTokenSet(cfg.tokenSet)
      setToken('')
      // 4. Claim the new lease (one-shot, explicit user action).
      if (accessToken) {
        try {
          const result = await claimSubdomain(accessToken, row.label, deviceId, deviceLabel)
          if (!result.claimed && result.holder && result.holder !== deviceId) {
            setError(`${row.label}.k2.dev is in use on another device — stop it there first.`)
            return
          }
        } catch (e) {
          setError(e instanceof Error ? e.message : 'Failed to claim subdomain')
          return
        }
      }
      // 5. Start the new tunnel.
      const startRes = await tunnelPost(`start?subdomain=${encodeURIComponent(row.label)}`)
      if (!startRes.ok) {
        setError(await errText(startRes))
        if (accessToken) void releaseSubdomain(accessToken, row.label, deviceId).catch(() => undefined)
        return
      }
      await refreshStatus()
      // Daemon now owns the renewal for the new subdomain (K2 #674).
      setBoundMsg(`Swapped to ${row.label}.k2.dev.`)
      void refreshSubdomains()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Swap failed')
    } finally {
      setSwapBusy(false)
    }
  }

  return (
    <div className="w-full">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1 flex items-center gap-2">
        K2 Toge
        <span className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]">
          beta
        </span>
      </h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4">
        Expose THIS device&apos;s K2 daemon at a public URL so you can reach it from another
        computer (the server switcher) or your phone (K2 Companion). One tunnel, both clients.
      </p>

      <div className="space-y-5">
        {/* K2 #628: the EXPOSE controls (account login, tunnel config,
            start/stop) only apply to THIS Mac's own daemon. On a remote
            host show a short note instead and let the user switch back. */}
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
                  <div className="text-[10px] text-red-400 px-2 py-1 border border-red-400/20 bg-red-400/5">{authError}</div>
                )}
                <button
                  type="submit"
                  disabled={authBusy || !email.trim() || !password}
                  className="px-3 py-1 text-[11px] text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
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
                    onChange={(v) => void handleSubdomainPick(v)}
                  />
                )}
              </SettingRow>
              {boundMsg && (
                <div className="text-[10px] text-green-400 py-1">{boundMsg}</div>
              )}
            </>
          )}
        </SettingsGroup>

        {/* ── Live status ──────────────────────────────────────────── */}
        <div className="flex items-center gap-2 px-3 py-2 border border-[var(--color-border)]">
          <span
            className="w-2 h-2 flex-shrink-0 rounded-full"
            style={{ backgroundColor: running ? '#22c55e' : '#6b7280' }}
          />
          <span className="text-xs text-[var(--color-text-secondary)]">
            {running ? 'Tunnel running' : 'Tunnel stopped'}
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

        {frpcMissing && (
          <div className="flex items-start gap-2 px-3 py-2 border border-amber-400/30 bg-amber-400/5">
            <span className="text-amber-400 text-sm leading-none flex-shrink-0 mt-0.5">&#9888;</span>
            <div className="text-[10px] text-amber-300/80 leading-relaxed">
              <strong className="text-amber-300">frpc not installed.</strong>{' '}
              K2 Connect needs the <span className="font-mono">frpc</span> client on your PATH.
              Install it from <span className="font-mono">github.com/fatedier/frp/releases</span>{' '}
              (or via <span className="font-mono">brew install frpc</span>) and try again.
            </div>
          </div>
        )}

        {error && <div className="text-[10px] text-red-400 px-3 py-1.5 border border-red-400/20 bg-red-400/5">{error}</div>}

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
                <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="white" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
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
              className="px-3 py-1 text-[11px] text-white bg-red-500/80 hover:bg-red-500 no-drag cursor-pointer disabled:opacity-60"
            >
              {swapBusy ? 'Swapping…' : 'Stop tunnel'}
            </button>
          ) : (
            <button
              onClick={() => void startTunnel()}
              disabled={busy || swapBusy}
              className="px-3 py-1 text-[11px] text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
            >
              Start tunnel
            </button>
          )}
        </div>

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
                    {tokenSet && <span className="ml-2 text-[10px] text-green-400">Set</span>}
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
                    placeholder="server address"
                    value={serverAddr}
                    onChange={(e) => setServerAddr(e.target.value)}
                  />
                  <input
                    className={inputCls}
                    style={{ maxWidth: 64 }}
                    placeholder="port"
                    value={serverPort}
                    onChange={(e) => setServerPort(e.target.value)}
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
                {savedMsg && <span className="text-[10px] text-green-400">{savedMsg}</span>}
              </div>
            </SettingsGroup>
          )}
        </div>
          </>
        )}

        {/* ── Users / Access — role-gated multi-user list (#617 / #629) ─── */}
        <SettingsGroup title="Users / Access">
          <div data-settings-id="k2-connect.users" className="space-y-3">
            {/* K2 #629: only viewers who can manage users (Owner|Admin)
                see the management surface. A Member viewer (or an
                unresolved/forbidden whoami) gets a clean note — never the
                raw token/403 error. */}
            {whoamiLoaded && !canManageUsers(viewerRole) ? (
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
                        <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="white" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
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
                {policySavedMsg && <span className="text-[10px] text-green-400">{policySavedMsg}</span>}
                {policyError && <span className="text-[10px] text-red-400">{policyError}</span>}
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
              <div className="flex gap-1.5 w-full">
                <input
                  className={`${inputCls} flex-1 min-w-0`}
                  placeholder="username"
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  value={newUsername}
                  onChange={(e) => setNewUsername(e.target.value)}
                />
                <div className="relative flex-1 min-w-0">
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
                <button
                  type="submit"
                  disabled={addBusy || !newUsername.trim() || !newPassword}
                  className="flex-shrink-0 px-3 py-1 text-[11px] text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
                >
                  {addBusy ? 'Adding…' : 'Add user'}
                </button>
              </div>
              {addError && (
                <div className="text-[10px] text-red-400 px-2 py-1 border border-red-400/20 bg-red-400/5">{addError}</div>
              )}
              {addedMsg && <div className="text-[10px] text-green-400">{addedMsg}</div>}
            </form>

            {usersError && (
              <div className="text-[10px] text-red-400 px-2 py-1 border border-red-400/20 bg-red-400/5">{usersError}</div>
            )}
            {resetMsg && <div className="text-[10px] text-green-400">{resetMsg}</div>}

            {/* User list */}
            {usersLoaded && users.length === 0 ? (
              <div className="text-[10px] text-[var(--color-text-muted)] py-1">
                No users yet — add one above.
              </div>
            ) : (
              <div className="divide-y divide-[var(--color-border)]">
                {users.map((u) => (
                  <div key={u.username} className="py-2 space-y-2">
                    <div className="flex items-center justify-between gap-3">
                      <span className="flex items-center gap-2 min-w-0">
                        <span className="text-xs text-[var(--color-text-primary)] font-mono truncate">{u.username}</span>
                        {u.disabled && (
                          <span className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-text-muted)]/15 text-[var(--color-text-muted)]">
                            disabled
                          </span>
                        )}
                      </span>
                      <div className="flex items-center gap-3 flex-shrink-0">
                        {/* K2 #629 — role selector. Editable only for an
                            Owner viewer; Admins see the role read-only. */}
                        {canChangeRoles(viewerRole) ? (
                          <SettingDropdown
                            value={u.role ?? 'member'}
                            options={[
                              { value: 'member', label: 'Member' },
                              { value: 'admin', label: 'Admin' },
                              { value: 'owner', label: 'Owner' },
                            ]}
                            onChange={(value) => void changeRole(u.username, value as K2Role)}
                          />
                        ) : (
                          <span className="text-[9px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-text-secondary)]">
                            {u.role ?? 'member'}
                          </span>
                        )}
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
                              <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="white" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                                <path d="M2.5 6.5 L5 9 L9.5 3.5" />
                              </svg>
                            )}
                          </span>
                          <span className="text-[10px] text-[var(--color-text-secondary)]">Enabled</span>
                        </label>
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
                              className="text-[10px] text-red-400 hover:underline no-drag cursor-pointer"
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
                            className="text-[10px] text-[var(--color-text-muted)] hover:text-red-400 hover:underline no-drag cursor-pointer"
                          >
                            Remove
                          </button>
                        )}
                      </div>
                    </div>
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
            </>
            )}
          </div>
        </SettingsGroup>
      </div>
    </div>
  )
}
