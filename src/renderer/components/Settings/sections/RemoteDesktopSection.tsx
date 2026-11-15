// Settings → Access → Remote Desktop (Beta; prd-remote-desktop-tab-v1 §5,
// §8): a front end for `/cli/desktop/*` on the window's server. The
// daemon owns every rule: the switch is an Owner's (on until an Owner turns
// it off), Owners and Admins open the desktop, a Member only with a grant
// and only to watch. This page keeps no logic of its own: it shows what
// `desktop/status` says and sends the Owner's gestures.
//
// Owner: the switch, the hide-list, grants (add a Member to watch; remove),
// viewers (disconnect), the access log (last 50). Everyone else: the state,
// read only. Anything that cuts someone off (turning it off while someone
// is watching, removing a grant, disconnecting a viewer) asks first.

import { useCallback, useEffect, useState } from 'react'
import { BetaTag, Button, Input, Textarea, Toggle } from '@/components/ui'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { onDesktopState } from '@/stores/session-events'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import {
  askDesktop,
  disableDesktop,
  enableDesktop,
  fetchDesktopAudit,
  grantDesktop,
  kickDesktopViewer,
  revokeDesktop,
  setDesktopHideApps,
  type DesktopAuditLine,
} from '@/components/Desktop/desktop-api'
import { daemonCliGet } from '@/lib/daemon-cli'
import {
  DESKTOP_ENABLE_CARD,
  DESKTOP_FEATURE,
  VIRTUAL_DESKTOP_FEATURE,
  VIRTUAL_ENABLE_CARD,
  readDesktopStatus,
  type DesktopGrant,
  type DesktopPermission,
  type DesktopStatus,
  type DesktopViewer,
  type PermissionPane,
} from '@/components/Desktop/desktop-state'
import {
  SETTING_FIELD_WIDTH,
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  SettingSwitchRow,
} from '../controls/SettingControls'
import { HealthWord, StatusDot, type HealthTone } from './ConnectionsParts'
import { ActionNote, readFailure, useActionNote } from './ActionNote'
import type { SettingEntry } from '../searchManifest'

export const REMOTE_DESKTOP_MANIFEST: SettingEntry[] = [
  {
    id: 'remote-desktop.switch',
    section: 'remote-desktop',
    label: 'Remote Desktop',
    description: "See and drive this computer's screen from another K2 (people only)",
    keywords: ['remote desktop', 'screen', 'screen sharing', 'vnc', 'control', 'desktop tab', 'k2 screen'],
  },
  {
    id: 'remote-desktop.grants',
    section: 'remote-desktop',
    label: 'Remote Desktop grants',
    description: 'Let a Member watch this screen',
    keywords: ['grant', 'member', 'watch', 'view only'],
  },
]

/** Is the page shown for the window's server? */
export function remoteDesktopAvailable(scope: ServerScope): boolean {
  return scope.serverSupports(DESKTOP_FEATURE)
}

const CONTENT_WIDTH = 'max-w-[52rem]'

/** The status read, through the shape guard (an odd reply is an error card,
 *  never a crash). */
async function fetchStatus(scope: ServerScope): Promise<DesktopStatus> {
  return readDesktopStatus(await daemonCliGet<unknown>(scope, 'desktop/status'))
}

/** The machine's state in a sentence, under its name. */
function stateLine(s: DesktopStatus): string {
  if (!s.enabled) return 'Off. Nobody can open this screen.'
  if (s.state === 'ready') return 'On, and ready for the people you allow.'
  return s.message || 'On, but the screen isn’t ready yet.'
}

/** The machine's state as one word and a tone. */
function health(s: DesktopStatus): { tone: HealthTone; word: string } {
  if (!s.enabled) return { tone: 'neutral', word: 'Off' }
  if (s.state === 'ready') return { tone: 'ok', word: 'Ready' }
  if (s.state === 'unavailable') return { tone: 'neutral', word: 'Not available' }
  return { tone: 'warn', word: 'Needs attention' }
}

const dateTime = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
const dateOnly = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' })

function when(ms: number, fmt: Intl.DateTimeFormat = dateTime): string {
  if (!Number.isFinite(ms) || ms <= 0) return ''
  return fmt.format(new Date(ms))
}

