// Settings → Connections — the K2 Connect CLIENT address book (PRD §1,
// build order step #3). Where users set up / edit / remove the K2 servers
// they connect OUT to. (The OTHER direction — exposing THIS device's own
// daemon — lives in Settings → K2 Connect.)
//
// Each saved ConnectHost: label · address · status dot, with Add / Edit /
// Remove and a "Remember password" toggle. Non-secret fields persist to
// ~/.k2so/connect-hosts.json (via the store's connect_hosts_write); the
// token goes to the OS keychain ONLY when "Remember password" is on, else
// it's kept in memory for the session.
//
// Selecting/connecting a host reuses the top-bar switcher path
// (`pickHost`) so a host without a remembered token drops into the same
// full-screen sign-in.

import React, { useEffect, useRef, useState } from 'react'
import {
  useConnectHostStore,
  rememberPassword,
  forgetPassword,
  forgetToken,
  loginToHost,
  type ConnectHost,
  type ConnectionStatus,
} from '@/stores/connect-host'
import { parseServerUrl, isValidUsername } from '@/lib/connect-validate'
import { IconLock } from '@/components/icons/IconLock'
import { FederationOverview } from './FederationOverview'
import { useAddServerFocus } from '@/stores/add-server-focus'
import type { SettingEntry } from '../searchManifest'
import {
  remoteCreds,
  hostOpPost,
  hostOpGet,
  hostBootStatus,
  summarizeCheck,
  federationBadgeText,
  type CheckSummary,
  type FederationState,
} from '@/lib/host-ops'
import { isConnectionLevelError } from '@/lib/remote-retry'
import { reviveRemoteSession } from '@/lib/remote-session'
import { recoveryStatusText } from '@/lib/remote-recovery'
import {
  updatePhaseCopy,
  isTerminalPhase,
  isForbiddenError,
  isAuthError,
  updateForbiddenCopy,
  type UpdateCheckResult,
  type UpdateStatusResult,
} from './update-host'

// Shared small-button styles for the per-host tile controls — matching the
// Save/Add/Cancel buttons in this file and K2ConnectSection.tsx (bordered/
// elevated, accent for primary, red for destructive). Real buttons, not the
// pre-#661 text links.
const BTN_SECONDARY =
  'px-2 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-default'
const BTN_ACCENT =
  'px-2 py-1 text-[11px] text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-50 disabled:cursor-default'
const BTN_DANGER =
  'px-2 py-1 text-[11px] text-red-400 border border-red-400/40 hover:bg-red-400/10 no-drag cursor-pointer disabled:opacity-50 disabled:cursor-default'
// Orange/amber, mirroring the remote Restart/Update buttons on the General
// settings tab (GeneralSection.tsx RestartHostRow/UpdateHostRow) so the
// per-host Restart + Sign-in read as the same "remote host" action color.
// Same amber tokens as General; only the spacing (px-2 py-1) and disabled
// cursor match this file's sibling BTN_* consts.
const BTN_ORANGE =
  'px-2 py-1 text-[11px] font-medium text-amber-200 bg-amber-500/15 border border-amber-500/40 hover:bg-amber-500/25 transition-colors no-drag cursor-pointer disabled:opacity-50 disabled:cursor-default'

export const CONNECTIONS_MANIFEST: SettingEntry[] = [
  { id: 'connections.add', section: 'connections', label: 'Add a Server', description: 'Save a remote K2 daemon to connect to', keywords: ['server', 'remote', 'connect', 'host', 'add', 'k2 connect', 'address book'] },
  { id: 'connections.remember-password', section: 'connections', label: 'Remember Password', description: 'Store a server token in your OS keychain', keywords: ['token', 'password', 'keychain', 'remember', 'credentials'] },
  { id: 'connections.list', section: 'connections', label: 'Saved Servers', description: 'Edit or remove saved K2 servers', keywords: ['servers', 'hosts', 'edit', 'remove', 'list'] },
]

