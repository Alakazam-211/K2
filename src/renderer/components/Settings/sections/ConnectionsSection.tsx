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
import { useAddServerFocus } from '@/stores/add-server-focus'
import type { SettingEntry } from '../searchManifest'
import {
  remoteCreds,
  hostOpPost,
  hostOpGet,
  summarizeCheck,
  federationBadgeText,
  type CheckSummary,
  type FederationState,
} from '@/lib/host-ops'
import {
  updatePhaseCopy,
  isTerminalPhase,
  isForbiddenError,
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

  const connect = (h: ConnectHost): void => {
    pickHost(h)
  }

  const inputCls =
    'w-full px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

  return (
    <div className="w-full">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Connections</h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4">
        K2 servers this device connects to. Each server&apos;s password is stored in your OS
        keychain only when &ldquo;Remember password&rdquo; is on — never in plain text.
      </p>

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

      {/* Saved hosts */}
      <div className="space-y-2" data-settings-id="connections.list">
        {hosts.map((h) => {
          const isActive = activeHost !== 'local' && activeHost.id === h.id
          return (
            <HostTile
              key={h.id}
              host={h}
              isActive={isActive}
              connectionStatus={connectionStatus}
              onConnect={() => connect(h)}
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
            <span className="w-3.5 h-3.5 flex-shrink-0 border border-[var(--color-border)] bg-[var(--color-bg-surface)] peer-checked:bg-[var(--color-accent)] peer-checked:border-[var(--color-accent)]" />
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
  onConnect,
  onEdit,
  onRemove,
}: {
  host: ConnectHost
  isActive: boolean
  connectionStatus: ConnectionStatus
  onConnect: () => void
  onEdit: () => void
  onRemove: () => void
}): React.JSX.Element {
  const label = host.label || host.hostname
  const creds = remoteCreds(host)
  const signedOut = creds.token.length === 0

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

  const doRestart = async (): Promise<void> => {
    setRestartBusy(true)
    setRestartMsg(null)
    try {
      await hostOpPost(creds, 'daemon/restart')
      setRestartMsg({ ok: true, text: `${label} is restarting — it'll be briefly unreachable, then reconnect.` })
      setFederation('unknown') // the server is dropping; re-read after it's back
    } catch (e) {
      const m = errMsg(e)
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
        // expected terminal state, not an error.
        if (aliveRef.current) {
          setPhaseText(`${label} is installing & restarting… it'll reconnect automatically.`)
        }
        return
      }
      if (!aliveRef.current) return
      const pct = typeof status.progress === 'number' ? status.progress * 100 : undefined
      setPhaseText(updatePhaseCopy(status.phase, label, { progress: pct, current: hostCurrent }))
      if (isTerminalPhase(status.phase)) {
        if (status.error) setUpdateError(`Update error on ${label}: ${status.error}`)
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
          {isActive ? (
            <span className="text-[10px] text-[var(--color-text-muted)]">Active</span>
          ) : (
            <button onClick={onConnect} className={BTN_ACCENT}>
              Connect
            </button>
          )}
          <button onClick={onEdit} className={BTN_SECONDARY}>
            Edit
          </button>
          <button onClick={onRemove} className={BTN_DANGER}>
            Remove
          </button>
        </div>
      </div>

      {/* Per-host management — drives THIS host's daemon over its own creds. */}
      <div className="flex items-center gap-2 flex-wrap">
        <button onClick={() => void doRestart()} disabled={signedOut || restartBusy} className={BTN_SECONDARY}>
          {restartBusy ? 'Restarting…' : 'Restart'}
        </button>
        <button onClick={() => void doCheck()} disabled={signedOut || checkBusy} className={BTN_SECONDARY}>
          {checkBusy ? 'Checking…' : 'Check for updates'}
        </button>
        {summary?.kind === 'available' && (
          <button onClick={() => void doUpdate()} disabled={signedOut || updateBusy} className={BTN_ACCENT}>
            {updateBusy ? 'Updating…' : `Update to ${summary.latest}`}
          </button>
        )}
        {signedOut && (
          <span className="text-[10px] text-[var(--color-text-muted)]">Sign in (Connect) to manage this server</span>
        )}
      </div>

      {/* Inline status — fail loud, never silent. */}
      {restartMsg && (
        <div className={`text-[10px] ${restartMsg.ok ? 'text-[var(--color-text-muted)]' : 'text-red-400'}`}>
          {restartMsg.text}
        </div>
      )}
      {checkError && <div className="text-[10px] text-red-400">{checkError}</div>}
      {summary && !checkError && <div className="text-[10px] text-[var(--color-text-muted)]">{summary.text}</div>}
      {phaseText && <div className="text-[10px] text-[var(--color-text-muted)]">{phaseText}</div>}
      {updateError && <div className="text-[10px] text-red-400">{updateError}</div>}
    </div>
  )
}