function person(who: string | undefined): string {
  if (!who) return 'Someone'
  return who === 'owner' ? 'The owner' : who
}

/** One access-log line in words: "jordan connected to use the screen". */
export function auditSentence(l: DesktopAuditLine): string {
  const who = person(l.who)
  const target = l.target ? person(l.target).replace(/^The owner$/, 'the owner') : 'someone'
  const what: Record<string, string> = {
    enable: 'turned Remote Desktop on',
    disable: 'turned Remote Desktop off',
    policy: 'changed the hidden apps',
    grant: `let ${target} watch`,
    revoke: `stopped letting ${target} watch`,
    kick: `disconnected ${target}`,
    open: 'opened the screen',
    ticket: 'started a session',
    connect: l.mode === 'control' ? 'connected to use the screen' : 'connected to watch',
    disconnect: 'disconnected',
    control_taken: 'took control',
    control_handed_back: 'handed control back',
    control_timed_out: 'lost control after a pause',
    local_hold: 'was paused while someone used this computer',
    mode_change: l.mode === 'control' ? 'switched to using the screen' : 'switched to watching',
    refused: 'was turned away',
    clipboard_read: 'copied from this computer’s clipboard',
    clipboard_write: 'pasted into this computer’s clipboard',
  }
  const verb = what[l.event] ?? l.event.replace(/_/g, ' ')
  const outcome = l.outcome && l.outcome !== 'ok' ? ` (${l.outcome === 'unchanged' ? 'no change' : l.outcome.replace(/_/g, ' ')})` : ''
  return `${who} ${verb}${outcome}`
}

/** The two macOS permissions as steps a person follows at a glance. */
export const PERMISSION_STEPS: ReadonlyArray<{ pane: PermissionPane; label: string; hint: string }> = [
  { pane: 'screen', label: 'Screen Recording', hint: 'Lets K2 Screen show this screen to the people you allow.' },
  {
    pane: 'accessibility',
    label: 'Accessibility',
    hint: 'Lets the people you allow type and click on this Mac from another computer.',
  },
]

/** After the switch went on: what to do next, in one line. */
export function turnedOnNote(permission: DesktopPermission | undefined): string {
  if (!permission || permission.step === 'ready') return 'Remote Desktop is on. Owners and Admins can open this screen now.'
  return 'Remote Desktop is on. Allow K2 Screen in the macOS prompts on this Mac; this page updates by itself.'
}

function PermissionSteps({
  permission,
  manage,
  busy,
  machine,
  sameMachine,
  onOpen,
  onCheck,
  note,
}: {
  permission: DesktopPermission
  manage: boolean
  busy: boolean
  machine: string
  sameMachine: boolean
  onOpen: (pane: PermissionPane) => void
  onCheck: () => void
  note: React.ReactNode
}): React.JSX.Element {
  return (
    <div data-testid="remote-desktop-permission" data-step={permission.step}>
      <SettingRow
        label="Allow K2 Screen on this Mac"
        hint={
          <span data-testid="remote-desktop-permission-line">
            {permission.line}
            {permission.checking ? ' K2 notices by itself when you do.' : ''}
          </span>
        }
        testId="remote-desktop-permission-head"
        below={note ?? undefined}
      >
        {manage && !permission.checking ? (
          <Button disabled={busy} data-testid="remote-desktop-permission-check" onClick={onCheck}>
            Check again
          </Button>
        ) : (
          <span className="type-row text-[var(--color-text-secondary)]">{permission.checking ? 'Checking…' : ''}</span>
        )}
      </SettingRow>
      {PERMISSION_STEPS.map(({ pane, label, hint }) => {
        const allowed = (pane === 'screen' ? permission.screen : permission.accessibility) === 'allowed'
        return (
          <SettingRow
            key={pane}
            label={label}
            hint={!sameMachine && manage && !allowed ? `${hint} The button opens System Settings on ${machine}.` : hint}
            testId={`remote-desktop-permission-${pane}`}
          >
            <span className="inline-flex items-center gap-3 type-row">
              <span className="inline-flex items-center gap-2" data-permission-word={allowed ? 'allowed' : 'waiting'}>
                <StatusDot tone={allowed ? 'ok' : 'warn'} />
                <HealthWord tone={allowed ? 'ok' : 'warn'}>{allowed ? 'Allowed' : 'Waiting'}</HealthWord>
              </span>
              {manage && !allowed ? (
                <Button disabled={busy} data-testid={`remote-desktop-permission-open-${pane}`} onClick={() => onOpen(pane)}>
                  Open System Settings
                </Button>
              ) : null}
            </span>
          </SettingRow>
        )
      })}
    </div>
  )
}