function statusColor(status: ConnectionStatus): string {
  switch (status) {
    case 'connected':
      return '#3fb950'
    case 'connecting':
      return '#d29922'
    case 'offline':
      return '#f85149'
  }
}

type DraftHost = {
  id: string | null // null = creating a new host
  label: string
  /** Raw "K2 Server URL" field (e.g. https://rosson.k2.dev); parsed on
   *  save via parseServerUrl into hostname/secure/port. */
  url: string
  username: string
  password: string
  remember: boolean
}

function emptyDraft(): DraftHost {
  return { id: null, label: '', url: '', username: '', password: '', remember: false }
}

/** Reconstruct the URL field from a saved host for the edit form. */
function hostToUrl(h: ConnectHost): string {
  const scheme = h.secure ? 'https' : 'http'
  const authority = h.secure && h.port === 443 ? h.hostname : `${h.hostname}:${h.port}`
  return `${scheme}://${authority}`
}

export function ConnectionsSection(): React.JSX.Element {
  const hosts = useConnectHostStore((s) => s.hosts)
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const addHost = useConnectHostStore((s) => s.addHost)
  const removeHost = useConnectHostStore((s) => s.removeHost)
  const pickHost = useConnectHostStore((s) => s.pickHost)

  // The device address book (saved-server tiles + add form) is a per-DEVICE
  // concept — only meaningful when the active daemon is local. On a remote host
  // it's replaced by the host-aware FederationOverview.
  const isLocalActive = activeHost === 'local'
  const activeHostLabel = isLocalActive ? 'This Mac' : activeHost.label || activeHost.hostname

  const [draft, setDraft] = useState<DraftHost | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  // Top-bar "Add a server…" → reveal + scroll-to + focus this form.
  const addServerFocusSeq = useAddServerFocus()
  const formRef = useRef<HTMLDivElement | null>(null)
  const labelRef = useRef<HTMLInputElement | null>(null)

  const beginAdd = (): void => {
    setError(null)
    setDraft(emptyDraft())
  }

  // When the top-bar requests focus, open the add form (if not already
  // editing one) so it mounts, then scroll it into view + focus the first
  // input. We respond to every bump of the monotonic request counter.
  useEffect(() => {
    if (addServerFocusSeq === 0) return
    setError(null)
    setDraft((d) => d ?? emptyDraft())
    // Defer to the next frame so the form (and its inputs) are mounted.
    const raf = requestAnimationFrame(() => {
      formRef.current?.scrollIntoView({ behavior: 'smooth', block: 'center' })
      labelRef.current?.focus()
    })
    return () => cancelAnimationFrame(raf)
    // Only re-run when a NEW focus request arrives.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [addServerFocusSeq])

  const beginEdit = (h: ConnectHost): void => {
    setError(null)
    setDraft({
      id: h.id,
      label: h.label,
      url: hostToUrl(h),
      username: h.username ?? '',
      // We never read the password back out of the keychain into a form
      // field; leave it blank. An empty password on save = "leave the
      // remembered password as-is" (see save()).
      password: '',
      remember: h.remember,
    })
  }

  const save = async (): Promise<void> => {
    if (!draft) return
    if (!draft.label.trim()) {
      setError('Label is required')
      return
    }
    const parsed = parseServerUrl(draft.url)
    if (!parsed.ok) {
      setError(parsed.reason)
      return
    }
    const username = draft.username.trim()
    if (!isValidUsername(username)) {
      setError('Username must be 2+ chars: lowercase letters, numbers, _ or -.')
      return
    }
    const passwordEntered = draft.password // do NOT trim — passwords may have edge whitespace

    const id = draft.id ?? `host-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
    const existing = draft.id ? hosts.find((h) => h.id === draft.id) : undefined

    const host: ConnectHost = {
      id,
      label: draft.label.trim(),
      hostname: parsed.hostname,
      port: parsed.port,
      username,
      // Preserve any live in-memory session token across an edit; a fresh
      // login (below) overwrites it. New hosts start token-less.
      token: existing?.token ?? '',
      secure: parsed.secure,
      remember: draft.remember,
      lastConnectedAt: existing?.lastConnectedAt ?? null,
    }

    // When a password was entered, verify the credentials by logging in
    // now (connect-users #617). A blank password on an EDIT keeps the
    // remembered one untouched (no re-login). loginToHost commits the
    // session token + lastConnectedAt into the store on success.
    if (passwordEntered) {
      setBusy(true)
      setError(null)
      addHost(host) // make the host known so loginToHost can update it by id
      const result = await loginToHost(host, passwordEntered)
      setBusy(false)
      if (!result.ok) {
        setError(result.reason)
        return
      }
      // Keychain side: remember the password when asked; else forget it.
      if (draft.remember) {
        await rememberPassword(id, passwordEntered)
      } else {
        await forgetPassword(id)
        await forgetToken(id)
      }
    } else {
      // Edit with no new password: just persist the non-secret fields.
      addHost(host)
      if (!draft.remember) {
        await forgetPassword(id)
        await forgetToken(id)
      }
    }

    setDraft(null)
    setError(null)
  }

  const inputCls =
    'w-full px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

  return (
    <div className="w-full">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Connections</h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4">
        {isLocalActive
          ? 'K2 servers this device connects to. Each server’s password is stored in your OS keychain only when “Remember password” is on — never in plain text. Below: who the local daemon is federated with.'
          : `Viewing ${activeHostLabel}. The saved-servers address book is per-device and hidden while you’re on a remote host — instead, here is who ${activeHostLabel} is federated with.`}
      </p>

      {/* Device address book (Local tile + saved hosts + add/edit form) is a
          per-DEVICE client concept — show it only when the active daemon is
          local. On a remote host it'd be confusing (it's THIS Mac's list, not
          the remote's), so it's replaced by the federation overview below. */}
      {isLocalActive && (
      <>
      {/* Local — always present, never editable. */}
      <div className="flex items-center gap-2 mb-2 px-3 py-2 border border-[var(--color-border)]">
        <span
          className="w-2 h-2 flex-shrink-0 rounded-full"
          style={{ backgroundColor: activeHost === 'local' ? statusColor(connectionStatus) : '#6b7280' }}
        />
        <div className="flex flex-col min-w-0">
          <span className="text-xs text-[var(--color-text-primary)]">Local</span>
          <span className="text-[10px] text-[var(--color-text-muted)]">This Mac · bundled daemon</span>
        </div>
        {activeHost === 'local' ? (
          <span className="ml-auto text-[10px] text-[var(--color-text-muted)]">Active</span>
        ) : (
          <button onClick={() => pickHost('local')} className={`ml-auto ${BTN_ACCENT}`}>
            Connect
          </button>
        )}
      </div>

      {/* Saved hosts — sorted alphabetically by display label (case-insensitive).
          'Local' is the separate pinned tile above this list. */}
      <div className="space-y-2" data-settings-id="connections.list">
        {[...hosts]
          .sort((a, b) =>
            (a.label || a.hostname).localeCompare(b.label || b.hostname, undefined, { sensitivity: 'base' })
          )
          .map((h) => {
          // This address book only renders when the LOCAL daemon is active
          // (isLocalActive gate above), so no saved host can be the active
          // one — TS correctly narrows `activeHost` to 'local' here and the
          // old `activeHost !== 'local' && activeHost.id === h.id` check
          // was provably always false.
          const isActive = false
          return (
            <HostTile
              key={h.id}
              host={h}
              isActive={isActive}
              connectionStatus={connectionStatus}
              onEdit={() => beginEdit(h)}
              onRemove={() => removeHost(h.id)}
            />
          )
        })}
        {hosts.length === 0 && (
          <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2">No saved servers yet.</div>
        )}
      </div>

      {/* Add / Edit form */}
      {draft ? (
        <div ref={formRef} className="mt-4 px-3 py-3 border border-[var(--color-border)] space-y-2" data-settings-id="connections.add">
          <div className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">
            {draft.id ? 'Edit server' : 'Add server'}
          </div>
          <input ref={labelRef} className={inputCls} placeholder="Label (e.g. My Mac Mini)" value={draft.label} onChange={(e) => setDraft({ ...draft, label: e.target.value })} />
          <input
            className={inputCls}
            placeholder="K2 Server URL (e.g. https://rosson.k2.dev)"
            value={draft.url}
            onChange={(e) => setDraft({ ...draft, url: e.target.value })}
          />
          <input
            className={inputCls}
            placeholder="Username"
            autoComplete="username"
            value={draft.username}
            onChange={(e) => setDraft({ ...draft, username: e.target.value })}
          />
          <input
            className={inputCls}
            type="password"
            autoComplete="off"
            placeholder={draft.id ? 'Password (leave blank to keep saved)' : 'Password'}
            value={draft.password}
            onChange={(e) => setDraft({ ...draft, password: e.target.value })}
          />
          <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer select-none" data-settings-id="connections.remember-password">
            <input
              type="checkbox"
              className="peer sr-only"
              checked={draft.remember}
              onChange={(e) => setDraft({ ...draft, remember: e.target.checked })}
            />
            <span
              aria-hidden
              className="w-3.5 h-3.5 flex-shrink-0 flex items-center justify-center border border-[var(--color-border)] bg-[var(--color-bg-surface)] peer-checked:bg-[var(--color-accent)] peer-checked:border-[var(--color-accent)]"
            >
              {draft.remember && (
                <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="white" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M2.5 6.5 L5 9 L9.5 3.5" />
                </svg>
              )}
            </span>
            Remember password (OS keychain)
          </label>
          {error && <div className="text-[10px] text-red-400">{error}</div>}
          <div className="flex gap-2 pt-1">
            <button
              onClick={() => void save()}
              disabled={busy}
              className="px-3 py-1 text-[11px] text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
            >
              {busy ? 'Verifying…' : 'Save'}
            </button>
            <button onClick={() => { setDraft(null); setError(null) }} className="px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer">
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <button onClick={beginAdd} className="mt-4 px-3 py-1.5 text-[11px] text-[var(--color-accent)] border border-[var(--color-accent)]/40 hover:bg-[var(--color-accent)]/10 no-drag cursor-pointer">
          + Add a server
        </button>
      )}
      </>
      )}

      {/* Host-aware federation overview — the ACTIVE daemon's peers + cross-agent
          connections (local or remote). Always shown. */}
      <FederationOverview />
    </div>
  )
}

/** One saved-server tile. Owns its own per-host operation state (restart /
 *  update-check / update / federation badge) and drives THAT host's daemon
 *  via its OWN `{base, token}` (host-ops `remoteCreds(h)`), never the active
 *  connection — so the owner can operate a connected server straight from its
 *  tile without switching to it. A signed-out host (no token) disables the
 *  owner-gated controls and shows a "sign in" hint. */
function HostTile({
  host,
  isActive,
  connectionStatus,
  onEdit,
  onRemove,
}: {
  host: ConnectHost
  isActive: boolean
  connectionStatus: ConnectionStatus
  onEdit: () => void
  onRemove: () => void
}): React.JSX.Element {
  const label = host.label || host.hostname
  const creds = remoteCreds(host)
  const signedOut = creds.token.length === 0
  const signInForManagement = useConnectHostStore((s) => s.signInForManagement)
  // The ACTIVE host's three-state recovery surface (owner contract —
  // lib/remote-recovery.ts). Rendered as a status line on the active tile so
  // the K2 Connect panel always shows whether the connection is restarting /
  // re-authenticating / waiting on a sign-in. Only meaningful for isActive.
  const activeRecovery = useConnectHostStore((s) => s.recovery)

  const [federation, setFederation] = useState<FederationState>('loading')
  const [restartBusy, setRestartBusy] = useState(false)
  const [restartMsg, setRestartMsg] = useState<{ ok: boolean; text: string } | null>(null)
  const [checkBusy, setCheckBusy] = useState(false)
  const [checkError, setCheckError] = useState<string | null>(null)
  const [summary, setSummary] = useState<CheckSummary | null>(null)
  const [hostCurrent, setHostCurrent] = useState<string | undefined>(undefined)
  const [updateBusy, setUpdateBusy] = useState(false)
  const [updateError, setUpdateError] = useState<string | null>(null)
  const [phaseText, setPhaseText] = useState<string | null>(null)
  // True while polling /boot-status back to 'ready' after a restart/update we
  // triggered — drives the "reconnecting…" UX and disables re-triggering.
  const [reconnecting, setReconnecting] = useState(false)

  // Stays false after unmount so async pollers / fetches don't setState on a
  // gone tile.
  const aliveRef = useRef(true)
  useEffect(() => {
    aliveRef.current = true
    return () => {
      aliveRef.current = false
    }
  }, [])

  // Best-effort federation badge: fetch THIS host's settings on mount (and
  // whenever its base/token change). Never blocks the tile render; any
  // failure (signed out / unreachable / field absent) collapses to "—".
  useEffect(() => {
    if (signedOut) {
      setFederation('unknown')
      return
    }
    let cancelled = false
    setFederation('loading')
    void hostOpGet<{ federationEnabled?: boolean }>(creds, 'settings/get')
      .then((s) => {
        if (cancelled) return
        setFederation(s?.federationEnabled === true ? 'on' : s?.federationEnabled === false ? 'off' : 'unknown')
      })
      .catch(() => {
        if (!cancelled) setFederation('unknown')
      })
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [creds.base, creds.token])

  const errMsg = (e: unknown): string => (e instanceof Error ? e.message : String(e))

  // On a 401-class rejection — OR the daemon's token-gate 403 ("Invalid or
  // missing auth token", which a stale session produces and which shares its
  // body with a genuine role denial) — run the session revival: it
  // whoami-confirms staleness, silently re-logs-in with the remembered
  // password, and only drops the token (flipping the tile to signed-out with
  // its Sign-in button) when re-login can't proceed. A network failure
  // ("Load failed"/timeout) is NOT auth — the server may just be restarting —
  // so the token stays intact and the reconnect poll handles it.
  const clearIfAuthError = (m: string): void => {
    if (isAuthError(m) || isForbiddenError(m)) void reviveRemoteSession(host.id)
  }

  // Best-effort re-read of THIS host's federation badge (used by the reconnect
  // poller once the host is back; the mount effect below has its own copy with
  // a per-render `cancelled` guard). aliveRef-guarded so a gone tile is safe.
  const refreshFederation = async (): Promise<void> => {
    if (signedOut) {
      setFederation('unknown')
      return
    }
    setFederation('loading')
    try {
      const s = await hostOpGet<{ federationEnabled?: boolean }>(creds, 'settings/get')
      if (!aliveRef.current) return
      setFederation(s?.federationEnabled === true ? 'on' : s?.federationEnabled === false ? 'off' : 'unknown')
    } catch {
      if (aliveRef.current) setFederation('unknown')
    }
  }

  // STATE-AWARE reconnect after a restart/update WE triggered on this host.
  // The server goes down for a while (download → install → restart → tunnel
  // reconnect) and the WKWebView pool holds DEAD sockets, so the tile would
  // otherwise show "Load failed" / "Can't connect" until a full app relaunch.
  //
  // Instead of guessing a delay, poll the daemon's PUBLIC `/boot-status` until
  // it reports `phase === 'ready'` again — reacting to the server ACTUALLY
  // being back, NOT a fixed timer. Each `hostBootStatus` fetch's
  // throw-then-retry also evicts the dead pooled socket and reopens a fresh
  // one, so the pool is healthy by the time `ready` is observed. aliveRef
  // guards every setState so a navigated-away tile is safe. The total wait is
  // capped (~4 min) so a host that never returns surfaces a recovery hint
  // instead of looping forever.
  const waitForHostReady = async (): Promise<void> => {
    if (reconnecting) return // already polling this host back to life
    setReconnecting(true)
    // Clear the transient errors the dropping server produced — "reconnecting"
    // is the correct UX here, not a hard failure.
    setRestartMsg(null)
    setCheckError(null)
    setUpdateError(null)
    setFederation('unknown') // the server is dropping; re-read once it's back
    setPhaseText(`${label} is restarting — reconnecting…`)

    const deadline = Date.now() + 4 * 60_000 // cap the total wait at ~4 minutes
    const intervalMs = 2500
    try {
      while (aliveRef.current && Date.now() < deadline) {
        await new Promise((r) => setTimeout(r, intervalMs))
        if (!aliveRef.current) return
        const status = await hostBootStatus(creds)
        if (!aliveRef.current) return
        if (status?.phase === 'ready') {
          // The server is genuinely back — clear the reconnecting/phase state,
          // refresh the federation badge + current-version read so the tile
          // reflects the new state, and stop polling.
          setPhaseText(null)
          if (status.version) setHostCurrent(status.version)
          setRestartMsg({
            ok: true,
            text: `${label} is back online${status.version ? ` (v${status.version})` : ''}.`,
          })
          // The restart that just completed WIPED the daemon's in-memory
          // connect-sessions: /boot-status is green but this host's cached
          // session token is dead. Revive it now (whoami-confirm + silent
          // re-login with the remembered password) so the tile's owner ops —
          // and, when this is the ACTIVE host, every pane/WS reconnect loop —
          // get a live session without an app relaunch. Forced: the owner
          // explicitly restarted/updated this host.
          void reviveRemoteSession(host.id, { force: true })
          void refreshFederation()
          return
        }
      }
      if (!aliveRef.current) return
      // Timed out — don't loop forever; give a concrete recovery path.
      setPhaseText(null)
      setUpdateError(
        `${label} is still unreachable — try “Check for updates”, or relaunch K2 if it persists.`,
      )
    } finally {
      if (aliveRef.current) setReconnecting(false)
    }
  }

  const doRestart = async (): Promise<void> => {
    setRestartBusy(true)
    setRestartMsg(null)
    try {
      await hostOpPost(creds, 'daemon/restart')
      // The server is dropping now — poll /boot-status until it's back on
      // 'ready' (state-aware), showing a "reconnecting…" line meanwhile,
      // instead of a guessed delay or a premature "Load failed".
      void waitForHostReady()
    } catch (e) {
      const m = errMsg(e)
      clearIfAuthError(m)
      setRestartMsg({
        ok: false,
        text: isForbiddenError(m)
          ? `You don't have permission to restart ${label}. Only the host owner or an admin can.`
          : `Couldn't restart ${label}: ${m}`,
      })
    } finally {
      setRestartBusy(false)
    }
  }

  const doCheck = async (): Promise<void> => {
    setCheckBusy(true)
    setCheckError(null)
    setSummary(null)
    setPhaseText(null)
    setUpdateError(null)
    try {
      const r = await hostOpPost<UpdateCheckResult>(creds, 'daemon/update/check', 15000)
      setSummary(summarizeCheck(label, r))
      setHostCurrent(r.current)
    } catch (e) {
      const m = errMsg(e)
      clearIfAuthError(m)
      setCheckError(isForbiddenError(m) ? updateForbiddenCopy(label) : `Update check failed: ${m}`)
    } finally {
      setCheckBusy(false)
    }
  }

  const pollStatus = async (jobId: string): Promise<void> => {
    for (let i = 0; i < 90; i++) {
      if (!aliveRef.current) return
      await new Promise((r) => setTimeout(r, 2000))
      if (!aliveRef.current) return
      let status: UpdateStatusResult
      try {
        status = await hostOpGet<UpdateStatusResult>(creds, 'daemon/update/status', { job_id: jobId })
      } catch {
        // The host goes unreachable while it installs & restarts — that's the
        // EXPECTED terminal state, not an error. Hand off to the state-aware
        // reconnect poll: watch /boot-status until the host is back on 'ready'.
        if (aliveRef.current) void waitForHostReady()
        return
      }
      if (!aliveRef.current) return
      const pct = typeof status.progress === 'number' ? status.progress * 100 : undefined
      setPhaseText(updatePhaseCopy(status.phase, label, { progress: pct, current: hostCurrent }))
      if (isTerminalPhase(status.phase)) {
        if (status.error) {
          setUpdateError(`Update error on ${label}: ${status.error}`)
        } else if (status.phase === 'restarting' || status.phase === 'done') {
          // The host is going down to apply the update — start the state-aware
          // reconnect poll instead of leaving a stale "restarting…" line that
          // never clears (failed/rolled-back stay put: nothing to wait for).
          if (aliveRef.current) void waitForHostReady()
        }
        return
      }
    }
  }

  const doUpdate = async (): Promise<void> => {
    setUpdateBusy(true)
    setUpdateError(null)
    setRestartMsg(null)
    setPhaseText(`Starting update for ${label}…`)
    try {
      const res = await hostOpPost<{ job_id?: string }>(creds, 'daemon/update/start', 30000)
      const jobId = res?.job_id
      if (!jobId) {
        setPhaseText(null)
        setUpdateError(`${label} did not return an update job id.`)
        return
      }
      await pollStatus(jobId)
    } catch (e) {
      const m = errMsg(e)
      clearIfAuthError(m)
      // The user explicitly triggered this update. A CONNECTION-level failure
      // here (e.g. "Load failed" because the host already started dropping and
      // the update/start response socket died) is NOT a hard failure — the
      // correct UX is "reconnecting", so poll /boot-status back to ready
      // instead of surfacing the transient network error. Auth/403 are
      // authoritative and still surface immediately.
      if (!isForbiddenError(m) && !isAuthError(m) && isConnectionLevelError(e)) {
        void waitForHostReady()
        return
      }
      setPhaseText(null)
      setUpdateError(isForbiddenError(m) ? updateForbiddenCopy(label) : `Couldn't start the update on ${label}: ${m}`)
    } finally {
      setUpdateBusy(false)
    }
  }

  const fedClass =
    federation === 'on'
      ? 'border-[var(--color-accent)]/50 text-[var(--color-accent)]'
      : 'border-[var(--color-border)] text-[var(--color-text-muted)]'

  return (
    <div className="px-3 py-2 border border-[var(--color-border)] space-y-2">
      <div className="flex items-center gap-2">
        <span
          className="w-2 h-2 flex-shrink-0 rounded-full"
          style={{ backgroundColor: isActive ? statusColor(connectionStatus) : '#6b7280' }}
        />
        <div className="flex flex-col min-w-0">
          <span className="text-xs text-[var(--color-text-primary)] truncate">{host.label}</span>
          <span className="text-[10px] text-[var(--color-text-muted)] truncate flex items-center gap-1">
            {host.secure && <IconLock className="w-2.5 h-2.5 flex-shrink-0" />}
            {host.secure && host.port === 443 ? host.hostname : `${host.hostname}:${host.port}`}
            {host.remember ? ' · saved' : ''}
          </span>
        </div>
        <div className="ml-auto flex items-center gap-2">
          <span
            className={`text-[9px] px-1.5 py-0.5 border whitespace-nowrap no-drag ${fedClass}`}
            title="Whether cross-server federation is enabled on this server"
          >
            {federationBadgeText(federation)}
          </span>
          {/* "Active" badge only — switching to a host happens from the
              server switcher / K2 Connect view, not here. The tile is for
              managing the host in place (Sign in / Restart / updates). */}
          {isActive && (
            <span className="text-[10px] text-[var(--color-text-muted)]">Active</span>
          )}
          <button onClick={onEdit} className={BTN_SECONDARY}>
            Edit
          </button>
          <button onClick={onRemove} className={BTN_DANGER}>
            Remove
          </button>
        </div>
      </div>

      {/* Per-host actions — orange (matches the General-tab remote Restart/
          Update color). Signed out ⇒ just a Sign in button, hint on the left.
          Signed in ⇒ Restart + Check for updates (both orange) in one
          bottom-right row, plus Update when one's available. */}
      {signedOut ? (
        <div className="flex items-center justify-between gap-2">
          <span className="text-[10px] text-[var(--color-text-muted)]">Sign in to manage this server</span>
          <button onClick={() => signInForManagement(host)} className={BTN_ACCENT}>
            Sign in
          </button>
        </div>
      ) : (
        <div className="flex justify-end gap-1 flex-wrap">
          {/* Escape hatch: a signed-in op just failed (e.g. "Load failed" after
              a remote restart/update, when we CAN'T tell if the token went
              stale). Offer re-sign-in so the user is never stranded with a
              present-but-dead token. A true 401 already cleared the token above
              and flipped the tile to the signed-out branch. */}
          {(checkError || updateError || (restartMsg ? !restartMsg.ok : false)) && (
            <button onClick={() => signInForManagement(host)} className={BTN_ACCENT}>
              Sign in again
            </button>
          )}
          <button onClick={() => void doRestart()} disabled={restartBusy || reconnecting} className={BTN_ORANGE}>
            {restartBusy ? 'Restarting…' : reconnecting ? 'Reconnecting…' : 'Restart'}
          </button>
          <button onClick={() => void doCheck()} disabled={checkBusy} className={BTN_ORANGE}>
            {checkBusy ? 'Checking…' : 'Check for updates'}
          </button>
          {summary?.kind === 'available' && (
            <button onClick={() => void doUpdate()} disabled={updateBusy || reconnecting} className={BTN_ACCENT}>
              {updateBusy ? 'Updating…' : `Update to ${summary.latest}`}
            </button>
          )}
        </div>
      )}

      {/* The ACTIVE connection's recovery state — always visible while it's
          not plain 'connected', so the owner can see at a glance whether the
          server is restarting (with boot phase), re-authenticating, or
          waiting on a sign-in (the only state that needs him). Same copy as
          the in-app banner (recoveryStatusText — single source). */}
      {isActive && activeRecovery.kind !== 'connected' && (
        <div
          className={`text-[10px] text-right ${
            activeRecovery.kind === 'signin-required'
              ? 'text-red-400'
              : 'text-[var(--color-text-muted)]'
          }`}
        >
          {recoveryStatusText(label, activeRecovery)}
        </div>
      )}

      {/* All inline status — below the action row, right-justified. Covers
          restart result, check error, update-available/up-to-date summary,
          "Starting update for <server>…" phase text, and update errors.
          Fail loud, never silent. */}
      {restartMsg && (
        <div className={`text-[10px] text-right ${restartMsg.ok ? 'text-[var(--color-text-muted)]' : 'text-red-400'}`}>
          {restartMsg.text}
        </div>
      )}
      {checkError && <div className="text-[10px] text-right text-red-400">{checkError}</div>}
      {summary && !checkError && (
        <div className="text-[10px] text-right text-[var(--color-text-muted)]">{summary.text}</div>
      )}
      {phaseText && <div className="text-[10px] text-right text-[var(--color-text-muted)]">{phaseText}</div>}
      {updateError && <div className="text-[10px] text-right text-red-400">{updateError}</div>}
    </div>
  )
}
