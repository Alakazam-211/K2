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
// Address book vs remote peers:
//   - Active host = Local → left pane is THIS Mac's saved servers (same
//     source as the top-bar ServerSwitcher) + add/edit form.
//   - Active host = remote → left pane is THAT server's federation peers
//     (`GET /cli/federation/peers` on the active daemon) so operators can
//     see who the cloud is federated with. Pairing a NEW peer still uses
//     "Pair from this Mac" (local address-book credentials for the other
//     end). External agents on the right stay host-aware.
//
// FOLLOW-UP (review soon): discover/pair/"is it a peer?" is hard unless
// this device already has the other server signed in. Top bar must stay
// client-local. See docs/known-issues/connect-servers-pair-ux-followup.md
//
// Selecting/connecting a host reuses the top-bar switcher path
// (`pickHost`) so a host without a remembered token drops into the same
// full-screen sign-in.

import React, { useCallback, useEffect, useRef, useState } from 'react'
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
import {
  autoPairWithHost,
  federatedPeerHost,
  isTrustedPeerHost,
  listFederationPeers,
  savedHostBaseUrl,
  type FederationPeer,
} from '@/lib/federation'
import { isAirgap } from '@/lib/airgap'
import { isConnectionLevelError } from '@/lib/remote-retry'
import { reviveRemoteSession } from '@/lib/remote-session'
import { recoveryStatusText } from '@/lib/remote-recovery'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import {
  updatePhaseCopy,
  updateCompleteCopy,
  updateHostConfirmCopy,
  isTerminalPhase,
  isFailurePhase,
  isStaged,
  isForbiddenError,
  isAuthError,
  updateForbiddenCopy,
  shouldAutoApplyAfterStage,
  shouldResolveComeback,
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
  'px-2 py-1 text-[11px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-50 disabled:cursor-default'
const BTN_DANGER =
  'px-2 py-1 text-[11px] text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-default'
// Orange/amber, mirroring the remote Restart/Update buttons on the General
// settings tab (GeneralSection.tsx RestartHostRow/UpdateHostRow) so the
// per-host Restart + Sign-in read as the same "remote host" action color.
// Same amber tokens as General; only the spacing (px-2 py-1) and disabled
// cursor match this file's sibling BTN_* consts.
const BTN_ORANGE =
  'px-2 py-1 text-[11px] font-medium text-[var(--color-status-warn-amber-bright)] bg-[color-mix(in_srgb,var(--color-status-warn)_15%,transparent)] border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-warn)_25%,transparent)] transition-colors no-drag cursor-pointer disabled:opacity-50 disabled:cursor-default'

export const CONNECTIONS_MANIFEST: SettingEntry[] = [
  { id: 'connections.add', section: 'connections', label: 'Add a Server', description: 'Save a remote K2 daemon to connect to', keywords: ['server', 'remote', 'connect', 'host', 'add', 'k2 connect', 'address book'] },
  { id: 'connections.remember-password', section: 'connections', label: 'Remember Password', description: 'Store a server token in your OS keychain', keywords: ['token', 'password', 'keychain', 'remember', 'credentials'] },
  { id: 'connections.list', section: 'connections', label: 'Saved Servers', description: 'Edit or remove saved K2 servers', keywords: ['servers', 'hosts', 'edit', 'remove', 'list'] },
  {
    id: 'connections.pair-federated-peer',
    section: 'connections',
    label: 'Pair as federated peer',
    description:
      'Establish mutual federation trust between the active host (This Mac or a signed-in server) and another saved server so cross-server agents can connect — including cloud-to-cloud',
    keywords: ['federation', 'peer', 'pair', 'federated', 'cross-server', 'trust', 'agents', 'cloud'],
  },
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

  // Device address book = per-CLIENT (this Mac). Federation peer list =
  // per ACTIVE daemon. Right pane (FederationOverview) is always host-aware.
  const isLocalActive = activeHost === 'local'
  const activeHostLabel = isLocalActive ? 'This Mac' : activeHost.label || activeHost.hostname

  const [draft, setDraft] = useState<DraftHost | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // Bumped after a successful "Pair as federated peer" so FederationOverview
  // reloads peers without a full Settings remount.
  const [fedRefreshKey, setFedRefreshKey] = useState(0)

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
    const isNew = !draft.id

    // New hosts must prove credentials before they enter the address book —
    // otherwise a wrong password still creates a tile, and a second "add"
    // attempt leaves two lookalike servers.
    if (isNew && !passwordEntered) {
      setError('Password is required when adding a server.')
      return
    }

    // Block duplicate URL (same host:port:scheme) on ADD. Edits keep their id.
    if (isNew) {
      const dup = hosts.find(
        (h) =>
          h.hostname === parsed.hostname &&
          h.port === parsed.port &&
          h.secure === parsed.secure,
      )
      if (dup) {
        setError(
          `A server for ${parsed.hostname}${parsed.port === 443 && parsed.secure ? '' : `:${parsed.port}`} is already saved as “${dup.label}”. Edit that tile (or remove it) instead of adding a duplicate.`,
        )
        return
      }
    }

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
    //
    // addHost is required before loginToHost (it keys the host by id). On a
    // FAILED login for a NEW host we remove the provisional tile so a wrong
    // password never sticks in the list.
    if (passwordEntered) {
      setBusy(true)
      setError(null)
      addHost(host)
      const result = await loginToHost(host, passwordEntered)
      setBusy(false)
      if (!result.ok) {
        if (isNew) {
          removeHost(id)
        }
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
    <div className="w-full h-full min-h-0 flex flex-col">
      {/* Middle split: left = address book (local) OR active-host peers (remote)
          | right = external agents on the active daemon. */}
      <div className="flex flex-1 min-h-0">
      <div className="flex-1 min-w-0 overflow-y-auto pr-3 [scrollbar-gutter:stable]">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Servers</h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4">
        {isLocalActive
          ? 'K2 servers this Mac connects to. Passwords stay in the OS keychain only when “Remember password” is on. Peer trust on each tile is relative to This Mac.'
          : (
            <>
              Federated peers of{' '}
              <span className="text-[var(--color-text-secondary)]">{activeHostLabel}</span>
              {' '}(this cloud’s server graph). This Mac’s address book stays in the
              top-bar switcher; use “Pair from this Mac” below to add a new peer.
            </>
          )}
      </p>

      {isLocalActive ? (
      <>
      {/* Local — always present, never editable. */}
      <div className="flex items-center gap-2 mb-2 px-3 py-2 border border-[var(--color-border)]">
        <span
          className="w-2 h-2 flex-shrink-0 rounded-full"
          style={{ backgroundColor: statusColor(connectionStatus) }}
        />
        <div className="flex flex-col min-w-0">
          <span className="text-xs text-[var(--color-text-primary)]">Local</span>
          <span className="text-[10px] text-[var(--color-text-muted)]">This Mac · bundled daemon</span>
        </div>
        <span className="ml-auto text-[10px] text-[var(--color-text-muted)]">Active</span>
      </div>

      {/* Saved hosts — sorted alphabetically by display label (case-insensitive). */}
      <div className="space-y-2" data-settings-id="connections.list">
        {(Array.isArray(hosts) ? hosts.slice() : [])
          .sort((a, b) =>
            (a.label || a.hostname).localeCompare(b.label || b.hostname, undefined, { sensitivity: 'base' })
          )
          .map((h) => (
            <HostTile
              key={h.id}
              host={h}
              isActive={false}
              connectionStatus={connectionStatus}
              activePeerSideLabel={activeHostLabel}
              onEdit={() => beginEdit(h)}
              onRemove={() => removeHost(h.id)}
              onFederationPeersChanged={() => setFedRefreshKey((n) => n + 1)}
            />
          ))}
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
            placeholder="K2 Server URL (e.g. http://192.168.1.50:60710)"
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
                <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="var(--color-on-accent)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M2.5 6.5 L5 9 L9.5 3.5" />
                </svg>
              )}
            </span>
            Remember password (OS keychain)
          </label>
          {error && <div className="text-[10px] text-[var(--color-status-error-soft)]">{error}</div>}
          <div className="flex gap-2 pt-1">
            <button
              onClick={() => void save()}
              disabled={busy}
              className="px-3 py-1 text-[11px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
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
      ) : (
        <ActiveHostPeersPanel
          hostLabel={activeHostLabel}
          refreshKey={fedRefreshKey}
          localHosts={Array.isArray(hosts) ? hosts : []}
          onPeersChanged={() => setFedRefreshKey((n) => n + 1)}
          onConnectLocal={() => pickHost('local')}
        />
      )}
      </div>

      <div className="flex-1 min-w-0 overflow-y-auto border-l border-[var(--color-border)] pl-6 pr-3 [scrollbar-gutter:stable]">
        <FederationOverview refreshKey={fedRefreshKey} />
      </div>
      </div>
    </div>
  )
}