export function RemoteDesktopSection({ scope = primaryScope() }: { scope?: ServerScope }): React.JSX.Element {
  const [status, setStatus] = useState<DesktopStatus | null>(null)
  const [loadError, setLoadError] = useState<unknown>(null)
  const [audit, setAudit] = useState<DesktopAuditLine[] | null>(null)
  const [auditError, setAuditError] = useState<unknown>(null)
  const [busy, setBusy] = useState(false)
  const [retrying, setRetrying] = useState(false)
  const [grantUser, setGrantUser] = useState('')
  const [hideDraft, setHideDraft] = useState<string | null>(null)
  const switchNote = useActionNote()
  const hideNote = useActionNote()
  const grantNote = useActionNote()
  const viewerNote = useActionNote()
  const permissionNote = useActionNote()

  const loadAudit = useCallback(async () => {
    try {
      setAudit(await fetchDesktopAudit(scope, 50))
      setAuditError(null)
    } catch (err) {
      setAuditError(err ?? new Error('unknown'))
    }
  }, [scope])

  const load = useCallback(async () => {
    try {
      const s = await fetchStatus(scope)
      setStatus(s)
      setLoadError(null)
      if (s.you.mayManage) await loadAudit()
    } catch (err) {
      setLoadError(err ?? new Error('unknown'))
    }
  }, [scope, loadAudit])

  const supported = remoteDesktopAvailable(scope)
  useEffect(() => {
    // An older server has no routes: never ask it.
    if (!supported) return
    void load()
    return onDesktopState(scope, () => void load())
  }, [scope, load, supported])

  /** One write: busy while it runs, a note after, and a fresh read. */
  const act = async (
    fn: () => Promise<unknown>,
    note: ReturnType<typeof useActionNote>,
    ok: string | ((reply: unknown) => string),
    fail: string,
  ): Promise<boolean> => {
    setBusy(true)
    note.clear()
    let landed = false
    try {
      const reply = await fn()
      note.done(typeof ok === 'function' ? ok(reply) : ok)
      landed = true
    } catch (err) {
      note.failed(fail, err)
    } finally {
      setBusy(false)
      await load()
    }
    return landed
  }

  const confirm = useConfirmDialogStore((s) => s.confirm)

  const header = (
    <header className="min-w-0">
      <h2 className="type-title text-[var(--color-text-primary)] flex items-center gap-2">
        Remote Desktop
        <BetaTag />
      </h2>
      <p className={SETTING_ROW_HINT}>
        Open this computer’s screen in a Desktop tab on another K2, live. For people only: agents can’t use it.
      </p>
    </header>
  )

  if (!supported) {
    return (
      <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`}>
        {header}
        <SettingsLoadState kind="empty" title="This server can’t share its screen yet" testId="remote-desktop-unsupported">
          Update K2 on this computer to use Remote Desktop.
        </SettingsLoadState>
      </div>
    )
  }

  if (!status) {
    const fail = loadError ? readFailure(loadError) : null
    return (
      <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`} data-testid="remote-desktop-section">
        {header}
        {fail ? (
          <SettingsLoadState
            kind="error"
            title="Couldn’t load Remote Desktop"
            detail={fail.detail}
            testId="remote-desktop-error"
            action={
              <Button
                disabled={retrying}
                onClick={() => {
                  setRetrying(true)
                  void load().finally(() => setRetrying(false))
                }}
              >
                {retrying ? 'Trying…' : 'Try again'}
              </Button>
            }
          >
            K2 couldn’t read this computer’s screen-sharing settings.
            {fail.shape ? <span className="block mt-1 text-[var(--color-text-muted)]">Details: {fail.shape}.</span> : null}
          </SettingsLoadState>
        ) : (
          <SettingsLoadState kind="loading" title="Loading Remote Desktop…" testId="remote-desktop-loading" />
        )}
      </div>
    )
  }

  const manage = status.you.mayManage
  // A Linux server with no screen: This server is its `virtual` desktop,
  // and the switch turns on every virtual desktop there (§4 item 2), with
  // its own card (LD-18).
  const virtual = scope.serverSupports(VIRTUAL_DESKTOP_FEATURE) && status.desktop.startsWith('virtual')
  const h = health(status)
  const machine = status.machine || 'This computer'
  const viewers = status.viewers ?? []
  const grants = status.grants ?? []
  const savedHide = (status.hideApps ?? []).join('\n')
  const hideApps = hideDraft ?? savedHide
  const hideChanged = hideDraft !== null && hideDraft.trim() !== savedHide.trim()

  const flip = async (next: boolean): Promise<void> => {
    if (next) {
      const yes = await confirm({ title: 'Turn on Remote Desktop?', message: virtual ? VIRTUAL_ENABLE_CARD : DESKTOP_ENABLE_CARD, confirmLabel: 'Turn on' })
      if (!yes) return
      // One action (Rosson 2026-11-15): on a Mac the daemon starts K2
      // Screen, which asks macOS for its permission at once.
      await act(
        () => enableDesktop(scope, virtual ? 'virtual' : undefined),
        switchNote,
        (r) => turnedOnNote((r as { macPermission?: DesktopPermission } | undefined)?.macPermission),
        'Couldn’t turn on Remote Desktop. It’s still off.',
      )
      return
    }
    if (viewers.length > 0) {
      const names = viewers.map((v) => v.user).join(', ')
      const yes = await confirm({
        title: 'Turn off Remote Desktop?',
        message: `${names} ${viewers.length === 1 ? 'has' : 'have'} this screen open and will be disconnected now. Nobody can open it until an Owner turns it on again.`,
        confirmLabel: 'Turn off',
        destructive: true,
      })
      if (!yes) return
    }
    await act(() => disableDesktop(scope), switchNote, 'Remote Desktop is off. Nobody can open this screen.', 'Couldn’t turn off Remote Desktop. It’s still on.')
  }

  const addGrant = async (): Promise<void> => {
    const user = grantUser.trim()
    if (!user) return
    if (await act(() => grantDesktop(scope, user), grantNote, `${user} can now watch this screen.`, `Couldn’t let ${user} watch.`)) setGrantUser('')
  }

  const removeGrant = async (g: DesktopGrant): Promise<void> => {
    const watching = viewers.some((v) => v.user === g.user)
    const yes = await confirm({
      title: `Stop letting ${g.user} watch?`,
      message: watching
        ? `${g.user} is watching now and will be disconnected. You can let them watch again at any time.`
        : `${g.user} won’t be able to open this screen. You can let them watch again at any time.`,
      confirmLabel: 'Stop sharing',
      destructive: true,
    })
    if (!yes) return
    await act(() => revokeDesktop(scope, g.user), grantNote, `${g.user} can no longer watch this screen.`, `Couldn’t stop ${g.user} watching. They can still watch.`)
  }

  const disconnect = async (v: DesktopViewer): Promise<void> => {
    const yes = await confirm({
      title: `Disconnect ${v.user}?`,
      message: `${v.user}’s Desktop tab closes now. They can open it again while they still have access.`,
      confirmLabel: 'Disconnect',
      destructive: true,
    })
    if (!yes) return
    await act(() => kickDesktopViewer(scope, v.id), viewerNote, `${v.user} was disconnected.`, `Couldn’t disconnect ${v.user}. They’re still connected.`)
  }

  const openPane = (pane: PermissionPane): void => {
    void act(
      () => askDesktop(scope, { open: pane }),
      permissionNote,
      `Opened System Settings on ${machine}. Allow K2 Screen there.`,
      'Couldn’t open System Settings.',
    )
  }
  const checkAgain = (): void => {
    void act(() => askDesktop(scope, { check: true }), permissionNote, 'Checking again.', 'Couldn’t check again.')
  }

  const saveHidden = async (): Promise<void> => {
    const list = hideApps
      .split('\n')
      .map((l) => l.trim())
      .filter(Boolean)
    if (await act(() => setDesktopHideApps(scope, list), hideNote, 'Saved. These apps stay out of the picture.', 'Couldn’t save the hidden apps. Nothing changed; try again.')) setHideDraft(null)
  }

  return (
    <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`} data-settings-id="remote-desktop.switch" data-testid="remote-desktop-section">
      {header}

      <SettingsGroup title="This computer" card testId="remote-desktop-computer">
        {manage ? (
          <SettingSwitchRow
            testId="remote-desktop-switch-row"
            control={
              <Toggle
                checked={status.enabled}
                disabled={busy}
                aria-label="Remote Desktop"
                data-testid="remote-desktop-switch"
                onChange={(next) => void flip(next)}
              />
            }
          >
            <div className="type-row text-[var(--color-text-primary)]">Share this screen</div>
            <div className={SETTING_ROW_HINT}>
              Owners and Admins can open it and use it. Members can only watch, and only if you let them. It stays on until an Owner
              turns it off.
            </div>
            {switchNote.note ? (
              <div className="mt-2">
                <ActionNote note={switchNote.note} testId="remote-desktop-switch-note" />
              </div>
            ) : null}
          </SettingSwitchRow>
        ) : null}
        <SettingRow
          label={machine}
          hint={<span data-testid="remote-desktop-state">{stateLine(status)}</span>}
          testId="remote-desktop-machine"
        >
          <span className="inline-flex items-center gap-2 type-row">
            <StatusDot tone={h.tone} />
            <HealthWord tone={h.tone}>{h.word}</HealthWord>
          </span>
        </SettingRow>
        {status.enabled && status.macPermission && status.macPermission.step !== 'ready' ? (
          <PermissionSteps
            permission={status.macPermission}
            manage={manage}
            busy={busy}
            machine={machine}
            sameMachine={status.you.sameMachine}
            onOpen={openPane}
            onCheck={checkAgain}
            note={permissionNote.note ? <ActionNote note={permissionNote.note} testId="remote-desktop-permission-note" /> : null}
          />
        ) : null}
        {!manage ? (
          <SettingRow label="Who can change this" hint="Only the server’s owner can turn Remote Desktop on or off and choose who may watch." testId="remote-desktop-readonly">
            <span className="type-row text-[var(--color-text-secondary)]">The owner</span>
          </SettingRow>
        ) : null}
      </SettingsGroup>

      {manage ? (
        <>
          <SettingsGroup
            title="Hidden apps"
            hint={status.backend === 'linux' ? 'On Linux, K2 can’t hide apps from the picture.' : 'Windows of these apps never show in the picture, so passwords and wallets stay private.'}
            card
            testId="remote-desktop-hidden"
          >
            <SettingRow
              stack
              label="Apps to hide"
              hint="One app ID a line, like com.apple.Passwords."
              testId="remote-desktop-hide-row"
              below={hideNote.note ? <ActionNote note={hideNote.note} testId="remote-desktop-hide-note" /> : undefined}
            >
              <Textarea
                className="font-mono w-full"
                rows={4}
                aria-label="Apps to hide"
                data-testid="remote-desktop-hide-apps"
                value={hideApps}
                onChange={(e) => {
                  hideNote.clear()
                  setHideDraft(e.target.value)
                }}
              />
              <div className="flex items-center gap-2">
                <Button variant="accent" disabled={busy || !hideChanged} data-testid="remote-desktop-hide-save" onClick={() => void saveHidden()}>
                  Save
                </Button>
                {hideChanged ? (
                  <Button variant="ghost" disabled={busy} onClick={() => setHideDraft(null)}>
                    Undo changes
                  </Button>
                ) : null}
              </div>
            </SettingRow>
          </SettingsGroup>

          <SettingsGroup
            title="Who can watch"
            hint="Members you add can watch this screen but never use it. Owners and Admins don’t need to be added."
            card
            settingId="remote-desktop.grants"
            testId="remote-desktop-grants"
          >
            <SettingRow
              label="Let a Member watch"
              hint="Their K2 username on this server."
              testId="remote-desktop-grant-row"
              below={grantNote.note ? <ActionNote note={grantNote.note} testId="remote-desktop-grant-note" /> : undefined}
            >
              <form
                className="flex items-center gap-2"
                onSubmit={(e) => {
                  e.preventDefault()
                  void addGrant()
                }}
              >
                <Input
                  className={SETTING_FIELD_WIDTH}
                  value={grantUser}
                  placeholder="Username"
                  aria-label="Member’s username"
                  data-testid="remote-desktop-grant-user"
                  onChange={(e) => {
                    grantNote.clear()
                    setGrantUser(e.target.value)
                  }}
                />
                <Button type="submit" disabled={busy || !grantUser.trim()} data-testid="remote-desktop-grant-add">
                  Let watch
                </Button>
              </form>
            </SettingRow>
            {grants.length === 0 ? (
              <div className="px-4 py-3 type-hint text-[var(--color-text-secondary)]" data-testid="remote-desktop-grants-empty">
                No Members can watch yet.
              </div>
            ) : (
              grants.map((g) => (
                <div key={g.userId} className={SETTING_LIST_ROW_HOVER} data-testid="remote-desktop-grant">
                  <SettingRow
                    label={<span className="type-name">{g.user}</span>}
                    hint={[g.mode === 'view' ? 'Can watch' : 'Can use the screen', g.grantedBy ? `added by ${person(g.grantedBy).replace(/^The owner$/, 'the owner')}` : '', when(g.grantedAt * 1000, dateOnly)].filter(Boolean).join(' · ')}
                  >
                    <Button variant="danger-quiet" disabled={busy} aria-label={`Stop letting ${g.user} watch`} onClick={() => void removeGrant(g)}>
                      Remove
                    </Button>
                  </SettingRow>
                </div>
              ))
            )}
          </SettingsGroup>

          <SettingsGroup title="Open now" hint="People who have this screen open in a Desktop tab right now." card testId="remote-desktop-viewers">
            {viewers.length === 0 ? (
              <div className="px-4 py-3 type-hint text-[var(--color-text-secondary)]" data-testid="remote-desktop-viewers-empty">
                Nobody has this screen open.
              </div>
            ) : (
              viewers.map((v) => (
                <div key={v.id} className={SETTING_LIST_ROW_HOVER} data-testid="remote-desktop-viewer">
                  <SettingRow
                    label={<span className="type-name">{v.user}</span>}
                    hint={[v.mode === 'control' ? 'Using the screen' : 'Watching', v.device || '', v.since ? `since ${when(v.since)}` : ''].filter(Boolean).join(' · ')}
                  >
                    <Button variant="danger-quiet" disabled={busy} aria-label={`Disconnect ${v.user}`} onClick={() => void disconnect(v)}>
                      Disconnect
                    </Button>
                  </SettingRow>
                </div>
              ))
            )}
            {viewerNote.note ? (
              <div className="px-4 py-3">
                <ActionNote note={viewerNote.note} testId="remote-desktop-viewer-note" />
              </div>
            ) : null}
          </SettingsGroup>

          <SettingsGroup
            title="Recent activity"
            hint="The last 50 times someone turned this on or off, opened the screen or changed who can watch."
            card
            testId="remote-desktop-audit"
            state={
              auditError && audit === null ? (
                <SettingsLoadState
                  kind="error"
                  title="Couldn’t load recent activity"
                  detail={readFailure(auditError).detail}
                  action={<Button onClick={() => void loadAudit()}>Try again</Button>}
                >
                  The rest of this page is up to date.
                </SettingsLoadState>
              ) : audit === null ? (
                <SettingsLoadState kind="loading" title="Loading recent activity…" />
              ) : audit.length === 0 ? (
                <SettingsLoadState kind="empty" title="Nothing yet">
                  Turning Remote Desktop on, opening the screen and changes to who can watch show here.
                </SettingsLoadState>
              ) : undefined
            }
          >
            {(audit ?? []).map((l, i) => (
              <SettingRow
                key={`${l.ts}-${i}`}
                label={auditSentence(l)}
                hint={[when(Date.parse(l.ts)), l.device ?? ''].filter(Boolean).join(' · ')}
                testId="remote-desktop-audit-line"
              >
                {null}
              </SettingRow>
            ))}
          </SettingsGroup>
        </>
      ) : null}
    </div>
  )
}

export default RemoteDesktopSection
