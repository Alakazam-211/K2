// Settings → Connections — the K2 Connect CLIENT address book (PRD §1,
// build order step #3). Where users set up / edit / remove the K2 servers
// they connect OUT to. (The OTHER direction — exposing THIS device's own
// daemon — lives in Settings → K2 Connect.)
//
// One column: Servers, then Agents on other servers. Each saved ConnectHost
// is a row: name, then health (from its public /boot-status), address and
// trust; at most one button for what it needs next, and everything else in
// the row's menu. Add and Edit open a dialog with a "Remember password"
// checkbox. Non-secret fields persist to ~/.k2so/connect-hosts.json (via the
// store's connect_hosts_write); the token goes to the OS keychain ONLY when
// "Remember password" is on, else it's kept in memory for the session.
//
// Address book vs remote peers:
//   - Active host = Local → THIS computer's saved servers (same source as
//     the top-bar ServerSwitcher) + the add/edit dialog.
//   - Active host = remote → THAT server's federation peers
//     (`GET /cli/federation/peers` on the active daemon) so operators can
//     see who the cloud is federated with. Pairing a NEW peer still uses
//     "Pair from this computer" (local address-book credentials for the
//     other end). External agents below stay host-aware.
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
  saveRememberedPassword,
  forgetPassword,
  forgetToken,
  duplicateSavedHost,
  duplicateSavedHostMessage,
  type ConnectHost,
  type ConnectionStatus,
} from '@/stores/connect-host'
import { VOICE_CHAT_LINE, hostIsVoiceChatOnly, isVoiceChatOnly, voiceChatAddedText } from '@/lib/voice-chat-only'
import { VoiceChatOnlyChip } from '@/components/Voice/VoiceChatOnlyChip'
import { homeHostKey } from '@/lib/host-key'
import { abandonRotation, fetchWhoamiRole, verifyHostCredentials } from '@/lib/add-server-login'
import { PasswordRotationStep } from '@/components/PasswordRotationStep'
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
  REPAIR_RECOMMENDED_HINT,
  savedHostBaseUrl,
  sortPeersForPicker,
  type FederationPeer,
} from '@/lib/federation'
import { isAirgap } from '@/lib/airgap'
import { primaryScope } from '@/kessel/server-scope'
import { onFederationPeersChanged } from '@/stores/session-events'
import { reviveRemoteSession } from '@/lib/remote-session'
import { signOutOfHost } from '@/lib/host-pool-instance'
import { recoveryStatusText } from '@/lib/remote-recovery'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import {
  updatePhaseCopy,
  updateCompleteCopy,
  updateHostConfirmCopy,
  isForbiddenError,
  isAuthError,
  updateForbiddenCopy,
  shouldResolveComeback,
  driveUpdateJob,
  readUpdateCheck,
  readUpdateStart,
  UpdateReplyError,
  type UpdateCheckResult,
} from './update-host'
import { useToastStore } from '@/stores/toast'
import {
  Button,
  Callout,
  Checkbox,
  DialogFrame,
  DialogScrim,
  Input,
  OverflowMenu,
  Surface,
  type OverflowMenuItem,
} from '@/components/ui'
import { useDialogEscape } from '../settings-esc'
import {
  ConnectionRow,
  HealthWord,
  MetaDot,
  SectionHeader,
  StateNote,
  StatusDot,
  type HealthTone,
} from './ConnectionsParts'

export const CONNECTIONS_MANIFEST: SettingEntry[] = [
  { id: 'connections.add', section: 'connections', label: 'Add a server', description: 'Save another K2 server to connect to', keywords: ['server', 'remote', 'connect', 'host', 'add', 'k2 connect', 'address book'] },
  { id: 'connections.remember-password', section: 'connections', label: 'Remember password', description: 'Keep a server’s password in this computer’s keychain', keywords: ['token', 'password', 'keychain', 'remember', 'credentials'] },
  { id: 'connections.list', section: 'connections', label: 'Saved servers', description: 'Edit or remove saved K2 servers', keywords: ['servers', 'hosts', 'edit', 'remove', 'list', 'search', 'filter'] },
  {
    id: 'connections.pair-federated-peer',
    section: 'connections',
    label: 'Pair a server',
    description:
      'Let this computer, or a server you’re signed in to, trust another saved server so their agents can connect',
    keywords: ['federation', 'peer', 'pair', 'federated', 'federated peer', 'cross-server', 'trust', 'agents', 'cloud'],
  },
]