/** When the active host is a REMOTE cloud, list THAT daemon's federation
 *  peers (not this Mac's address book). Pair-from-this-Mac uses local
 *  saved hosts only for the Pair action — no per-tile settings/get probes
 *  (those caused CORS storms against offline rpm tiles while on iascm). */
function ActiveHostPeersPanel({
  hostLabel,
  refreshKey,
  localHosts,
  onPeersChanged,
  onConnectLocal,
}: {
  hostLabel: string
  refreshKey: number
  localHosts: ConnectHost[]
  onPeersChanged: () => void
  onConnectLocal: () => void
}): React.JSX.Element {
  const [loading, setLoading] = useState(true)
  const [available, setAvailable] = useState(true)
  const [peers, setPeers] = useState<FederationPeer[]>([])
  const [pairBusyHost, setPairBusyHost] = useState<string | null>(null)
  const [pairMsg, setPairMsg] = useState<{ ok: boolean; text: string } | null>(null)

  const reload = useCallback(async () => {
    setLoading(true)
    const res = await listFederationPeers()
    if (!res.available) {
      setAvailable(false)
      setPeers([])
    } else {
      setAvailable(true)
      setPeers(
        res.data
          .slice()
          .sort((a, b) =>
            (a.label || a.subdomain || a.fingerprint).localeCompare(
              b.label || b.subdomain || b.fingerprint,
              undefined,
              { sensitivity: 'base' },
            ),
          ),
      )
    }
    setLoading(false)
  }, [])

  useEffect(() => {
    void reload()
  }, [reload, refreshKey, hostLabel])

  const pairableLocal = localHosts.filter((h) => h.token.length > 0)

  const handlePair = async (h: ConnectHost): Promise<void> => {
    setPairBusyHost(h.id)
    setPairMsg(null)
    try {
      await autoPairWithHost(h.hostname)
      setPairMsg({
        ok: true,
        text: `Paired ${hostLabel} ↔ ${h.label || h.hostname}.`,
      })
      onPeersChanged()
      await reload()
    } catch (e) {
      setPairMsg({
        ok: false,
        text: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setPairBusyHost(null)
    }
  }

  const trustColor = (t: string): string => {
    if (t === 'trusted') return 'var(--color-status-ok-soft)'
    if (t === 'pending') return 'var(--color-status-warn-amber-bright)'
    return 'var(--color-text-muted)'
  }

  return (
    <div data-settings-id="connections.active-host-peers">
      <div className="flex items-center gap-2 mb-3 px-3 py-2 border border-[var(--color-border)]">
        <span
          className="w-2 h-2 flex-shrink-0 rounded-full"
          style={{ backgroundColor: '#3fb950' }}
        />
        <div className="flex flex-col min-w-0">
          <span className="text-xs text-[var(--color-text-primary)] truncate">{hostLabel}</span>
          <span className="text-[10px] text-[var(--color-text-muted)]">Active remote · federation peers</span>
        </div>
        <button onClick={onConnectLocal} className={`ml-auto ${BTN_SECONDARY}`}>
          This Mac
        </button>
      </div>

      <div className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)] mb-2 px-1">
        Peers of {hostLabel}
      </div>
      {loading ? (
        <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2">Loading peers…</div>
      ) : !available ? (
        <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2 leading-relaxed">
          Federation peers are unavailable on this host (federation off, not owner/admin, or unreachable).
          Enable federation under {isAirgap() ? 'Servers → Policies' : 'Tunnel → Policies'} while signed into this server.
        </div>
      ) : peers.length === 0 ? (
        <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2">
          No federated peers yet. Pair a server from this Mac below.
        </div>
      ) : (
        <div className="space-y-2 mb-4">
          {peers.map((p) => (
            <div
              key={p.fingerprint}
              className="px-3 py-2 border border-[var(--color-border)] space-y-1"
            >
              <div className="flex items-center justify-between gap-2">
                <span className="text-xs text-[var(--color-text-primary)] font-medium truncate">
                  {p.label || p.subdomain || p.fingerprint.slice(0, 12)}
                </span>
                <span
                  className="text-[9px] uppercase tracking-wider font-semibold flex-shrink-0"
                  style={{ color: trustColor(p.trust) }}
                >
                  {p.trust}
                </span>
              </div>
              <div className="text-[10px] text-[var(--color-text-muted)] font-mono truncate">
                {p.base_url || p.baseUrl
                  ? (p.base_url || p.baseUrl)
                  : federatedPeerHost(p) || p.fingerprint.slice(0, 24) + '…'}
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="border-t border-[var(--color-border)] pt-3 mt-2">
        <div className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)] mb-1 px-1">
          Pair from this Mac
        </div>
        <p className="text-[10px] text-[var(--color-text-muted)] mb-2 px-1 leading-relaxed">
          Mutual trust between {hostLabel} and another server you&apos;re signed into on this client.
          Does not probe offline hosts for settings — Pair only.
        </p>
        {pairableLocal.length === 0 ? (
          <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2">
            No signed-in servers on this Mac. Switch to This Mac to add/sign in, then return here to pair.
          </div>
        ) : (
          <div className="space-y-2">
            {pairableLocal.map((h) => (
              <div
                key={h.id}
                className="flex items-center justify-between gap-2 px-3 py-2 border border-[var(--color-border)]"
              >
                <div className="min-w-0">
                  <div className="text-xs text-[var(--color-text-primary)] truncate">
                    {h.label || h.hostname}
                  </div>
                  <div className="text-[10px] text-[var(--color-text-muted)] font-mono truncate">
                    {h.secure ? h.hostname : savedHostBaseUrl(h)}
                  </div>
                </div>
                <button
                  type="button"
                  disabled={pairBusyHost === h.id}
                  onClick={() => void handlePair(h)}
                  className={`${BTN_ACCENT} flex-shrink-0`}
                >
                  {pairBusyHost === h.id ? 'Pairing…' : 'Pair'}
                </button>
              </div>
            ))}
          </div>
        )}
        {pairMsg && (
          <div
            className={`mt-2 text-[10px] px-1 ${
              pairMsg.ok ? 'text-[var(--color-status-ok-soft)]' : 'text-[var(--color-status-error-soft)]'
            }`}
          >
            {pairMsg.text}
          </div>
        )}
      </div>
    </div>
  )
}

/** One saved-server tile. Owns its own per-host operation state (restart /
 *  update-check / update / federation badge / pair-as-peer) and drives THAT
 *  host's daemon via its OWN `{base, token}` (host-ops `remoteCreds(h)`), never
 *  the active connection — so the owner can operate a connected server
 *  straight from its tile without switching to it. A signed-out host (no
 *  token) disables the owner-gated controls and shows a "sign in" hint.
 *
 *  "Pair as federated peer" establishes mutual trust between the ACTIVE
 *  daemon (Local or whichever remote you're signed into) and this saved host —
 *  the missing step between "Federation: on" and the Federated Connections
 *  server picker. Hidden when this tile is the active host (can't pair self). */
function HostTile({
  host,
  isActive,
  connectionStatus,
  activePeerSideLabel,
  onEdit,
  onRemove,
  onFederationPeersChanged,
}: {
  host: ConnectHost
  isActive: boolean
  connectionStatus: ConnectionStatus
  /** Display name of the ACTIVE daemon for Peer/Pair copy ("This Mac" / server label). */
  activePeerSideLabel: string
  onEdit: () => void
  onRemove: () => void
  onFederationPeersChanged?: () => void
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
  // From the last successful check — drives Shape A vs B (auto-apply) and the
  // version-gated comeback after apply. Absent on older hosts → treat as Shape B.
  const [installKind, setInstallKind] = useState<UpdateCheckResult['installKind'] | undefined>(
    undefined,
  )
  const [updateBusy, setUpdateBusy] = useState(false)
  const [updateError, setUpdateError] = useState<string | null>(null)
  const [phaseText, setPhaseText] = useState<string | null>(null)
  // True while polling /boot-status back to 'ready' after a restart/update we
  // triggered — drives the "reconnecting…" UX and disables re-triggering.
  const [reconnecting, setReconnecting] = useState(false)
  const confirm = useConfirmDialogStore((s) => s.confirm)
  // Federation peer pin status relative to the ACTIVE daemon (not this tile's
  // host settings). 'checking' while listFederationPeers is in flight.
  const [peerPaired, setPeerPaired] = useState<'checking' | 'yes' | 'no'>('checking')
  const [pairBusy, setPairBusy] = useState(false)
  const [pairMsg, setPairMsg] = useState<{ ok: boolean; text: string } | null>(null)

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
  // Only runs when HostTile is mounted (local address book) — remote mode
  // uses ActiveHostPeersPanel and does not probe every saved host.
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

  // Does the ACTIVE daemon already trust this host as a federated peer?
  // Re-check when sign-in state changes or after a successful pair.
  const refreshPeerPaired = async (): Promise<void> => {
    if (signedOut) {
      if (aliveRef.current) setPeerPaired('no')
      return
    }
    if (aliveRef.current) setPeerPaired('checking')
    try {
      const yes = await isTrustedPeerHost(host.hostname)
      if (aliveRef.current) setPeerPaired(yes ? 'yes' : 'no')
    } catch {
      if (aliveRef.current) setPeerPaired('no')
    }
  }

  useEffect(() => {
    void refreshPeerPaired()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [creds.base, creds.token, host.hostname, signedOut])

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

  const doPairPeer = async (): Promise<void> => {
    setPairBusy(true)
    setPairMsg(null)
    try {
      await autoPairWithHost(host.hostname)
      if (!aliveRef.current) return
      setPeerPaired('yes')
      setPairMsg({
        ok: true,
        text: `Paired ${activePeerSideLabel} ↔ ${label} — available in workspace Federated Connections.`,
      })
      onFederationPeersChanged?.()
    } catch (e) {
      if (!aliveRef.current) return
      const m = errMsg(e)
      clearIfAuthError(m)
      setPairMsg({ ok: false, text: m })
    } finally {
      if (aliveRef.current) setPairBusy(false)
    }
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
  //
  // When `expectedVersion` is set (post-update path), success is VERSION-GATED
  // via shouldResolveComeback (Baden rule): ready+old without sawDown keeps
  // watching; ready+expected → success; ready+wrong after sawDown → hard fail
  // ("update did not take"). Plain restart (no expected) still accepts any ready.
  const waitForHostReady = async (opts?: { expectedVersion?: string }): Promise<void> => {
    if (reconnecting) return // already polling this host back to life
    setReconnecting(true)
    // Clear the transient errors the dropping server produced — "reconnecting"
    // is the correct UX here, not a hard failure.
    setRestartMsg(null)
    setCheckError(null)
    setUpdateError(null)
    setFederation('unknown') // the server is dropping; re-read once it's back
    setPhaseText(`${label} is restarting — reconnecting…`)

    const expected = opts?.expectedVersion
    const deadline = Date.now() + 4 * 60_000 // cap the total wait at ~4 minutes
    const intervalMs = 2500
    // Baden: old daemon keeps answering ready until swap — track an outage.
    let sawDown = false
    try {
      while (aliveRef.current && Date.now() < deadline) {
        await new Promise((r) => setTimeout(r, intervalMs))
        if (!aliveRef.current) return
        const status = await hostBootStatus(creds)
        if (!aliveRef.current) return
        if (status === null) sawDown = true

        if (expected) {
          const verdict = shouldResolveComeback({
            phase: status?.phase,
            version: status?.version,
            expected,
            sawDown,
          })
          if (verdict === 'keep-watching') continue
          if (verdict === 'wrong-version') {
            setPhaseText(null)
            const observed = status?.version
            setHostCurrent(observed)
            setUpdateError(
              `${label} is back on v${observed ?? 'an unexpected version'} — update did not take` +
                ` (expected v${expected}). Try again or SSH install-daemon.sh --version ${expected}.`,
            )
            void reviveRemoteSession(host.id, { force: true })
            void refreshFederation()
            return
          }
          // success
          setPhaseText(null)
          if (status?.version) setHostCurrent(status.version)
          setRestartMsg({
            ok: true,
            text: updateCompleteCopy(label, expected, status?.version),
          })
          setSummary(null) // hide stale "Update to X" until next check
          void reviveRemoteSession(host.id, { force: true })
          void refreshFederation()
          return
        }

        if (status?.phase === 'ready') {
          // Plain restart path — any ready is enough (no version gate).
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
        expected
          ? `${label} did not come back on v${expected} within the wait window — try “Check for updates”, or SSH install-daemon.sh.`
          : `${label} is still unreachable — try “Check for updates”, or relaunch K2 if it persists.`,
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
      // Shape A vs B: Connections auto-applies only for standalone (and
      // older hosts that omit installKind). bundled-app never gets apply.
      setInstallKind(r.installKind)
    } catch (e) {
      const m = errMsg(e)
      clearIfAuthError(m)
      setCheckError(isForbiddenError(m) ? updateForbiddenCopy(label) : `Update check failed: ${m}`)
    } finally {
      setCheckBusy(false)
    }
  }

  /**
   * Poll update status after start. Shape B (standalone / unknown): when
   * phase hits `staged`, POST apply once with `{ job_id }` — without this the
   * binary never swaps (#55 Bug A). Shape A (bundled-app): never apply; keep
   * polling until terminal (co-located Tauri app installs itself).
   *
   * Connection errors BEFORE apply are hard failures (not "reconnecting").
   * Connection errors AFTER apply (or after Shape A stage, when the app is
   * installing) hand off to version-gated waitForHostReady.
   */
  const pollStatus = async (
    jobId: string,
    kind: UpdateCheckResult['installKind'] | undefined,
  ): Promise<'applied' | 'failed' | 'unreachable'> => {
    let appliedSent = false
    let sawStaged = false
    for (let i = 0; i < 90; i++) {
      if (!aliveRef.current) return 'failed'
      await new Promise((r) => setTimeout(r, 2000))
      if (!aliveRef.current) return 'failed'
      let status: UpdateStatusResult
      try {
        status = await hostOpGet<UpdateStatusResult>(creds, 'daemon/update/status', {
          job_id: jobId,
        })
      } catch (e) {
        // Only treat as "host going down for install" AFTER apply (Shape B)
        // or after staged on Shape A (app self-installs). Pre-apply blips are
        // real failures — never claim reconnecting success.
        if (appliedSent || (sawStaged && !shouldAutoApplyAfterStage(kind))) {
          return 'unreachable'
        }
        const m = errMsg(e)
        clearIfAuthError(m)
        if (aliveRef.current) {
          setPhaseText(null)
          setUpdateError(
            isForbiddenError(m)
              ? updateForbiddenCopy(label)
              : `Lost connection to ${label} before the update could install: ${m}`,
          )
        }
        return 'failed'
      }
      if (!aliveRef.current) return 'failed'
      const pct = typeof status.progress === 'number' ? status.progress * 100 : undefined
      setPhaseText(updatePhaseCopy(status.phase, label, { progress: pct, current: hostCurrent }))

      if (isFailurePhase(status.phase)) {
        setPhaseText(null)
        setUpdateError(
          status.error
            ? `Update error on ${label}: ${status.error}`
            : updatePhaseCopy(status.phase, label, { current: hostCurrent }),
        )
        return 'failed'
      }

      if (isStaged(status.phase)) {
        sawStaged = true
        if (shouldAutoApplyAfterStage(kind) && !appliedSent) {
          appliedSent = true
          setPhaseText(`Installing update on ${label}…`)
          try {
            // Shape B: start only stages; apply swaps the binary + restarts.
            await hostOpPost(creds, 'daemon/update/apply', 30000, { job_id: jobId })
          } catch (e) {
            const m = errMsg(e)
            // Apply often races the host drop: connection error AFTER apply
            // was attempted is the install/restart path, not a hard failure.
            if (isConnectionLevelError(e) && !isForbiddenError(m) && !isAuthError(m)) {
              return 'unreachable'
            }
            clearIfAuthError(m)
            if (aliveRef.current) {
              setPhaseText(null)
              setUpdateError(
                isForbiddenError(m)
                  ? updateForbiddenCopy(label)
                  : `Couldn't install the update on ${label}: ${m}`,
              )
            }
            return 'failed'
          }
          continue
        }
        // Shape A: stay in the poll loop until terminal / host goes away.
      }

      if (status.phase === 'restarting' || status.phase === 'done') {
        return 'applied'
      }

      // Other terminal (shouldn't reach here after isFailurePhase) — stop.
      if (isTerminalPhase(status.phase)) {
        return 'failed'
      }
    }
    if (aliveRef.current) {
      setPhaseText(null)
      setUpdateError(`Timed out waiting for the update on ${label}.`)
    }
    return 'failed'
  }

  const doUpdate = async (): Promise<void> => {
    const latest = summary?.kind === 'available' ? summary.latest : undefined
    // Confirm before bouncing a live box (same copy as General Install & restart).
    const copy = updateHostConfirmCopy(label, host.hostname, latest ?? 'the new version')
    const ok = await confirm({
      title: copy.title,
      message: copy.message,
      confirmLabel: copy.confirmLabel,
      destructive: true,
    })
    if (!ok) return

    setUpdateBusy(true)
    setUpdateError(null)
    setRestartMsg(null)
    setPhaseText(`Starting update for ${label}…`)
    const expectedVersion = latest
    const kind = installKind
    try {
      const res = await hostOpPost<{ job_id?: string }>(creds, 'daemon/update/start', 30000)
      const jobId = res?.job_id
      if (!jobId) {
        setPhaseText(null)
        setUpdateError(`${label} did not return an update job id.`)
        return
      }
      const outcome = await pollStatus(jobId, kind)
      if (!aliveRef.current) return
      if (outcome === 'failed') {
        // pollStatus already set updateError / phaseText
        return
      }
      // applied | unreachable after apply → version-gated reconnect
      setPhaseText(`Installing & restarting ${label}… it'll reconnect automatically.`)
      void waitForHostReady({ expectedVersion })
    } catch (e) {
      const m = errMsg(e)
      clearIfAuthError(m)
      // start() connection failure is a hard error BEFORE apply — do NOT treat
      // pre-apply blips as "update restarting". Auth/403 still surface as-is.
      setPhaseText(null)
      setUpdateError(
        isForbiddenError(m)
          ? updateForbiddenCopy(label)
          : `Couldn't start the update on ${label}: ${m}`,
      )
    } finally {
      if (aliveRef.current) setUpdateBusy(false)
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
          style={{ backgroundColor: isActive ? statusColor(connectionStatus) : 'var(--color-neutral)' }}
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
          {!signedOut && !isActive && peerPaired === 'yes' && (
            <span
              className="text-[9px] px-1.5 py-0.5 border whitespace-nowrap no-drag border-emerald-500/40 text-emerald-300 bg-emerald-500/10"
              title={`${activePeerSideLabel} already has a Trusted federation pin for this server`}
            >
              Peer: trusted
            </span>
          )}
          {/* "Active" badge only — switching to a host happens from the
              server switcher, not here. The tile is for managing the host in
              place (Sign in / Restart / updates / Pair). */}
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
          Signed in ⇒ Pair as peer + Restart + Check for updates (both orange)
          in one bottom-right row, plus Update when one's available. */}
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
          {(checkError || updateError || (restartMsg ? !restartMsg.ok : false) || (pairMsg ? !pairMsg.ok : false)) && (
            <button onClick={() => signInForManagement(host)} className={BTN_ACCENT}>
              Sign in again
            </button>
          )}
          {/* Chicken-and-egg fix: enable federation on both sides still leaves
              the peer store empty until mutual trust is pinned. Pair is relative
              to the ACTIVE daemon (Local or the remote you're signed into) —
              never offered on the active host's own tile (can't pair self). */}
          {!isActive &&
            (peerPaired === 'yes' ? (
              <button
                type="button"
                onClick={() => void doPairPeer()}
                disabled={pairBusy || reconnecting}
                className={BTN_SECONDARY}
                title={`Already paired with ${activePeerSideLabel} — click to re-check mutual trust`}
              >
                {pairBusy ? 'Pairing…' : 'Re-pair peer'}
              </button>
            ) : (
              <button
                type="button"
                onClick={() => void doPairPeer()}
                disabled={pairBusy || reconnecting || federation === 'off'}
                className={BTN_ACCENT}
                data-settings-id="connections.pair-federated-peer"
                title={
                  federation === 'off'
                    ? `Enable federation on this server (and on ${activePeerSideLabel}) first`
                    : `Establish mutual federation trust between ${activePeerSideLabel} and this server`
                }
              >
                {pairBusy ? 'Pairing…' : peerPaired === 'checking' ? 'Checking peer…' : 'Pair as federated peer'}
              </button>
            ))}
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
              ? 'text-[var(--color-status-error-soft)]'
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
        <div className={`text-[10px] text-right ${restartMsg.ok ? 'text-[var(--color-text-muted)]' : 'text-[var(--color-status-error-soft)]'}`}>
          {restartMsg.text}
        </div>
      )}
      {pairMsg && (
        <div
          className={`text-[10px] text-right ${pairMsg.ok ? 'text-[var(--color-text-muted)]' : 'text-[var(--color-status-error-soft)]'}`}
        >
          {pairMsg.text}
        </div>
      )}
      {checkError && <div className="text-[10px] text-right text-[var(--color-status-error-soft)]">{checkError}</div>}
      {summary && !checkError && (
        <div className="text-[10px] text-right text-[var(--color-text-muted)]">{summary.text}</div>
      )}
      {phaseText && <div className="text-[10px] text-right text-[var(--color-text-muted)]">{phaseText}</div>}
      {updateError && <div className="text-[10px] text-right text-[var(--color-status-error-soft)]">{updateError}</div>}
    </div>
  )
}
