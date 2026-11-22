// Settings → Connections — the K2 Connect CLIENT address book (PRD §1,
// build order step #3). Where users set up / edit / remove the K2 servers
// they connect OUT to. (The OTHER direction — exposing THIS device's own
// daemon — lives in Settings → K2 Connect.)
//
// Two columns on a wide pane (Servers left, Agents on other servers right,
// each scrolling on its own); one column on a narrow one, Servers first.
// Each saved ConnectHost is a row: a status dot, its name and address, then
// its state in words (health from its public /boot-status, password,
// pairing); at most one button for what it needs next, and everything else
// in the row's menu. Add a server is one dialog in steps: the address
// (checked against its /boot-status), then sign in, then a done step. Edit
// is the same dialog with every field. Non-secret fields persist to ~/.k2so/connect-hosts.json (via the
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

import React, { useCallback, useEffect, useId, useRef, useState } from 'react'
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
import { abandonRotation, fetchWhoamiRole, verifyHostCredentials, type LoginFailureKind } from '@/lib/add-server-login'
import { PasswordRotationStep } from '@/components/PasswordRotationStep'
import { parseServerUrl, isValidUsername } from '@/lib/connect-validate'
import { IconLock } from '@/components/icons/IconLock'
import { FederationOverview } from './FederationOverview'
import { useAddServerFocus } from '@/stores/add-server-focus'
import { useElementWidth } from '@/hooks/useElementWidth'
import type { SettingEntry } from '../searchManifest'
import {
  remoteCreds,
  hostOpPost,
  hostOpGet,
  hostBootStatus,
  summarizeCheck,
  type CheckSummary,
  type FederationState,
} from '@/lib/host-ops'
import {
  autoPairWithHost,
  federatedPeerHost,
  isTrustedPeerHost,
  listFederationPeers,
  peerMatchesHostname,
  repairStepsForPeer,
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
  Input,
  OverflowMenu,
  SearchField,
  type OverflowMenuItem,
} from '@/components/ui'
import { SETTING_LIST_ROW_BAND, SETTING_ROW_LABEL, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { AccessSheet, DIALOG_FIELD, DIALOG_MUTED, Field } from './AccessDialogs'
import {
  ConnectionRow,
  Facts,
  ProblemNote,
  ServerAddress,
  StateNote,
  StatusDot,
  StatusFact,
  type HealthTone,
} from './ConnectionsParts'
import { CLI_NAME, PRODUCT_NAME } from '@/lib/brand'

export const CONNECTIONS_MANIFEST: SettingEntry[] = [
  { id: 'connections.add', section: 'connections', label: 'Add a server', description: `Save another ${PRODUCT_NAME} server to connect to`, keywords: ['server', 'remote', 'connect', 'host', 'add', `${CLI_NAME} connect`, 'address book'] },
  { id: 'connections.remember-password', section: 'connections', label: 'Remember password', description: 'Keep a server’s password in this computer’s keychain', keywords: ['token', 'password', 'keychain', 'remember', 'credentials'] },
  { id: 'connections.list', section: 'connections', label: 'Saved servers', description: `Edit or remove saved ${PRODUCT_NAME} servers`, keywords: ['servers', 'hosts', 'edit', 'remove', 'list', 'search', 'filter'] },
  {
    id: 'connections.pair-federated-peer',
    section: 'connections',
    label: 'Pair a server',
    description:
      'Let this computer, or a server you’re signed in to, trust another saved server so their agents can connect',
    keywords: ['federation', 'peer', 'pair', 'federated', 'federated peer', 'cross-server', 'trust', 'agents', 'cloud'],
  },
]

/** Settings pane width (px) from which Servers and Agents on other servers
 *  sit side by side, each column scrolling on its own. Narrower, they stack,
 *  Servers first, and the page scrolls. The same breakpoint as Workspaces /
 *  Agents; the servers column gets 3/5 of the width, so a row's name,
 *  address and chips still fit beside its buttons. Every theme alike. */
export const CONNECTED_SERVERS_TWO_COLUMN_MIN_PX = 840

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

/** Where the Add a server dialog is: the address, then signing in, then
 *  the confirmation. Edit is one step with every field. */
type DraftStep = 'address' | 'sign-in' | 'done' | 'edit'

type DraftHost = {
  id: string | null // null = creating a new host
  step: DraftStep
  label: string
  /** Raw address field (e.g. https://your-server.example); parsed on save via
   *  parseServerUrl into hostname/secure/port. */
  url: string
  username: string
  password: string
  remember: boolean
  /** Done step: the server was saved with Voice & Chat only. */
  voiceOnly?: boolean
}

/** What went wrong in the dialog: a field's own problem shows under it;
 *  anything else is a note above the buttons with the next step. */
type FormProblem = {
  what: string
  next?: string
  detail?: string | null
  field?: 'address' | 'name' | 'username' | 'password'
  /** The address didn't answer: offer to go on anyway. */
  anyway?: boolean
  /** The address is already saved as this server: offer to use it. */
  existing?: { id: string; label: string }
}

function emptyDraft(): DraftHost {
  return { id: null, step: 'address', label: '', url: '', username: '', password: '', remember: false }
}

/** Reconstruct the URL field from a saved host for the edit form. */
function hostToUrl(h: ConnectHost): string {
  const scheme = h.secure ? 'https' : 'http'
  const authority = h.secure && h.port === 443 ? h.hostname : `${h.hostname}:${h.port}`
  return `${scheme}://${authority}`
}

/** A server's address as people read it: no https://, no :443. */
function shownAddress(h: { hostname: string; port: number; secure: boolean }): string {
  return h.secure && h.port === 443 ? h.hostname : `${h.hostname}:${h.port}`
}

/** The address parser's reason, in plain words with an example. */
function addressProblem(reason: string): string {
  if (/^Enter /.test(reason)) return 'Enter the server’s address.'
  if (/explicit port/i.test(reason)) return 'Add the server’s port, like http://192.168.1.50:60710.'
  if (/not HTTPS/i.test(reason)) return 'Servers on your network use http://, not https://, like http://192.168.1.50:60710.'
  if (/^Port /.test(reason)) return 'The port must be a number from 1 to 65535.'
  return 'That doesn’t look like a server address. Try one like your-server.example.'
}

/** Why a sign-in failed, in plain words, with what to do next. */
function signInProblem(kind: LoginFailureKind | undefined, reason: string, server: string): FormProblem {
  switch (kind) {
    case 'auth':
      return { what: 'That username or password isn’t right.', next: 'Check both and try again. Usernames are lowercase.', field: 'password' }
    case 'unreachable':
      return { what: `Couldn’t reach ${server} to sign in.`, next: 'Check that the server is running and this computer is online, then try again.' }
    case 'throttled':
      return { what: 'Too many sign-in tries.', next: reason }
    case 'not-found':
    case 'server':
      return { what: `${server} didn’t accept the sign-in.`, next: `Make sure this is a ${PRODUCT_NAME} server, and that it and ${PRODUCT_NAME} are up to date.`, detail: reason }
    default:
      return { what: `Couldn’t sign in to ${server}.`, detail: reason }
  }
}

const USERNAME_RULE = 'Usernames are at least 2 characters: lowercase letters, numbers, _ or -.'

/** A note under a list card: what just happened, with a way to clear it. */
type PageNotice = { tone: 'ok' | 'info'; text: string }

export function ConnectionsSection(): React.JSX.Element {
  // Measure the full-width root (never the capped stacked column), so the
  // layout follows the pane as it resizes.
  const [rootEl, rootRef] = useState<HTMLDivElement | null>(null)
  const twoColumn = useElementWidth(rootEl) >= CONNECTED_SERVERS_TWO_COLUMN_MIN_PX
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
  const [problem, setProblem] = useState<FormProblem | null>(null)
  /** What the last add, edit or remove did, under the list. */
  const [notice, setNotice] = useState<PageNotice | null>(null)
  /** What the dialog is waiting on, in its button's words. */
  const [busy, setBusy] = useState<string | null>(null)
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
  /** A saved server picked from the Add dialog: its row is banded and
   *  brought into view for a moment. */
  const [highlightId, setHighlightId] = useState<string | null>(null)
  useEffect(() => {
    if (!highlightId) return
    const row = Array.from(document.querySelectorAll<HTMLElement>('[data-host-id]')).find((el) => el.dataset.hostId === highlightId)
    row?.scrollIntoView?.({ block: 'nearest' })
    const t = setTimeout(() => setHighlightId(null), 4000)
    return () => clearTimeout(t)
  }, [highlightId])
  // Bumped after a successful "Pair as federated peer" so FederationOverview
  // reloads peers without a full Settings remount.
  const [fedRefreshKey, setFedRefreshKey] = useState(0)

  // Top-bar "Add a server…" → open this dialog + focus its first field.
  const addServerFocusSeq = useAddServerFocus()
  const labelRef = useRef<HTMLInputElement | null>(null)

  const beginAdd = (): void => {
    setProblem(null)
    setNotice(null)
    setDraft(emptyDraft())
  }

  // When the top-bar requests focus, open the add dialog (if not already
  // editing a server) so it mounts, then focus the first input. We respond
  // to every bump of the monotonic request counter.
  useEffect(() => {
    if (addServerFocusSeq === 0) return
    setProblem(null)
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
    setProblem(null)
    setNotice(null)
    setDraft({
      id: h.id,
      step: 'edit',
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

  /** Remove a saved server (its row already asked) and say so. */
  const forgetHost = (h: ConnectHost): void => {
    removeHost(h.id)
    setNotice({ tone: 'ok', text: `Removed ${h.label || h.hostname} from this computer. You can add it again at any time.` })
  }

  /** The address check shared by Continue and Save: parsed, and not
   *  already saved under another entry (MS61). */
  const checkAddress = (d: DraftHost): { hostname: string; secure: boolean; port: number } | null => {
    const parsed = parseServerUrl(d.url)
    if (!parsed.ok) {
      setProblem({ what: addressProblem(parsed.reason), field: 'address' })
      return null
    }
    const dup = duplicateSavedHost(hosts, parsed, d.id ?? null)
    const editKeepsKey =
      d.id !== null && hosts.some((h) => h.id === d.id && homeHostKey(h) === homeHostKey(parsed))
    if (dup && !editKeepsKey) {
      const label = dup.label || dup.hostname
      setProblem(
        d.id === null
          ? {
              what: `You’ve already saved this server as ${label}.`,
              next: `Use ${label}, or change the address to add a different server.`,
              field: 'address',
              existing: { id: dup.id, label },
            }
          : {
              what: `You’ve already saved this server as ${label}.`,
              next: 'Change the address, or edit that one from its row’s menu.',
              field: 'address',
            },
      )
      return null
    }
    return parsed
  }

  /** Add, step 1 → 2: the address is well formed, new, and answers (its
   *  public /boot-status). `anyway` skips the answer check. */
  const continueToSignIn = async (anyway = false): Promise<void> => {
    if (!draft || busy) return
    setProblem(null)
    const parsed = checkAddress(draft)
    if (!parsed) return
    if (!anyway) {
      setBusy('Checking…')
      const status = await hostBootStatus(remoteCreds({ ...parsed, token: '' }))
      setBusy(null)
      if (status === null) {
        setProblem({
          what: `Couldn’t reach ${shownAddress(parsed)}.`,
          next: 'Check the address, and that the server is running and reachable from this computer.',
          field: 'address',
          anyway: true,
        })
        return
      }
    }
    setDraft((d) => (d ? { ...d, step: 'sign-in' } : d))
  }

  const save = async (): Promise<void> => {
    if (!draft || busy) return
    setProblem(null)
    const parsed = checkAddress(draft)
    if (!parsed) return
    const username = draft.username.trim()
    if (!isValidUsername(username)) {
      setProblem({ what: username ? USERNAME_RULE : 'Enter your username on this server.', field: 'username' })
      return
    }
    const passwordEntered = draft.password // do NOT trim — passwords may have edge whitespace
    const isNew = !draft.id

    // New hosts must prove credentials before they enter the address book —
    // otherwise a wrong password still creates a tile, and a second "add"
    // attempt leaves two lookalike servers.
    if (isNew && !passwordEntered) {
      setProblem({ what: 'Enter your password on this server.', field: 'password' })
      return
    }

    const id = draft.id ?? `host-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
    const existing = draft.id ? hosts.find((h) => h.id === draft.id) : undefined
    // No name: the address stands in for it, as it does everywhere.
    const label = draft.label.trim() || parsed.hostname

    const host: ConnectHost = {
      id,
      label,
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
    let voiceOnly = false
    if (passwordEntered) {
      setBusy('Signing in…')
      // Same loginToHost (same edge login URL rule) as the switcher and
      // RemoteSignIn; adds the host first, removes a NEW host on failure.
      const outcome = await verifyHostCredentials(host, passwordEntered, isNew)
      setBusy(null)
      if (outcome.kind === 'error') {
        setProblem(
          outcome.loginKind
            ? signInProblem(outcome.loginKind, outcome.reason, label)
            : { what: outcome.reason, field: 'address' },
        )
        return
      }
      // §8.4: saved, never switched to; the done step says where its rooms are.
      if (outcome.kind === 'voice-chat-only') voiceOnly = true
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
          setProblem({
            what: `You’re signed in, but ${PRODUCT_NAME} couldn’t save the password in the keychain.`,
            next: 'Turn off Remember password to finish without saving it, or try again.',
            detail: failure,
          })
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
    finishSave(label, isNew, voiceOnly)
  }

  /** The add lands on its done step; an edit closes and says it saved. */
  const finishSave = (label: string, isNew: boolean, voiceOnly: boolean): void => {
    setProblem(null)
    if (isNew) {
      setDraft((d) => (d ? { ...d, label, step: 'done', voiceOnly } : d))
      return
    }
    setDraft(null)
    setNotice(voiceOnly ? { tone: 'info', text: voiceChatAddedText(label) } : { tone: 'ok', text: `Saved your changes to ${label}.` })
  }

  // Rotation step completed: the fresh (unrestricted) session is already
  // committed by loginToHost. Finish what save() would have done — keychain
  // side with the NEW password — and close the form.
  const finishRotation = async (newPassword: string): Promise<void> => {
    if (!rotation) return
    // §8.4: the role decides whether the new server is Voice & Chat only.
    let voiceOnly = false
    const fresh = useConnectHostStore.getState().hosts.find((h) => h.id === rotation.host.id)
    if (fresh && fresh.token.length > 0) {
      const role = await fetchWhoamiRole(fresh, fresh.token)
      useConnectHostStore.getState().setHostAccess(fresh.id, role)
      voiceOnly = isVoiceChatOnly(role)
    }
    if (rotation.remember) {
      const failure = await saveRememberedPassword(rotation.host.id, newPassword)
      if (failure) {
        setRotation(null)
        setProblem({
          what: `You’re signed in, but ${PRODUCT_NAME} couldn’t save the new password in the keychain.`,
          next: 'Turn off Remember password to finish without saving it, or try again.',
          detail: failure,
        })
        return
      }
    } else {
      await forgetPassword(rotation.host.id)
      await forgetToken(rotation.host.id)
    }
    const label = rotation.host.label || rotation.host.hostname
    const isNew = rotation.isNew
    setRotation(null)
    finishSave(label, isNew, voiceOnly)
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
    setProblem(null)
  }

  const noticeNote = notice && (
    <Callout
      tone={notice.tone}
      role="status"
      data-testid="connections-notice"
      className="mt-3 type-hint flex items-center justify-between gap-3"
    >
      <span>{notice.text}</span>
      <Button variant="quiet" onClick={() => setNotice(null)}>
        OK
      </Button>
    </Callout>
  )

  const serversColumn = isLocalActive ? (
    <div>
      <SettingsGroup
        card
        title="Servers"
        hint={`Other ${PRODUCT_NAME} servers you use from this computer. Switch between them from the server menu in the top bar.`}
        action={
          <Button variant="accent" onClick={beginAdd} data-settings-id="connections.add">
            Add a server
          </Button>
        }
      >
        {hosts.length >= SERVER_SEARCH_FROM && (
          <div className="px-4 py-3">
            <SearchField
              value={serverQuery}
              onChange={(e) => setServerQuery(e.target.value)}
              placeholder="Search servers"
              aria-label="Search servers"
              className="w-full"
              data-settings-id="connections.search"
            />
          </div>
        )}
        {/* Local — always present, never editable. */}
        <ConnectionRow
          data-server-row="This computer"
          mark={<StatusDot tone={localHealth.tone} />}
          title={
            <>
              <span className="truncate flex-shrink-0">This computer</span>
              <span className="type-hint text-[var(--color-text-secondary)] truncate ml-1">
                The {PRODUCT_NAME} server built into this app
              </span>
            </>
          }
          meta={
            <Facts>
              <StatusFact kind="health" tone={localHealth.tone}>
                {localHealth.word}
              </StatusFact>
              <StatusFact kind="active" tone="quiet">
                In use
              </StatusFact>
            </Facts>
          }
        />
        <div className="divide-y divide-[var(--color-border)]" data-settings-id="connections.list">
          {rows.length === 0 ? (
            hosts.length === 0 ? (
              <StateNote
                data-testid="connections-empty"
                title="No other servers yet"
                hint="Add one to use its workspaces and agents from this computer."
              />
            ) : (
              <StateNote
                title={`No servers match “${serverQuery.trim()}”`}
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
                onRemove={() => forgetHost(h)}
                highlight={highlightId === h.id}
                onFederationPeersChanged={() => setFedRefreshKey((n) => n + 1)}
              />
            ))
          )}
        </div>
      </SettingsGroup>
      {noticeNote}
    </div>
  ) : (
    <ActiveHostPeersPanel
      hostLabel={activeHostLabel}
      refreshKey={fedRefreshKey}
      localHosts={Array.isArray(hosts) ? hosts : []}
      onPeersChanged={() => setFedRefreshKey((n) => n + 1)}
      onConnectLocal={() => pickHost('local')}
    />
  )
  const agentsColumn = <FederationOverview refreshKey={fedRefreshKey} />

  return (
    <div
      ref={rootRef}
      data-connected-servers=""
      data-servers-layout={twoColumn ? 'two-column' : 'stacked'}
      className={twoColumn ? 'w-full h-full min-h-0 flex' : 'w-full'}
    >
      {/* The same two wrappers in both layouts, so crossing the breakpoint
          restyles them instead of remounting the rows (and losing a row's
          update or pairing in progress). */}
      <div
        data-servers-column="servers"
        className={
          twoColumn
            ? 'flex-[3] min-w-0 overflow-y-auto pr-6 pb-6 [scrollbar-gutter:stable]'
            : 'max-w-[52rem] pb-8'
        }
      >
        {serversColumn}
      </div>
      <div
        data-servers-column="agents"
        className={
          twoColumn
            ? 'flex-[2] min-w-0 overflow-y-auto border-l border-[var(--color-border)] pl-6 pb-6 [scrollbar-gutter:stable]'
            : 'max-w-[52rem] pb-6'
        }
      >
        {agentsColumn}
      </div>

      {draft && isLocalActive && (
        <ServerFormDialog
          draft={draft}
          setDraft={(d) => {
            setDraft(d)
            // Typing in the field that had a problem clears it.
            if (problem?.field) setProblem(null)
          }}
          rotation={rotation}
          problem={problem}
          busy={busy}
          labelRef={labelRef}
          onContinue={(anyway) => void continueToSignIn(anyway)}
          onUseExisting={(existing) => {
            closeForm()
            setServerQuery('')
            setHighlightId(existing.id)
            setNotice({ tone: 'info', text: `${existing.label} was already saved, so nothing new was added. It’s highlighted in the list.` })
          }}
          onChangeAddress={() => setProblem(null)}
          onBack={() => {
            setProblem(null)
            setDraft({ ...draft, step: 'address', password: '' })
          }}
          onSave={() => void save()}
          onClose={closeForm}
          onRotationDone={(newPassword) => finishRotation(newPassword)}
          onRotationCancel={cancelRotation}
        />
      )}
    </div>
  )
}

/** From this many saved servers the list gets a search field. */
const SERVER_SEARCH_FROM = 6

/** Add a server (address, then sign in, then done) and Edit server. The
 *  shared Settings dialog: centred, labelled fields, buttons pinned at the
 *  foot. */
function ServerFormDialog({
  draft,
  setDraft,
  rotation,
  problem,
  busy,
  labelRef,
  onContinue,
  onUseExisting,
  onChangeAddress,
  onBack,
  onSave,
  onClose,
  onRotationDone,
  onRotationCancel,
}: {
  draft: DraftHost
  setDraft: (d: DraftHost) => void
  rotation: { host: ConnectHost; tempPassword: string } | null
  problem: FormProblem | null
  busy: string | null
  labelRef: React.RefObject<HTMLInputElement | null>
  onContinue: (anyway: boolean) => void
  onUseExisting: (existing: { id: string; label: string }) => void
  onChangeAddress: () => void
  onBack: () => void
  onSave: () => void
  onClose: () => void
  onRotationDone: (newPassword: string) => void | Promise<void>
  onRotationCancel: () => void
}): React.JSX.Element {
  const id = useId()
  const formId = `${id}-form`
  const step = draft.step
  const parsed = parseServerUrl(draft.url)
  const address = parsed.ok ? shownAddress(parsed) : draft.url.trim()
  const name = draft.label.trim() || (parsed.ok ? parsed.hostname : draft.url.trim())
  // A field's own problem (no next step to offer) shows under the field;
  // anything else is a note above the buttons.
  const inline = problem !== null && problem.field !== undefined && !problem.next && !problem.anyway
  const fieldProblem = (f: FormProblem['field']): string | null => (inline && problem?.field === f ? problem.what : null)
  const note =
    problem && !inline ? (
      <ProblemNote
        what={problem.what}
        next={problem.next}
        detail={problem.detail}
        testId="connections-form-problem"
        action={
          problem.anyway ? (
            <Button disabled={busy !== null} onClick={() => onContinue(true)}>
              Continue anyway
            </Button>
          ) : undefined
        }
      />
    ) : null

  const title = step === 'edit' ? 'Edit server' : step === 'done' ? 'Server added' : 'Add a server'
  const intro =
    rotation !== null
      ? undefined
      : step === 'address'
        ? 'Step 1 of 2. Where the server is. You’ll sign in to it next.'
        : step === 'sign-in'
          ? `Step 2 of 2. Sign in with your account on ${name}.`
          : step === 'edit'
            ? `Change how ${PRODUCT_NAME} finds and signs in to this server.`
            : undefined

  const rememberRow = (
    <label className="flex items-start gap-2 cursor-pointer select-none" data-settings-id="connections.remember-password">
      <Checkbox
        className="mt-0.5"
        checked={draft.remember}
        onChange={(e) => setDraft({ ...draft, remember: e.target.checked })}
      />
      <span>
        <span className={SETTING_ROW_LABEL}>Remember password</span>
        <span className={`block ${DIALOG_MUTED}`}>
          Saves it in this computer’s keychain, its built-in password store, so {PRODUCT_NAME} signs in by itself next time.
        </span>
      </span>
    </label>
  )

  const addressField = (
    <Field
      label="Address"
      htmlFor={`${id}-url`}
      problem={fieldProblem('address')}
      hint="Its web address, like your-server.example, or http://192.168.1.50:60710 on your network."
    >
      <Input
        id={`${id}-url`}
        ref={step === 'address' ? labelRef : undefined}
        autoFocus={step === 'address'}
        className={DIALOG_FIELD}
        aria-invalid={fieldProblem('address') ? true : undefined}
        value={draft.url}
        onChange={(e) => setDraft({ ...draft, url: e.target.value })}
      />
    </Field>
  )
  const nameField = (
    <Field
      label={step === 'edit' ? 'Name' : 'Name (optional)'}
      htmlFor={`${id}-name`}
      problem={fieldProblem('name')}
      hint={`How it shows in ${PRODUCT_NAME}. Leave it blank to use the address.`}
    >
      <Input
        id={`${id}-name`}
        ref={step === 'edit' ? labelRef : undefined}
        autoFocus={step === 'edit'}
        className={DIALOG_FIELD}
        value={draft.label}
        onChange={(e) => setDraft({ ...draft, label: e.target.value })}
      />
    </Field>
  )
  const usernameField = (
    <Field label="Username" htmlFor={`${id}-user`} problem={fieldProblem('username')}>
      <Input
        id={`${id}-user`}
        autoFocus={step === 'sign-in'}
        autoComplete="username"
        className={DIALOG_FIELD}
        aria-invalid={fieldProblem('username') ? true : undefined}
        value={draft.username}
        onChange={(e) => setDraft({ ...draft, username: e.target.value })}
      />
    </Field>
  )
  const passwordField = (
    <Field
      label="Password"
      htmlFor={`${id}-pass`}
      problem={fieldProblem('password')}
      hint={step === 'edit' ? 'Leave it blank to keep the saved password.' : undefined}
    >
      <Input
        id={`${id}-pass`}
        type="password"
        autoComplete="off"
        className={DIALOG_FIELD}
        aria-invalid={problem?.field === 'password' ? true : undefined}
        value={draft.password}
        onChange={(e) => setDraft({ ...draft, password: e.target.value })}
      />
    </Field>
  )

  const existing = step === 'address' ? problem?.existing : undefined
  const footer =
    rotation !== null ? undefined : existing ? (
      <>
        <Button
          variant="ghost"
          onClick={() => {
            onChangeAddress()
            labelRef.current?.focus()
            labelRef.current?.select()
          }}
        >
          Change address
        </Button>
        <Button variant="accent" autoFocus onClick={() => onUseExisting(existing)}>
          Use {existing.label}
        </Button>
      </>
    ) : step === 'done' ? (
      <Button variant="accent" autoFocus onClick={onClose}>
        Done
      </Button>
    ) : step === 'address' ? (
      <>
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button type="submit" form={formId} variant="accent" disabled={busy !== null}>
          {busy ?? 'Continue'}
        </Button>
      </>
    ) : step === 'sign-in' ? (
      <>
        <Button variant="ghost" disabled={busy !== null} onClick={onBack}>
          Back
        </Button>
        <Button type="submit" form={formId} variant="accent" disabled={busy !== null}>
          {busy ?? 'Sign in'}
        </Button>
      </>
    ) : (
      <>
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button type="submit" form={formId} variant="accent" disabled={busy !== null}>
          {busy ?? 'Save'}
        </Button>
      </>
    )

  return (
    <AccessSheet
      title={title}
      intro={intro}
      onClose={onClose}
      // Held mid-check or mid-sign-in, and mid-rotation (the server is
      // saved with a temporary password): the buttons there decide.
      busy={busy !== null || rotation !== null}
      testId="connections-server-dialog"
      footer={footer}
    >
      {rotation ? (
        <>
          <p className={DIALOG_MUTED}>
            {rotation.host.username} on {name}
          </p>
          <PasswordRotationStep
            host={rotation.host}
            initialCurrentPassword={rotation.tempPassword}
            variant="inline"
            onDone={({ newPassword }) => onRotationDone(newPassword)}
            onCancel={onRotationCancel}
          />
        </>
      ) : step === 'done' ? (
        <div className="flex items-start gap-3 pt-2" role="status" data-testid="connections-added">
          <span className="flex pt-1.5">
            <StatusDot tone="ok" />
          </span>
          <div className="min-w-0">
            <p className={SETTING_ROW_LABEL}>
              {draft.voiceOnly ? `Added ${name}` : `Connected to ${name}`}
            </p>
            <p className={`${DIALOG_MUTED} truncate`}>{address}</p>
            <p className={`${DIALOG_MUTED} mt-1`}>
              {draft.voiceOnly
                ? voiceChatAddedText(name)
                : 'It’s in your server list now. Open it from the server menu in the top bar. To let its agents work with your workspaces, pair it from its row.'}
            </p>
          </div>
        </div>
      ) : (
        <form
          id={formId}
          className="space-y-4"
          noValidate
          onSubmit={(e) => {
            e.preventDefault()
            if (busy !== null) return
            if (step === 'address') onContinue(false)
            else onSave()
          }}
        >
          {step === 'address' && (
            <>
              {addressField}
              {nameField}
            </>
          )}
          {step === 'sign-in' && (
            <>
              <div className="flex items-center justify-between gap-4 min-w-0" data-testid="connections-form-server">
                <div className="min-w-0">
                  <p className={`${SETTING_ROW_LABEL} truncate`}>{name}</p>
                  <p className={`${DIALOG_MUTED} truncate`}>{address}</p>
                </div>
                <button
                  type="button"
                  data-k2-focus=""
                  disabled={busy !== null}
                  onClick={onBack}
                  className="type-hint text-[var(--color-accent-text)] hover:underline cursor-pointer flex-shrink-0 disabled:opacity-60"
                >
                  Change
                </button>
              </div>
              {usernameField}
              {passwordField}
              {rememberRow}
            </>
          )}
          {step === 'edit' && (
            <>
              {nameField}
              {addressField}
              {usernameField}
              {passwordField}
              {rememberRow}
            </>
          )}
          {note}
        </form>
      )}
    </AccessSheet>
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
  // Re-pair from a peer row: the app runs both halves when it is signed in
  // to that server; otherwise it shows the steps for the two owners.
  const [repairSteps, setRepairSteps] = useState<{ fingerprint: string; steps: string[] } | null>(null)
  const handleRepairPeer = async (p: FederationPeer): Promise<void> => {
    const signedIn = pairableLocal.find((h) => peerMatchesHostname(p, h.hostname))
    if (signedIn) {
      setRepairSteps(null)
      await handlePair(signedIn)
      return
    }
    setPairMsg(null)
    setRepairSteps({ fingerprint: p.fingerprint, steps: repairStepsForPeer(p, hostLabel) })
  }

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

  const peerTrustWord = (t: FederationPeer['trust']): string =>
    t === 'trusted' ? 'Paired' : t === 'pending' ? 'Waiting for the other server' : 'Blocked'

  return (
    <div className="space-y-6" data-settings-id="connections.active-host-peers">
      <SettingsGroup
        card
        title="Servers"
        hint={
          <>
            You’re using {hostLabel}. Your saved servers are in the server menu in the top bar, and on this page when
            you switch back to this computer.
          </>
        }
      >
        <ConnectionRow
          mark={<StatusDot tone={activeHealth.tone} />}
          title={<span className="truncate">{hostLabel}</span>}
          meta={
            <Facts>
              <StatusFact kind="health" tone={activeHealth.tone}>
                {activeHealth.word}
              </StatusFact>
              <StatusFact kind="active" tone="quiet">
                In use
              </StatusFact>
            </Facts>
          }
          actions={<Button onClick={onConnectLocal}>Switch to this computer</Button>}
        />
      </SettingsGroup>

      <SettingsGroup
        card
        title={`Paired with ${hostLabel}`}
        hint="Servers whose agents can work with agents on this one."
        state={
          loading ? (
            <SettingsLoadState kind="loading" title="Loading paired servers…" />
          ) : !available ? (
            <SettingsLoadState
              kind="error"
              testId="connections-peers-error"
              title="Couldn’t list the servers paired with this one"
              action={
                <Button onClick={() => void reload()}>
                  Try again
                </Button>
              }
            >
              Connect agents across servers may be off there, or you may not be its owner or an admin. To turn it on,
              open {isAirgap() ? 'Servers' : 'Tunnel'} while you’re using this server.
            </SettingsLoadState>
          ) : peers.length === 0 ? (
            <SettingsLoadState kind="empty" title="No paired servers yet">
              Pair one from the list below.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {peers.map((p) => {
          // Trust, not reachability: this list can't say whether a peer is up.
          const trustTone: HealthTone = p.trust === 'pending' ? 'warn' : p.trust === 'trusted' ? 'ok' : 'error'
          return (
            <ConnectionRow
              key={p.fingerprint}
              mark={<StatusDot tone={p.repair_recommended ? 'warn' : trustTone} />}
              title={
                <>
                  <span className="truncate flex-shrink-0 max-w-[60%]">{p.label || p.subdomain || p.fingerprint.slice(0, 12)}</span>
                  <ServerAddress>
                    {p.base_url || p.baseUrl ? p.base_url || p.baseUrl : federatedPeerHost(p) || p.fingerprint.slice(0, 24) + '…'}
                  </ServerAddress>
                </>
              }
              meta={
                <Facts>
                  <StatusFact kind="paired" tone={trustTone}>
                    {peerTrustWord(p.trust)}
                  </StatusFact>
                  {p.repair_recommended && (
                    <StatusFact kind="paired" tone="warn" title={REPAIR_RECOMMENDED_HINT}>
                      Pair again recommended
                    </StatusFact>
                  )}
                </Facts>
              }
              actions={
                p.repair_recommended ? (
                  <Button
                    data-testid="connections-peer-repair"
                    disabled={pairBusyHost !== null}
                    onClick={() => void handleRepairPeer(p)}
                  >
                    Pair again
                  </Button>
                ) : undefined
              }
            >
              {p.repair_recommended && (
                <div className="type-hint text-[var(--color-text-secondary)] leading-relaxed space-y-2">
                  <p>{REPAIR_RECOMMENDED_HINT}</p>
                  {repairSteps?.fingerprint === p.fingerprint && (
                    <ol className="list-decimal pl-5 space-y-1" data-testid="connections-peer-repair-steps">
                      {repairSteps.steps.map((step) => (
                        <li key={step} className="select-text">
                          {step}
                        </li>
                      ))}
                    </ol>
                  )}
                </div>
              )}
            </ConnectionRow>
          )
        })}
      </SettingsGroup>

      <div>
        <SettingsGroup
          card
          title="Pair from this computer"
          hint={`Lets ${hostLabel} and a server you’re signed in to on this computer trust each other, so their agents can work together.`}
          state={
            pairableLocal.length === 0 ? (
              <SettingsLoadState kind="empty" title="No servers signed in on this computer">
                Switch to this computer, sign in to a server there, then come back to pair it.
              </SettingsLoadState>
            ) : undefined
          }
        >
          {pairableLocal.map((h) => (
            <ConnectionRow
              key={h.id}
              title={
                <>
                  <span className="truncate flex-shrink-0 max-w-[60%]">{h.label || h.hostname}</span>
                  <ServerAddress>{h.secure ? h.hostname : savedHostBaseUrl(h)}</ServerAddress>
                </>
              }
              actions={
                <Button disabled={pairBusyHost !== null} onClick={() => void handlePair(h)}>
                  {pairBusyHost === h.id ? 'Pairing…' : 'Pair'}
                </Button>
              }
            />
          ))}
        </SettingsGroup>
        {pairMsg && (
          <div className="mt-3">
            {pairMsg.ok ? (
              <Callout tone="ok" role="status" className="type-hint">
                {pairMsg.text}
              </Callout>
            ) : (
              <ProblemNote
                what="Couldn’t pair the two servers."
                next="Both need Connect agents across servers turned on. Try again once it is."
                detail={pairMsg.text}
              />
            )}
          </div>
        )}
      </div>
    </div>
  )
}

/** A row action that failed: what, in plain words, and the server's reason. */
type RowProblem = { what: string; next?: string; detail?: string }

/** One saved-server tile. Owns its own per-host operation state (restart /
 *  update-check / update / federation badge / pair-as-peer) and drives THAT
 *  host's daemon via its OWN `{base, token}` (host-ops `remoteCreds(h)`), never
 *  the active connection — so the owner can operate a connected server
 *  straight from its tile without switching to it. A signed-out host (no
 *  token) disables the owner-gated controls and shows a "sign in" hint.
 *
 *  "Pair as federated peer" establishes mutual trust between the ACTIVE
 *  daemon (Local or whichever remote you're signed into) and this saved host —
 *  the missing step between "Federation on" and the Federated Connections
 *  server picker. Hidden when this tile is the active host (can't pair self). */
function HostTile({
  host,
  isActive,
  connectionStatus,
  activePeerSideLabel,
  onEdit,
  onRemove,
  onFederationPeersChanged,
  highlight = false,
}: {
  host: ConnectHost
  isActive: boolean
  connectionStatus: ConnectionStatus
  /** Display name of the ACTIVE daemon for Peer/Pair copy ("This Mac" / server label). */
  activePeerSideLabel: string
  onEdit: () => void
  onRemove: () => void
  onFederationPeersChanged?: () => void
  /** Banded for a moment: picked from the Add dialog as already saved. */
  highlight?: boolean
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
  const [restartMsg, setRestartMsg] = useState<{ ok: boolean; text: string; detail?: string } | null>(null)
  const [checkBusy, setCheckBusy] = useState(false)
  const [checkError, setCheckError] = useState<RowProblem | null>(null)
  const [summary, setSummary] = useState<CheckSummary | null>(null)
  const [hostCurrent, setHostCurrent] = useState<string | undefined>(undefined)
  // From the last successful check — drives Shape A vs B (auto-apply) and the
  // version-gated comeback after apply. Absent on older hosts → treat as Shape B.
  const [installKind, setInstallKind] = useState<UpdateCheckResult['installKind'] | undefined>(
    undefined,
  )
  const [updateBusy, setUpdateBusy] = useState(false)
  const [updateError, setUpdateError] = useState<RowProblem | null>(null)
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
      // "Check pairing" on a paired server re-pairs (both rows move to the
      // current code); a first Pair pairs.
      await autoPairWithHost(host.hostname, { repair: peerPaired === 'yes' })
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
            setUpdateError({
              what: `The update didn’t finish: ${label} is on v${observed ?? 'an unexpected version'}, not v${expected}.`,
              next: 'Check for updates again from the row’s menu.',
            })
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
          ? { what: `${label} hasn’t come back on v${expected}.`, next: 'Check for updates again from the row’s menu.' }
          : { what: `${label} still isn’t responding.`, next: `Check that it’s running, then use Try again. If it stays down, quit and reopen ${PRODUCT_NAME}.` },
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
        text: isForbiddenError(m) ? `Only ${label}’s owner or an admin can restart it.` : `Couldn’t restart ${label}.`,
        detail: isForbiddenError(m) ? undefined : m,
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
        setCheckError({ what: `Couldn’t check ${label} for updates.`, detail: m })
        return
      }
      clearIfAuthError(m)
      setCheckError(isForbiddenError(m) ? { what: updateForbiddenCopy(label) } : { what: `Couldn’t check ${label} for updates.`, detail: m })
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
    const fail = (what: string, detail?: string): 'failed' => {
      if (aliveRef.current) {
        setPhaseText(null)
        setUpdateError({ what, detail })
      } else {
        useToastStore.getState().addToast(detail ? `${what} ${detail}` : what, 'error', 10000)
      }
      return 'failed'
    }
    const refused = (err: unknown): 'failed' => {
      const m = errMsg(err)
      // An unreadable reply quotes the body, which may say "Forbidden":
      // it is never a permission refusal.
      if (err instanceof UpdateReplyError) return fail(`Couldn’t install the update on ${label}.`, m)
      clearIfAuthError(m)
      return isForbiddenError(m) ? fail(updateForbiddenCopy(label)) : fail(`Couldn’t install the update on ${label}.`, m)
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
        return outcome.status.error
          ? fail(`The update on ${label} failed.`, outcome.status.error)
          : fail(updatePhaseCopy(outcome.status.phase, label, { current: hostCurrent }))
      case 'poll-failed':
      case 'apply-failed':
        return refused(outcome.err)
      case 'timed-out':
        return fail(`The update on ${label} is taking too long. Check for updates from the row’s menu to see whether it finished.`)
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
        isForbiddenError(m) ? { what: updateForbiddenCopy(label) } : { what: `Couldn’t start the update on ${label}.`, detail: m },
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
            title: `${activePeerSideLabel} is already paired with this server. Check the pairing and fix it if needed.`,
            onSelect: () => void doPairPeer(),
          }
        : {
            id: 'pair',
            label: pairBusy ? 'Pairing…' : peerPaired === 'checking' ? 'Checking…' : 'Pair',
            disabled: pairBusy || reconnecting || federation === 'off',
            hint: federation === 'off' ? `First turn on Connect agents across servers (in Tunnel) on ${peerSide} and on this server` : undefined,
            title:
              federation === 'off'
                ? `First turn on Connect agents across servers (in Tunnel) on ${peerSide} and on this server`
                : `Let ${peerSide} and this server trust each other, so their agents can work together`,
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
        message: `${PRODUCT_NAME} forgets ${label} on this computer, including its saved password and sign-in. Nothing changes on the server, and you can add it again later.`,
        confirmLabel: 'Remove',
        destructive: true,
      }).then((ok) => {
        if (ok) onRemove()
      })
    },
  })

  // One button at most, the thing this server needs next (none while its
  // health is still being checked). Whatever isn't promoted here stays in
  // the menu. Only a server that can't be reached
  // gets a new action: asking again.
  const primaryId: string | null =
    reach === 'offline' && !reconnecting && !isActive
      ? 'try-again'
      : signedOut
        ? 'sign-in'
        : !signedIn || (reach === 'checking' && !hasFailure)
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
      data-host-id={host.id}
      data-highlight={highlight ? '' : undefined}
      className={highlight ? SETTING_LIST_ROW_BAND : undefined}
      mark={<StatusDot tone={health.tone} />}
      // Line 1: the name and its address. Line 2: its state in words, always
      // in the same order (health, password, pairing, agents across servers,
      // in use), so every row reads the same way whatever its address.
      title={
        <>
          <span className="truncate flex-shrink-0 max-w-[60%]">{label}</span>
          {voiceOnly && <VoiceChatOnlyChip />}
          <ServerAddress lock={host.secure ? <IconLock className="w-2.5 h-2.5 flex-shrink-0" /> : undefined}>
            {hostText}
          </ServerAddress>
        </>
      }
      meta={
        <Facts>
          <StatusFact kind="health" tone={health.tone}>
            {health.word}
          </StatusFact>
          {host.remember && (
            <StatusFact kind="saved" tone="quiet" title="Its password is saved in this computer’s keychain">
              Password saved
            </StatusFact>
          )}
          {!isActive && ((peerPaired === 'yes' && !signedOut) || (peerPaired === 'no' && signedIn)) && (
            <StatusFact
              kind="paired"
              tone="quiet"
              title={
                peerPaired === 'yes'
                  ? `${activePeerSideLabel} trusts this server, so their agents can work together`
                  : `${activePeerSideLabel} and this server don’t trust each other yet`
              }
            >
              {peerPaired === 'yes' ? 'Paired' : 'Not paired'}
            </StatusFact>
          )}
          {federation === 'off' && (
            <StatusFact kind="federation" tone="quiet" title="Connect agents across servers is off on this server">
              Agents across servers off
            </StatusFact>
          )}
          {/* "In use" only — switching to a host happens from the server
              switcher, not here. The row is for managing the host in place. */}
          {isActive && (
            <StatusFact kind="active" tone="quiet">
              In use
            </StatusFact>
          )}
          {signedOut && reach !== 'offline' && (
            <StatusFact kind="saved" tone="quiet">
              Sign in to manage it
            </StatusFact>
          )}
        </Facts>
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
        <p className="type-hint text-[var(--color-text-secondary)] leading-relaxed">
          {PRODUCT_NAME} can’t reach this server. Check that it’s running and on your network, then try again.
        </p>
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
      {restartMsg && !restartMsg.ok && <ProblemNote what={restartMsg.text} detail={restartMsg.detail} />}
      {pairMsg && !pairMsg.ok && (
        <ProblemNote
          what={`Couldn’t pair with ${label}.`}
          next="Both servers need Connect agents across servers turned on (in Tunnel). Try again once they do."
          detail={pairMsg.text}
        />
      )}
      {checkError && <ProblemNote what={checkError.what} next={checkError.next} detail={checkError.detail} />}
      {updateError && <ProblemNote what={updateError.what} next={updateError.next} detail={updateError.detail} />}
    </ConnectionRow>
  )
}