/** The window's own connection, in the words and colours of a server's health. */
function connectionHealth(status: ConnectionStatus): { tone: HealthTone; word: string } {
  switch (status) {
    case 'connected':
      return { tone: 'ok', word: 'Online' }
    case 'connecting':
      return { tone: 'neutral', word: 'Connecting…' }
    case 'offline':
      return { tone: 'error', word: 'Offline' }
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
  // per ACTIVE daemon. The agents list (FederationOverview) is always host-aware.
  const isLocalActive = activeHost === 'local'
  const activeHostLabel = isLocalActive ? 'This computer' : activeHost.label || activeHost.hostname

  const [draft, setDraft] = useState<DraftHost | null>(null)
  const [error, setError] = useState<string | null>(null)
  /** §8.4: "Added <server>. It gives you Voice & Chat only…" after a save. */
  const [notice, setNotice] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // W4: the add/edit form's credentials were accepted but the daemon
  // requires a new password first. The shared PasswordRotationStep replaces
  // the form fields until it completes (then the save finishes) or is
  // cancelled (a NEW host's provisional tile is removed).
  const [rotation, setRotation] = useState<{
    host: ConnectHost
    tempPassword: string
    remember: boolean
    isNew: boolean
  } | null>(null)
  const [serverQuery, setServerQuery] = useState('')
  // Bumped after a successful "Pair as federated peer" so FederationOverview
  // reloads peers without a full Settings remount.
  const [fedRefreshKey, setFedRefreshKey] = useState(0)

  // Top-bar "Add a server…" → open this dialog + focus its first field.
  const addServerFocusSeq = useAddServerFocus()
  const labelRef = useRef<HTMLInputElement | null>(null)

  const beginAdd = (): void => {
    setError(null)
    setDraft(emptyDraft())
  }

  // When the top-bar requests focus, open the add dialog (if not already
  // editing a server) so it mounts, then focus the first input. We respond
  // to every bump of the monotonic request counter.
  useEffect(() => {
    if (addServerFocusSeq === 0) return
    setError(null)
    setDraft((d) => d ?? emptyDraft())
    // Defer to the next frame so the dialog (and its inputs) are mounted.
    const raf = requestAnimationFrame(() => {
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
      setError('Enter a name for this server.')
      return
    }
    const parsed = parseServerUrl(draft.url)
    if (!parsed.ok) {
      setError(parsed.reason)
      return
    }
    const username = draft.username.trim()
    if (!isValidUsername(username)) {
      setError('Username must be at least 2 characters: lowercase letters, numbers, _ or -.')
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

    // MS61: one saved entry per server (host key), for an add AND for an
    // edit that would move an entry onto another entry's address.
    {
      const dup = duplicateSavedHost(hosts, parsed, draft.id ?? null)
      const editKeepsKey =
        draft.id !== null &&
        hosts.some((h) => h.id === draft.id && homeHostKey(h) === homeHostKey(parsed))
      if (dup && !editKeepsKey) {
        setError(
          `${duplicateSavedHostMessage(dup)} Edit or remove that server instead.`,
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
      // Same loginToHost (same edge login URL rule) as the switcher and
      // RemoteSignIn; adds the host first, removes a NEW host on failure.
      const outcome = await verifyHostCredentials(host, passwordEntered, isNew)
      setBusy(false)
      if (outcome.kind === 'error') {
        setError(outcome.reason)
        return
      }
      if (outcome.kind === 'voice-chat-only') {
        // §8.4: saved, never switched to; say where its rooms are.
        setNotice(voiceChatAddedText(host.label || host.hostname))
      }
      if (outcome.kind === 'rotate') {
        // Not signed in yet: hand over to the rotation step (inline). The
        // save completes from its onDone with the NEW password.
        setRotation({
          host: outcome.host,
          tempPassword: passwordEntered,
          remember: draft.remember,
          isNew,
        })
        return
      }
      // Keychain side: remember the password when asked; else forget it.
      // A failed keychain write un-marks the host and is shown (GH #42).
      if (draft.remember) {
        const failure = await saveRememberedPassword(id, passwordEntered)
        if (failure) {
          setError(failure)
          return
        }
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

  // Rotation step completed: the fresh (unrestricted) session is already
  // committed by loginToHost. Finish what save() would have done — keychain
  // side with the NEW password — and close the form.
  const finishRotation = async (newPassword: string): Promise<void> => {
    if (!rotation) return
    // §8.4: the role decides whether the new server is Voice & Chat only.
    const fresh = useConnectHostStore.getState().hosts.find((h) => h.id === rotation.host.id)
    if (fresh && fresh.token.length > 0) {
      const role = await fetchWhoamiRole(fresh, fresh.token)
      useConnectHostStore.getState().setHostAccess(fresh.id, role)
      if (isVoiceChatOnly(role)) setNotice(voiceChatAddedText(fresh.label || fresh.hostname))
    }
    if (rotation.remember) {
      const failure = await saveRememberedPassword(rotation.host.id, newPassword)
      if (failure) {
        setRotation(null)
        setError(failure)
        return
      }
    } else {
      await forgetPassword(rotation.host.id)
      await forgetToken(rotation.host.id)
    }
    setRotation(null)
    setDraft(null)
    setError(null)
  }

  const cancelRotation = (): void => {
    if (!rotation) return
    // A restricted session is useless: for a NEW host drop the provisional
    // tile (so a half-added server never sticks); for an edit, drop the
    // restricted token in memory AND the keychain so the tile reads
    // signed-out and no restricted token survives a relaunch. The draft
    // stays for another try.
    void abandonRotation(rotation.host.id, rotation.isNew)
    setRotation(null)
  }

  const q = serverQuery.trim().toLowerCase()
  // Saved hosts — sorted alphabetically by display label (case-insensitive).
  const rows = (Array.isArray(hosts) ? hosts.slice() : [])
    .filter((h) => {
      if (!q) return true
      const hay = [h.label, h.hostname, h.username, `${h.hostname}:${h.port}`].join(' ').toLowerCase()
      return hay.includes(q)
    })
    .sort((a, b) => (a.label || a.hostname).localeCompare(b.label || b.hostname, undefined, { sensitivity: 'base' }))
  const localHealth = connectionHealth(connectionStatus)
  const closeForm = (): void => {
    setDraft(null)
    setError(null)
  }

  return (
    <div className="w-full max-w-[52rem] space-y-8 pb-6" data-connected-servers="">
      <section>
        <SectionHeader
          title="Servers"
          hint={
            isLocalActive
              ? 'Servers this computer can connect to. A password is saved only if you choose Remember password, and only in this computer’s keychain.'
              : (
                <>
                  Servers paired with <span className="text-[var(--color-text-primary)]">{activeHostLabel}</span>. Your
                  saved servers are in the server menu in the top bar.
                </>
              )
          }
          action={
            isLocalActive ? (
              <Button variant="accent" onClick={beginAdd} data-settings-id="connections.add">
                + Add a server
              </Button>
            ) : undefined
          }
        />

        {notice && (
          <Callout
            tone="info"
            data-testid="connections-voice-chat-notice"
            className="mb-3 type-hint flex items-center justify-between gap-3"
          >
            <span>{notice}</span>
            <Button variant="quiet" onClick={() => setNotice(null)}>
              OK
            </Button>
          </Callout>
        )}

        {isLocalActive ? (
          <>
            {hosts.length > 0 && (
              <Input
                variant="search"
                value={serverQuery}
                onChange={(e) => setServerQuery(e.target.value)}
                placeholder="Search servers"
                aria-label="Search servers"
                className="w-full mb-3"
                data-settings-id="connections.search"
              />
            )}

            <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]">
              {/* Local — always present, never editable. */}
              <ConnectionRow
                mark={<StatusDot tone={localHealth.tone} />}
                title={<span className="truncate">This computer</span>}
                meta={
                  <>
                    <HealthWord tone={localHealth.tone}>{localHealth.word}</HealthWord>
                    <MetaDot />
                    <span>K2 server built into this app</span>
                  </>
                }
                actions={<span className="type-hint text-[var(--color-text-secondary)]">Active</span>}
              />
              <div className="divide-y divide-[var(--color-border)]" data-settings-id="connections.list">
                {rows.length === 0 ? (
                  hosts.length === 0 ? (
                    <StateNote
                      data-testid="connections-empty"
                      title="No saved servers yet."
                      hint="Add a K2 server to reach its workspaces and agents from this computer."
                      action={
                        <Button variant="quiet" onClick={beginAdd}>
                          + Add a server
                        </Button>
                      }
                    />
                  ) : (
                    <StateNote
                      title="No matching servers."
                      action={
                        <Button variant="quiet" onClick={() => setServerQuery('')}>
                          Clear search
                        </Button>
                      }
                    />
                  )
                ) : (
                  rows.map((h) => (
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
                  ))
                )}
              </div>
            </Surface>
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
      </section>

      <FederationOverview refreshKey={fedRefreshKey} />

      {draft && isLocalActive && (
        <ServerFormDialog
          draft={draft}
          setDraft={setDraft}
          rotation={rotation}
          error={error}
          busy={busy}
          labelRef={labelRef}
          onSave={() => void save()}
          onClose={closeForm}
          onRotationDone={(newPassword) => finishRotation(newPassword)}
          onRotationCancel={cancelRotation}
        />
      )}
    </div>
  )
}

/** Add a server / Edit server. A modal, so an edit opened from far down the
 *  list stays with the row you picked instead of pushing it off screen. */
function ServerFormDialog({
  draft,
  setDraft,
  rotation,
  error,
  busy,
  labelRef,
  onSave,
  onClose,
  onRotationDone,
  onRotationCancel,
}: {
  draft: DraftHost
  setDraft: (d: DraftHost) => void
  rotation: { host: ConnectHost; tempPassword: string } | null
  error: string | null
  busy: boolean
  labelRef: React.RefObject<HTMLInputElement | null>
  onSave: () => void
  onClose: () => void
  onRotationDone: (newPassword: string) => void | Promise<void>
  onRotationCancel: () => void
}): React.JSX.Element {
  // Mid-save or mid-rotation Esc does nothing; the buttons there decide.
  useDialogEscape(onClose, !busy && !rotation)
  const field = 'w-full type-control'
  return (
    <>
      <DialogScrim />
      <DialogFrame
        role="dialog"
        aria-modal="true"
        aria-label={draft.id ? 'Edit server' : 'Add server'}
        className="w-[26rem] max-w-[90vw] p-5"
      >
        <h3 className="type-heading text-[var(--color-text-primary)] mb-3">{draft.id ? 'Edit server' : 'Add server'}</h3>
        {rotation ? (
          <>
            <div className="type-hint text-[var(--color-text-secondary)] mb-2">
              {rotation.host.username}@{hostToUrl(rotation.host).replace(/^https?:\/\//, '')}
            </div>
            <PasswordRotationStep
              host={rotation.host}
              initialCurrentPassword={rotation.tempPassword}
              variant="inline"
              onDone={({ newPassword }) => onRotationDone(newPassword)}
              onCancel={onRotationCancel}
            />
          </>
        ) : (
          <form
            className="space-y-2"
            onSubmit={(e) => {
              e.preventDefault()
              if (!busy) onSave()
            }}
          >
            <Input
              ref={labelRef}
              autoFocus
              className={field}
              placeholder="Name (e.g. Mac mini)"
              aria-label="Name"
              value={draft.label}
              onChange={(e) => setDraft({ ...draft, label: e.target.value })}
            />
            <Input
              className={field}
              placeholder="Server URL (e.g. http://192.168.1.50:60710)"
              aria-label="Server URL"
              value={draft.url}
              onChange={(e) => setDraft({ ...draft, url: e.target.value })}
            />
            <Input
              className={field}
              placeholder="Username"
              aria-label="Username"
              autoComplete="username"
              value={draft.username}
              onChange={(e) => setDraft({ ...draft, username: e.target.value })}
            />
            <Input
              className={field}
              type="password"
              autoComplete="off"
              aria-label="Password"
              placeholder={draft.id ? 'Password (leave blank to keep saved)' : 'Password'}
              value={draft.password}
              onChange={(e) => setDraft({ ...draft, password: e.target.value })}
            />
            <label
              className="flex items-center gap-2 pt-1 type-hint text-[var(--color-text-secondary)] cursor-pointer select-none"
              data-settings-id="connections.remember-password"
            >
              <Checkbox
                checked={draft.remember}
                onChange={(e) => setDraft({ ...draft, remember: e.target.checked })}
              />
              Remember password on this computer
            </label>
            {error && (
              <Callout tone="error" role="alert" className="type-hint">
                {error}
              </Callout>
            )}
            <div className="flex justify-end gap-2 pt-2">
              <Button variant="ghost" onClick={onClose}>
                Cancel
              </Button>
              <Button type="submit" variant="accent" disabled={busy}>
                {busy ? 'Verifying…' : 'Save'}
              </Button>
            </div>
          </form>
        )}
      </DialogFrame>
    </>
  )
}

/** When the active host is a REMOTE cloud, list THAT daemon's federation
 *  peers (not this Mac's address book). Pair-from-this-Mac uses local
 *  saved hosts only for the Pair action — no per-tile settings/get probes
 *  (those caused CORS storms against offline rpm tiles while on a customer box). */
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
      // The daemon sends them A to Z (0.46.1); the same keys here keep an
      // older server's pairing order from leaking through.
      setPeers(sortPeersForPicker(res.data))
    }
    setLoading(false)
  }, [])

  useEffect(() => {
    void reload()
  }, [reload, refreshKey, hostLabel])

  // 0.45.2: a peer's signed address notice moved its address.
  useEffect(() => onFederationPeersChanged(primaryScope(), () => void reload()), [reload])

  const pairableLocal = localHosts.filter((h) => h.token.length > 0)
  const activeHealth = connectionHealth(useConnectHostStore((s) => s.connectionStatus))

  const handlePair = async (h: ConnectHost): Promise<void> => {
    setPairBusyHost(h.id)
    setPairMsg(null)
    try {
      // An explicit Pair click pairs again even when already trusted, so a
      // peer paired with the older code is upgraded (re-pair keeps its
      // address and permissions).
      await autoPairWithHost(h.hostname, { repair: true })
      setPairMsg({
        ok: true,
        text: `Paired ${hostLabel} with ${h.label || h.hostname}.`,
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

  return (
    <div className="space-y-8" data-settings-id="connections.active-host-peers">
      <Surface chamfer material="tile">
        <ConnectionRow
          mark={<StatusDot tone={activeHealth.tone} />}
          title={<span className="truncate">{hostLabel}</span>}
          meta={
            <>
              <HealthWord tone={activeHealth.tone}>{activeHealth.word}</HealthWord>
              <MetaDot />
              <span>Active</span>
            </>
          }
          actions={
            <Button variant="quiet" onClick={onConnectLocal}>
              Switch to this computer
            </Button>
          }
        />
      </Surface>

      <section>
        <SectionHeader title={<>Peers of {hostLabel}</>} />
        <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]">
          {loading ? (
            <StateNote title="Loading peers…" />
          ) : !available ? (
            <StateNote
              data-testid="connections-peers-error"
              title="Can’t list this server’s paired servers."
              hint={
                <>
                  Federation may be off, you may not be an owner or admin, or the server may be unreachable. To turn
                  federation on, open {isAirgap() ? 'Servers → Policies' : 'Tunnel → Policies'} while signed in to this
                  server.
                </>
              }
              action={
                <Button variant="quiet" onClick={() => void reload()}>
                  Try again
                </Button>
              }
            />
          ) : peers.length === 0 ? (
            <StateNote title="No paired servers yet." hint="Pair one below." />
          ) : (
            peers.map((p) => {
              // Trust, not reachability: this list can't say whether a peer is up.
              const trustTone: HealthTone = p.trust === 'pending' ? 'warn' : 'neutral'
              return (
                <ConnectionRow
                  key={p.fingerprint}
                  mark={<StatusDot tone={p.repair_recommended ? 'warn' : trustTone} />}
                  title={<span className="truncate">{p.label || p.subdomain || p.fingerprint.slice(0, 12)}</span>}
                  meta={
                    <>
                      <HealthWord tone={trustTone}>{p.trust}</HealthWord>
                      <MetaDot />
                      <span className="truncate">
                        {p.base_url || p.baseUrl
                          ? p.base_url || p.baseUrl
                          : federatedPeerHost(p) || p.fingerprint.slice(0, 24) + '…'}
                      </span>
                      {p.repair_recommended && (
                        <>
                          <MetaDot />
                          <HealthWord tone="warn">
                            <span title={REPAIR_RECOMMENDED_HINT}>Re-pair recommended</span>
                          </HealthWord>
                        </>
                      )}
                    </>
                  }
                >
                  {p.repair_recommended && (
                    <div className="type-hint text-[var(--color-text-secondary)] leading-relaxed">
                      {REPAIR_RECOMMENDED_HINT}
                    </div>
                  )}
                </ConnectionRow>
              )
            })
          )}
        </Surface>
      </section>

      <section>
        <SectionHeader
          title="Pair from this computer"
          hint={
            <>
              Lets {hostLabel} and a server you&apos;re signed in to on this computer trust each other, so their agents
              can be connected.
            </>
          }
        />
        <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]">
          {pairableLocal.length === 0 ? (
            <StateNote title="No servers signed in on this computer. Switch to This computer, sign in to a server, then come back here to pair." />
          ) : (
            pairableLocal.map((h) => (
              <ConnectionRow
                key={h.id}
                title={<span className="truncate">{h.label || h.hostname}</span>}
                meta={<span className="truncate">{h.secure ? h.hostname : savedHostBaseUrl(h)}</span>}
                actions={
                  <Button disabled={pairBusyHost === h.id} onClick={() => void handlePair(h)}>
                    {pairBusyHost === h.id ? 'Pairing…' : 'Pair'}
                  </Button>
                }
              />
            ))
          )}
        </Surface>
        {pairMsg && (
          <Callout tone={pairMsg.ok ? 'ok' : 'error'} className="mt-3 type-hint">
            {pairMsg.text}
          </Callout>
        )}
      </section>
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
  const peerSide = activePeerSideLabel === 'This computer' ? 'this computer' : activePeerSideLabel
  const voiceOnly = hostIsVoiceChatOnly(host)
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

  // Is this server up? Its PUBLIC /boot-status, so a signed-out server still
  // shows its health. Checks again on a slow timer while the page is open,
  // and right after a restart or update we drove finishes.
  const [reach, setReach] = useState<'checking' | 'live' | 'starting' | 'offline'>('checking')
  const [reachKick, setReachKick] = useState(0)
  useEffect(() => {
    if (reconnecting) return
    let stopped = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const tick = async (): Promise<void> => {
      const status = await hostBootStatus(creds)
      if (stopped || !aliveRef.current) return
      setReach(status === null ? 'offline' : status.phase === 'ready' ? 'live' : 'starting')
      timer = setTimeout(() => void tick(), 30_000)
    }
    void tick()
    return () => {
      stopped = true
      if (timer) clearTimeout(timer)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [creds.base, creds.token, reconnecting, reachKick])
  const checkAgain = (): void => {
    setReach('checking')
    setReachKick((n) => n + 1)
  }

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
        text: `Paired with ${label}. Add its agents from a workspace’s Connections tab.`,
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
              `The update didn’t finish: ${label} is on v${observed ?? 'an unexpected version'}, expected v${expected}.` +
                ` Try again, or run install-daemon.sh --version ${expected} on the server.`,
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
          ? `${label} hasn’t come back on v${expected}. Try Check for updates, or run install-daemon.sh on the server.`
          : `${label} still isn’t responding. Try Check for updates, or relaunch K2.`,
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
          ? `Only ${label}’s owner or an admin can restart it.`
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
      // A reply that isn't an update check throws (named), never "vundefined".
      const r = readUpdateCheck(label, await hostOpPost<unknown>(creds, 'daemon/update/check', 15000))
      setSummary(summarizeCheck(label, r))
      setHostCurrent(r.current)
      // Shape A vs B: Connections auto-applies only for standalone (and
      // older hosts that omit installKind). bundled-app never gets apply.
      setInstallKind(r.installKind)
    } catch (e) {
      const m = errMsg(e)
      if (e instanceof UpdateReplyError) {
        setCheckError(`Update check failed: ${m}`)
        return
      }
      clearIfAuthError(m)
      setCheckError(isForbiddenError(m) ? updateForbiddenCopy(label) : `Update check failed: ${m}`)
    } finally {
      setCheckBusy(false)
    }
  }

  /**
   * Drive the started job to an outcome (`driveUpdateJob`): Shape B
   * (standalone / unknown) applies once the job is `staged`; Shape A
   * (bundled-app) never applies. The loop keeps going if this tile unmounts
   * (the person confirmed the install; stopping left z3.k2.dev `staged` with
   * nothing to apply it, 2026-11-05). On screen: the tile's lines; gone: a
   * toast for a failure.
   *
   * Errors BEFORE apply are hard failures (not "reconnecting"), an
   * unreadable status reply included. After apply (or a Shape A stage) a
   * dropped host hands off to the version-gated waitForHostReady.
   */
  const pollStatus = async (
    jobId: string,
    kind: UpdateCheckResult['installKind'] | undefined,
  ): Promise<'applied' | 'failed' | 'unreachable'> => {
    const fail = (text: string): 'failed' => {
      if (aliveRef.current) {
        setPhaseText(null)
        setUpdateError(text)
      } else {
        useToastStore.getState().addToast(text, 'error', 10000)
      }
      return 'failed'
    }
    const refused = (err: unknown): 'failed' => {
      const m = errMsg(err)
      // An unreadable reply quotes the body, which may say "Forbidden":
      // it is never a permission refusal.
      if (err instanceof UpdateReplyError) return fail(`Couldn't install the update on ${label}: ${m}`)
      clearIfAuthError(m)
      return fail(
        isForbiddenError(m) ? updateForbiddenCopy(label) : `Couldn't install the update on ${label}: ${m}`,
      )
    }
    const outcome = await driveUpdateJob({
      hostLabel: label,
      installKind: kind,
      status: () => hostOpGet<unknown>(creds, 'daemon/update/status', { job_id: jobId }),
      apply: () => hostOpPost(creds, 'daemon/update/apply', 30000, { job_id: jobId }),
      sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
      onStatus: (status) => {
        if (!aliveRef.current) return
        const pct = typeof status.progress === 'number' ? status.progress * 100 : undefined
        setPhaseText(updatePhaseCopy(status.phase, label, { progress: pct, current: hostCurrent }))
      },
      onApplying: () => {
        if (aliveRef.current) setPhaseText(`Installing update on ${label}…`)
      },
    })
    switch (outcome.kind) {
      case 'applied':
      case 'unreachable':
        return outcome.kind
      case 'job-failed':
        return fail(
          outcome.status.error
            ? `Update error on ${label}: ${outcome.status.error}`
            : updatePhaseCopy(outcome.status.phase, label, { current: hostCurrent }),
        )
      case 'poll-failed':
      case 'apply-failed':
        return refused(outcome.err)
      case 'timed-out':
        return fail(`Timed out waiting for the update on ${label}. Use Check for updates to see whether it finished.`)
    }
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
      const { job_id: jobId } = readUpdateStart(
        label,
        await hostOpPost<unknown>(creds, 'daemon/update/start', 30000),
      )
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

  const hasFailure =
    Boolean(checkError || updateError) || (restartMsg ? !restartMsg.ok : false) || (pairMsg ? !pairMsg.ok : false)
  const signedIn = !signedOut && !voiceOnly
  const updateLatest = summary?.kind === 'available' ? summary.latest : null

  // Health, the first thing the row says after the name. Red and amber only
  // for a server that's unreachable or an action that just failed.
  const health: { tone: HealthTone; word: string } = isActive
    ? connectionHealth(connectionStatus)
    : restartBusy
      ? { tone: 'neutral', word: 'Restarting…' }
      : reconnecting
        ? { tone: 'neutral', word: 'Reconnecting…' }
        : reach === 'offline'
          ? { tone: 'error', word: 'Offline' }
          : hasFailure
            ? { tone: 'warn', word: 'Needs attention' }
            : reach === 'checking'
              ? { tone: 'neutral', word: 'Checking…' }
              : reach === 'starting'
                ? { tone: 'neutral', word: 'Starting…' }
                : signedOut
                  ? { tone: 'neutral', word: 'Signed out' }
                  : { tone: 'ok', word: 'Online' }

  // Every action this row offers, in menu order.
  const pairItem: OverflowMenuItem | null =
    signedIn && !isActive
      ? peerPaired === 'yes'
        ? {
            id: 'pair',
            label: pairBusy ? 'Pairing…' : 'Check pairing',
            disabled: pairBusy || reconnecting,
            title: `${activePeerSideLabel} is already paired with this server. Check the pairing and repair it if needed.`,
            onSelect: () => void doPairPeer(),
          }
        : {
            id: 'pair',
            label: pairBusy ? 'Pairing…' : peerPaired === 'checking' ? 'Checking…' : 'Pair',
            disabled: pairBusy || reconnecting || federation === 'off',
            hint: federation === 'off' ? `Turn on federation on this server and on ${peerSide} first` : undefined,
            title:
              federation === 'off'
                ? `Turn on federation on this server and on ${peerSide} first`
                : `Let ${peerSide} and this server trust each other, so their agents can be connected`,
            onSelect: () => void doPairPeer(),
          }
      : null
  const actions: OverflowMenuItem[] = []
  if (pairItem) actions.push(pairItem)
  if (signedIn && updateLatest) {
    actions.push({
      id: 'update',
      label: updateBusy ? 'Updating…' : `Update to ${updateLatest}`,
      disabled: updateBusy || reconnecting,
      onSelect: () => void doUpdate(),
    })
  }
  if (signedOut) actions.push({ id: 'sign-in', label: 'Sign in', onSelect: () => signInForManagement(host) })
  if (signedIn && hasFailure) {
    actions.push({ id: 'sign-in-again', label: 'Sign in again', onSelect: () => signInForManagement(host) })
  }
  if (signedIn) {
    actions.push({
      id: 'restart',
      label: restartBusy ? 'Restarting…' : reconnecting ? 'Reconnecting…' : 'Restart',
      disabled: restartBusy || reconnecting,
      onSelect: () => void doRestart(),
    })
    actions.push({
      id: 'check-updates',
      label: checkBusy ? 'Checking…' : 'Check for updates',
      disabled: checkBusy,
      onSelect: () => void doCheck(),
    })
  }
  if (!signedOut && (voiceOnly || !isActive)) {
    actions.push({
      id: 'sign-out',
      label: 'Sign out',
      title: `Sign out of ${label} on this computer. The saved password is kept.`,
      onSelect: () => void signOutOfHost(host),
    })
  }
  actions.push({ id: 'edit', label: 'Edit', group: true, onSelect: onEdit })
  actions.push({
    id: 'remove',
    label: 'Remove',
    danger: true,
    onSelect: () => {
      void confirm({
        title: `Remove ${label}?`,
        message: `K2 forgets ${label} on this computer, including its saved password and sign-in. Nothing changes on the server, and you can add it again later.`,
        confirmLabel: 'Remove',
        destructive: true,
      }).then((ok) => {
        if (ok) onRemove()
      })
    },
  })

  // One button at most, the thing this server needs next. Whatever isn't
  // promoted here stays in the menu. Only a server that can't be reached
  // gets a new action: asking again.
  const primaryId: string | null =
    reach === 'offline' && !reconnecting && !isActive
      ? 'try-again'
      : signedOut
        ? 'sign-in'
        : !signedIn
          ? null
          : hasFailure
            ? 'sign-in-again'
            : updateLatest
              ? 'update'
              : pairItem && peerPaired === 'no' && federation !== 'off'
                ? 'pair'
                : null
  const primary: OverflowMenuItem | undefined =
    primaryId === 'try-again'
      ? { id: 'try-again', label: 'Try again', onSelect: checkAgain }
      : actions.find((a) => a.id === primaryId)
  const pairInMenu = Boolean(pairItem) && primaryId !== 'pair'

  const hostText = host.secure && host.port === 443 ? host.hostname : `${host.hostname}:${host.port}`
  const busyNote = checkBusy ? 'Checking for updates…' : pairBusy && primaryId !== 'pair' ? 'Pairing…' : null

  return (
    <ConnectionRow
      data-server-row={label}
      mark={<StatusDot tone={health.tone} />}
      title={
        <>
          <span className="truncate">{host.label}</span>
          {voiceOnly && <VoiceChatOnlyChip />}
        </>
      }
      meta={
        <>
          <HealthWord tone={health.tone}>{health.word}</HealthWord>
          <MetaDot />
          <span className="inline-flex items-center gap-1 min-w-0">
            {host.secure && <IconLock className="w-2.5 h-2.5 flex-shrink-0" />}
            <span className="truncate">{hostText}</span>
          </span>
          {host.remember && (
            <>
              <MetaDot />
              <span>saved</span>
            </>
          )}
          {!signedOut && !isActive && peerPaired === 'yes' && (
            <>
              <MetaDot />
              <span title={`${activePeerSideLabel} trusts this server`}>Paired</span>
            </>
          )}
          {(federation === 'on' || federation === 'off') && (
            <>
              <MetaDot />
              <span title="Whether this server can connect to agents on other servers">
                {federationBadgeText(federation)}
              </span>
            </>
          )}
          {signedOut && reach !== 'offline' && (
            <>
              <MetaDot />
              <span>Sign in to manage this server</span>
            </>
          )}
          {/* "Active" only — switching to a host happens from the server
              switcher, not here. The row is for managing the host in place. */}
          {isActive && (
            <>
              <MetaDot />
              <span>Active</span>
            </>
          )}
        </>
      }
      actions={
        <>
          {primary && (
            <Button
              disabled={primary.disabled}
              title={primary.title}
              onClick={primary.onSelect}
              data-settings-id={primary.id === 'pair' ? 'connections.pair-federated-peer' : undefined}
              data-primary-action={primary.id}
            >
              {primary.label}
            </Button>
          )}
          <OverflowMenu
            label={`More actions for ${label}`}
            items={actions.filter((a) => a.id !== primaryId)}
            triggerProps={pairInMenu ? { 'data-settings-id': 'connections.pair-federated-peer' } : undefined}
          />
        </>
      }
    >
      {/* What's happening on this row, or what just went wrong. Fail loud,
          never silent. */}
      {reach === 'offline' && !isActive && !reconnecting && (
        <div className="type-hint text-[var(--color-text-secondary)]">
          Can’t reach this server. Check that it’s running and on your network, then try again.
        </div>
      )}
      {!signedOut && voiceOnly && (
        // prd-voice-chat-only-members-v1 §8.2: the server never opens, and
        // this login can't restart, update or pair it. Its rooms are in the
        // headset; Sign out and Remove are all there is to do here.
        <div className="type-hint text-[var(--color-text-secondary)]" data-testid="connections-voice-chat-only">
          {VOICE_CHAT_LINE}
        </div>
      )}
      {/* The ACTIVE connection's recovery state — always visible while it's
          not plain 'connected'. Same copy as the in-app banner
          (recoveryStatusText — single source). */}
      {isActive && activeRecovery.kind !== 'connected' && (
        <div
          className={`type-hint ${
            activeRecovery.kind === 'signin-required'
              ? 'text-[var(--color-status-error-soft)]'
              : 'text-[var(--color-text-secondary)]'
          }`}
        >
          {recoveryStatusText(label, activeRecovery)}
        </div>
      )}
      {busyNote && <div className="type-hint text-[var(--color-text-secondary)]">{busyNote}</div>}
      {phaseText && <div className="type-hint text-[var(--color-text-secondary)]">{phaseText}</div>}
      {summary && !checkError && <div className="type-hint text-[var(--color-text-secondary)]">{summary.text}</div>}
      {restartMsg?.ok && <div className="type-hint text-[var(--color-text-secondary)]">{restartMsg.text}</div>}
      {pairMsg?.ok && <div className="type-hint text-[var(--color-text-secondary)]">{pairMsg.text}</div>}
      {restartMsg && !restartMsg.ok && <Callout role="alert" className="type-hint">{restartMsg.text}</Callout>}
      {pairMsg && !pairMsg.ok && <Callout role="alert" className="type-hint">{pairMsg.text}</Callout>}
      {checkError && <Callout role="alert" className="type-hint">{checkError}</Callout>}
      {updateError && <Callout role="alert" className="type-hint">{updateError}</Callout>}
    </ConnectionRow>
  )
}
