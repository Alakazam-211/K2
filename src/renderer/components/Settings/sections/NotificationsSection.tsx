// Settings → Notifications (prd-thread-push-notifications-v1 §9, slice S5).
//
// Phone pushes when an agent posts in a Thread, and agent email notices. The
// daemon owns every value (`/cli/notify/*`); this page renders it and sends
// the person's clicks. Card groups, top to bottom:
//
//   1. Your phone (every login): whether pushes can reach a phone (a status
//      row with the next step), then the server bell.
//   2. Pushes by workspace: one switch per workspace with where its value
//      came from ("Default (on)", "Set by you", "Set by alex" + Reset).
//   3. Email notices: the person's own address and per-workspace switches.
//   4. Message previews, Workspace defaults (Owner/Admin): what new and unset
//      people get.
//   5. People (Owner/Admin): everyone's settings. Admins see them; only the
//      owner changes other people's switches (Rosson 2026-10-11). Each
//      person is one collapsed row (name, role, a summary); one opens at a
//      time (Rosson 2026-10-18).
//
// Hidden entirely unless the window's server reports `thread-push-v1`.
// Refetches on `notify_prefs_changed` (no payload).

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import {
  SETTING_LIST_ROW_BAND,
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SETTING_SERVER_REASON,
  SaveError,
  SaveNote,
  SettingSwitchRow,
  SettingsGroup,
  SettingsLoadState,
  type SaveState,
} from '../controls/SettingControls'
import { Button, Toggle } from '@/components/ui'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { primaryScope } from '@/kessel/server-scope'
import { onNotifyPrefsChanged } from '@/stores/session-events'
import {
  deviceCountText,
  fetchNotifyAdmin,
  fetchNotifyCaller,
  phoneStatus,
  setMyServerBell,
  setMyWorkspacePush,
  setPersonServerBell,
  setPersonWorkspacePush,
  setWorkspaceDefault,
  sourceLabel,
  type NotifyAdmin,
  type NotifyCaller,
  type NotifyPerson,
  type NotifyPrefs,
  type NotifyWorkspacePref,
} from './notify-api'
import { threadPushAvailable, useNotifyPrefs } from './useNotifyPrefs'
import { PushPreviewsRow } from './PushPreviewsRow'
import { EmailNoticePrefs } from './EmailNoticePrefs'
import { StatusDot } from './ConnectionsParts'
import { PRODUCT_NAME } from '@/lib/brand'

export const NOTIFICATIONS_MANIFEST: SettingEntry[] = [
  {
    id: 'notifications.server-bell',
    section: 'notifications',
    label: 'Push to My Phones From This Server',
    description: 'Phone notifications from this server: Thread, ticket and project pushes',
    keywords: ['notification', 'notifications', 'push', 'phone', 'companion', 'bell', 'mute', 'quiet'],
    group: 'Your phone',
  },
  {
    id: 'notifications.workspaces',
    section: 'notifications',
    label: 'Thread Pushes per Workspace',
    description: 'Which agents may push your phone when they post in a Thread',
    keywords: ['notification', 'push', 'phone', 'thread', 'workspace', 'agent', 'mute'],
    group: 'Pushes by workspace',
  },
  {
    id: 'notifications.email-notices',
    section: 'notifications',
    label: 'Email Notices From Agents',
    description: 'Your address for agent email notices, and which workspaces may email you',
    keywords: ['email', 'notice', 'notices', 'address', 'confirm', 'agent', 'mute', 'mail'],
    group: 'Email notices',
  },
  {
    id: 'notifications.defaults',
    section: 'notifications',
    label: 'Workspace Notification Defaults',
    description: 'What new and unset people get for each workspace (owner and admins)',
    keywords: ['notification', 'push', 'default', 'workspace', 'admin'],
    group: 'Workspace defaults',
  },
  {
    id: 'notifications.previews',
    section: 'notifications',
    label: 'Show Message Text in Push Notifications',
    description: 'Server-wide switch for message previews on phones (owner and admins)',
    keywords: ['notification', 'push', 'preview', 'previews', 'message', 'text', 'privacy', 'lock screen'],
    group: 'Message previews',
  },
  {
    id: 'notifications.people',
    section: 'notifications',
    label: 'People’s Notifications',
    description: 'Everyone’s push settings on this server (owner and admins)',
    keywords: ['notification', 'push', 'people', 'users', 'admin', 'owner'],
    group: 'People',
  },
]

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

const PAGE = 'w-full max-w-[52rem] space-y-6 pb-6'

/** The page title and its one plain line. */
function Header(): React.JSX.Element {
  return (
    <header className="min-w-0">
      <h2 className="type-title text-[var(--color-text-primary)]">Notifications</h2>
      <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed max-w-[75ch]">
        How agents on this server reach you: a push to your phone when one posts in a Thread, or a short email notice.
      </p>
    </header>
  )
}

/** One row of a card: a switch (and Reset) on the right, the name, where its
 *  value came from and any save note on the left. */
function SwitchRow({
  settingId,
  testId,
  leading,
  label,
  hint,
  state,
  children,
}: {
  settingId?: string
  testId?: string
  leading?: React.ReactNode
  label: React.ReactNode
  hint?: React.ReactNode
  state?: SaveState
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <SettingSwitchRow
      settingId={settingId}
      testId={testId}
      control={
        <>
          {state && <SaveNote state={state} />}
          {children}
        </>
      }
    >
      <div className="flex items-start gap-3 min-w-0">
        {leading ? <div className="flex-shrink-0 pt-0.5">{leading}</div> : null}
        <div className="min-w-0">
          <div className={`${SETTING_ROW_LABEL} truncate`}>{label}</div>
          {hint ? <div className={SETTING_ROW_HINT}>{hint}</div> : null}
          {state && <SaveError state={state} />}
        </div>
      </div>
    </SettingSwitchRow>
  )
}

function ResetButton({
  onClick,
  disabled,
  label,
}: {
  onClick: () => void
  disabled?: boolean
  label: string
}): React.JSX.Element {
  return (
    <Button variant="quiet" onClick={onClick} disabled={disabled} aria-label={label} className="flex-shrink-0">
      Reset
    </Button>
  )
}

/** The workspace's agent avatar, when this window knows the workspace. */
function WorkspaceAvatar({ projectId, name }: { projectId: string; name: string }): React.JSX.Element | null {
  const project = useProjectsStore((s) => s.projects.find((p) => p.id === projectId))
  if (!project) return null
  return (
    <ProjectAvatar
      projectPath={project.path}
      projectName={name}
      projectColor={project.color}
      projectId={project.id}
      iconUrl={project.iconUrl}
      size={18}
    />
  )
}

/** The person-key hint (§3, Q6). Viewing as the owner person (the owner
 *  token or a login linked to it), `aliases` are the Connect logins that
 *  count as you: the ones marked "This is me (owner)" plus a sole
 *  Owner-role login. With none and two or more Owner-role logins, the
 *  owner token is its own person. */
export function ownerHint(
  caller: NotifyCaller | null,
  prefs: NotifyPrefs | null,
  admin: NotifyAdmin | null,
): { kind: 'aliases'; logins: string[] } | { kind: 'separate'; owners: number } | null {
  if (!prefs) return null
  if (prefs.principal === 'owner' && prefs.aliases.length > 0) return { kind: 'aliases', logins: prefs.aliases }
  if (!caller?.ownerToken) return null
  const ownerLogins = admin
    ? admin.people.filter((p) => p.principal !== 'owner' && p.role === 'owner').length
    : 0
  if (ownerLogins >= 2) return { kind: 'separate', owners: ownerLogins }
  return null
}

/** "alex", "alex or sam", "alex, sam or kim". */
function orList(names: string[]): string {
  if (names.length <= 1) return names.join('')
  return `${names.slice(0, -1).join(', ')} or ${names[names.length - 1]}`
}

/** The last write's outcome, on the row it came from: "Saved" for a moment,
 *  or why it failed until the next try. */
function useRowSave(holdMs = 2000): {
  stateOf: (key: string) => SaveState
  saved: (key: string) => void
  failed: (key: string, err: unknown) => void
} {
  const [last, setLast] = useState<{ key: string; state: SaveState } | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current)
    },
    [],
  )
  const saved = useCallback(
    (key: string) => {
      if (timer.current) clearTimeout(timer.current)
      setLast({ key, state: 'saved' })
      timer.current = setTimeout(() => setLast(null), holdMs)
    },
    [holdMs],
  )
  const failed = useCallback((key: string, err: unknown) => {
    if (timer.current) clearTimeout(timer.current)
    const raw = errorText(err).trim()
    setLast({ key, state: { error: raw || `${PRODUCT_NAME} couldn’t reach this server.` } })
  }, [])
  const stateOf = useCallback((key: string): SaveState => (last && last.key === key ? last.state : 'idle'), [last])
  return { stateOf, saved, failed }
}

const openCompanion = (): void => useSettingsStore.getState().openSettings('companion')

export function NotificationsSection(): React.JSX.Element {
  const available = threadPushAvailable()
  const { prefs, error: loadError, adopt, reload } = useNotifyPrefs(available)
  const [caller, setCaller] = useState<NotifyCaller | null>(null)
  const [admin, setAdmin] = useState<NotifyAdmin | null>(null)
  const [adminError, setAdminError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const rows = useRowSave()
  const { saved, failed } = rows
  // People accordion: at most one person open. Local view state only (not
  // the daemon's); keyed by principal so a refetch keeps it open.
  const [openPerson, setOpenPerson] = useState<string | null>(null)

  const isAdmin = caller?.role === 'owner' || caller?.role === 'admin'
  const canEditPeople = caller?.role === 'owner'

  useEffect(() => {
    if (!available) return
    let cancelled = false
    fetchNotifyCaller().then(
      (c) => {
        if (!cancelled) setCaller(c)
      },
      (e: unknown) => {
        // No role known: the page stays on the person's own notifications.
        console.warn('[notifications] whoami failed:', e)
      },
    )
    return () => {
      cancelled = true
    }
  }, [available])

  const reloadAdmin = useCallback(() => {
    fetchNotifyAdmin().then(
      (a) => {
        setAdmin(a)
        setAdminError(null)
      },
      (e: unknown) => setAdminError(errorText(e)),
    )
  }, [])

  const retryAdmin = useCallback(() => {
    setAdmin(null)
    setAdminError(null)
    reloadAdmin()
  }, [reloadAdmin])

  useEffect(() => {
    if (!available || !isAdmin) {
      setAdmin(null)
      return
    }
    reloadAdmin()
    return onNotifyPrefsChanged(primaryScope(), reloadAdmin)
  }, [available, isAdmin, reloadAdmin])

  const run = useCallback(
    async (key: string, write: () => Promise<void>): Promise<void> => {
      setBusy(true)
      try {
        await write()
        saved(key)
      } catch (e) {
        failed(key, e)
      } finally {
        setBusy(false)
      }
    },
    [saved, failed],
  )

  const mine = useCallback(
    (key: string, write: () => Promise<NotifyPrefs>) =>
      run(key, async () => {
        adopt(await write())
      }),
    [run, adopt],
  )

  const admins = useCallback(
    (key: string, write: () => Promise<void>) =>
      run(key, async () => {
        await write()
        reloadAdmin()
      }),
    [run, reloadAdmin],
  )

  const workspaceNames = useMemo(() => {
    const names = new Map<string, string>()
    for (const w of prefs?.workspaces ?? []) names.set(w.projectId, w.name)
    for (const d of admin?.defaults ?? []) if (!names.has(d.projectId)) names.set(d.projectId, d.name)
    return names
  }, [prefs, admin])

  if (!available) {
    return (
      <div className={PAGE} data-testid="notifications-settings-unavailable">
        <Header />
        <SettingsLoadState kind="empty" title="This server doesn’t send pushes yet">
          Update {PRODUCT_NAME} on this server to 0.45.5 or newer, then come back here to choose your notifications.
        </SettingsLoadState>
      </div>
    )
  }

  const hint = ownerHint(caller, prefs, admin)
  const me = prefs?.principal ?? ''

  const mineState =
    loadError && !prefs ? (
      <SettingsLoadState
        kind="error"
        title="Couldn’t load your notifications"
        detail={loadError}
        action={<Button onClick={reload}>Try again</Button>}
        testId="notifications-load-error"
      >
        Your phone and email settings are unchanged; this page just couldn’t read them.
      </SettingsLoadState>
    ) : !prefs ? (
      <SettingsLoadState kind="loading" title="Loading your notifications…" testId="notifications-loading" />
    ) : undefined

  const status = prefs ? phoneStatus(prefs) : null

  return (
    <div className={PAGE} data-testid="notifications-settings">
      <Header />

      <SettingsGroup title="Your phone" card state={mineState} testId="notifications-phone">
        {prefs && status && (
          <>
            <div className="px-4 py-3" data-testid="notifications-status" data-tone={status.tone}>
              <div className="flex items-start justify-between gap-6">
                <div className="min-w-0">
                  <div className={`${SETTING_ROW_LABEL} flex items-center gap-2`}>
                    <StatusDot tone={status.tone} />
                    <span data-testid="notifications-status-title">{status.title}</span>
                  </div>
                  <div>
                    <div className={SETTING_ROW_HINT}>{status.hint}</div>
                    {prefs.push === 'dormant' && prefs.pushReason && (
                      <p className={SETTING_SERVER_REASON}>The server said: {prefs.pushReason.replace(/[.\s]+$/, '')}.</p>
                    )}
                  </div>
                </div>
                {prefs.push !== 'airgap' && (
                  <Button variant={status.pair ? 'accent' : 'quiet'} onClick={openCompanion} className="flex-shrink-0">
                    {status.pair ? 'Pair a phone' : 'Manage phones'}
                  </Button>
                )}
              </div>
            </div>
            <SwitchRow
              settingId="notifications.server-bell"
              testId="notifications-server-bell"
              label="Push to my phones"
              hint={
                <>
                  Off silences every push from this server: Threads, tickets and projects.
                  {hint?.kind === 'aliases' && (
                    <span data-testid="notifications-owner-hint" className="block mt-1">
                      Signing in as {orList(hint.logins)} counts as you here.
                    </span>
                  )}
                  {hint?.kind === 'separate' && (
                    <span data-testid="notifications-owner-hint" className="block mt-1">
                      {hint.owners} logins have the Owner role, so this owner token is its own person here. Each owner
                      sets their own pushes on their phone; see People below.
                    </span>
                  )}
                </>
              }
              state={rows.stateOf('bell')}
            >
              <Toggle
                aria-label="Push to my phones from this server"
                checked={prefs.server.enabled}
                disabled={busy}
                onChange={(next) => void mine('bell', () => setMyServerBell(next))}
              />
            </SwitchRow>
          </>
        )}
      </SettingsGroup>

      {prefs && (
        <div data-settings-id="notifications.workspaces">
          <SettingsGroup
            title="Pushes by workspace"
            hint="Which agents may push your phone when they post in a Thread."
            card
            testId="notifications-workspaces"
            state={
              prefs.workspaces.length === 0 ? (
                <SettingsLoadState kind="empty" title="No workspaces on this server yet" testId="notifications-ws-empty">
                  When an agent joins this server, it shows here and you choose whether it may push your phone.
                </SettingsLoadState>
              ) : undefined
            }
          >
            {prefs.workspaces.map((w) => (
              <SwitchRow
                key={w.projectId}
                testId={`notifications-ws-${w.projectId}`}
                leading={<WorkspaceAvatar projectId={w.projectId} name={w.name} />}
                label={w.name}
                hint={<span data-testid="notifications-ws-source">{sourceLabel(w, me)}</span>}
                state={rows.stateOf(`ws:${w.projectId}`)}
              >
                {w.source === 'user' && (
                  <ResetButton
                    label={`Reset ${w.name} to the workspace default`}
                    disabled={busy}
                    onClick={() => void mine(`ws:${w.projectId}`, () => setMyWorkspacePush(w.projectId, null))}
                  />
                )}
                <Toggle
                  aria-label={`Thread pushes from ${w.name}`}
                  checked={w.threadPush}
                  disabled={busy}
                  onChange={(next) => void mine(`ws:${w.projectId}`, () => setMyWorkspacePush(w.projectId, next))}
                />
              </SwitchRow>
            ))}
          </SettingsGroup>
        </div>
      )}

      {/* Agent email notices (Email Notifications part 2, N10): this
          person's own address and per-workspace switches. Hides itself
          (title too) on a server without them. */}
      <EmailNoticePrefs
        scope={primaryScope()}
        workspaces={(prefs?.workspaces ?? []).map((w) => ({ projectId: w.projectId, name: w.name }))}
        avatar={(w) => <WorkspaceAvatar projectId={w.projectId} name={w.name} />}
      />

      {isAdmin && (
        <p className="type-hint text-[var(--color-text-secondary)] leading-relaxed border-t border-[var(--color-border)] pt-5" data-testid="notifications-everyone">
          The settings below are for everyone on this server. Only the owner and admins see them.
        </p>
      )}

      {/* Hides itself (title too) on a server without the key. */}
      {isAdmin && <PushPreviewsRow />}

      {isAdmin && (
        <div data-testid="notifications-defaults" data-settings-id="notifications.defaults">
          <SettingsGroup
            title="Workspace defaults"
            hint="What people get from each workspace until they choose for themselves. A person’s own switch wins."
            card
            state={
              adminError ? (
                <SettingsLoadState
                  kind="error"
                  title="Couldn’t load everyone’s notifications"
                  detail={adminError}
                  action={<Button onClick={retryAdmin}>Try again</Button>}
                  testId="notifications-admin-error"
                >
                  Nobody’s settings changed; this page just couldn’t read them.
                </SettingsLoadState>
              ) : !admin ? (
                <SettingsLoadState kind="loading" title="Loading workspace defaults…" />
              ) : admin.defaults.length === 0 ? (
                <SettingsLoadState kind="empty" title="No workspaces on this server yet">
                  Each workspace gets a default here once it joins this server.
                </SettingsLoadState>
              ) : undefined
            }
          >
            {admin?.defaults.map((d) => (
              <SwitchRow
                key={d.projectId}
                testId={`notifications-default-${d.projectId}`}
                leading={<WorkspaceAvatar projectId={d.projectId} name={d.name} />}
                label={d.name}
                hint={d.source === 'default' ? 'Set for this workspace' : 'Not set, so on'}
                state={rows.stateOf(`def:${d.projectId}`)}
              >
                {d.source === 'default' && (
                  <ResetButton
                    label={`Clear the default for ${d.name}`}
                    disabled={busy}
                    onClick={() => void admins(`def:${d.projectId}`, () => setWorkspaceDefault(d.projectId, null))}
                  />
                )}
                <Toggle
                  aria-label={`New and unset people get pushes from ${d.name}`}
                  checked={d.threadPush}
                  disabled={busy}
                  onChange={(next) => void admins(`def:${d.projectId}`, () => setWorkspaceDefault(d.projectId, next))}
                />
              </SwitchRow>
            ))}
          </SettingsGroup>
        </div>
      )}

      {isAdmin && !adminError && (
        <div data-testid="notifications-people" data-settings-id="notifications.people">
          <SettingsGroup
            title="People"
            hint={
              canEditPeople ? (
                'Everyone who signs in to this server. Open a person to see or change their pushes.'
              ) : (
                <span data-testid="notifications-people-readonly">
                  Everyone who signs in to this server. Only the owner can change other people’s notifications.
                </span>
              )
            }
            card
            state={
              !admin ? (
                <SettingsLoadState kind="loading" title="Loading people…" />
              ) : admin.people.length === 0 ? (
                <SettingsLoadState kind="empty" title="No one else signs in here yet">
                  People you add in User Access show here with their push settings.
                </SettingsLoadState>
              ) : undefined
            }
          >
            {admin?.people.map((p) => (
              <PersonRow
                key={p.principal}
                person={p}
                me={me}
                open={openPerson === p.principal}
                onToggleOpen={() => setOpenPerson((cur) => (cur === p.principal ? null : p.principal))}
                editable={canEditPeople && p.principal !== me}
                busy={busy}
                workspaceNames={workspaceNames}
                stateOf={rows.stateOf}
                onBell={(next) => void admins(`person:${p.principal}`, () => setPersonServerBell(p.principal, next))}
                onWorkspace={(projectId, next) =>
                  void admins(`person:${p.principal}:${projectId}`, () =>
                    setPersonWorkspacePush(p.principal, projectId, next),
                  )
                }
              />
            ))}
          </SettingsGroup>
        </div>
      )}
    </div>
  )
}

/** The collapsed row's one-line summary: "Muted" when the person's server
 *  bell is off, else "Pushes on · 3 of 7 workspaces". */
export function personSummary(person: Pick<NotifyPerson, 'server' | 'workspaces'>): string {
  if (!person.server.enabled) return 'Muted'
  const total = person.workspaces.length
  if (total === 0) return 'Pushes on'
  const on = person.workspaces.filter((w) => w.threadPush).length
  return `Pushes on · ${on} of ${total} workspace${total === 1 ? '' : 's'}`
}

function PersonRow({
  person,
  me,
  open,
  onToggleOpen,
  editable,
  busy,
  workspaceNames,
  stateOf,
  onBell,
  onWorkspace,
}: {
  person: NotifyPerson
  me: string
  /** Accordion state (local view state, owned by the page). */
  open: boolean
  onToggleOpen: () => void
  editable: boolean
  busy: boolean
  workspaceNames: Map<string, string>
  stateOf: (key: string) => SaveState
  onBell: (next: boolean) => void
  onWorkspace: (projectId: string, next: boolean | null) => void
}): React.JSX.Element {
  const isMe = person.principal === me
  const roleText = person.role ? person.role.charAt(0).toUpperCase() + person.role.slice(1) : 'No role'
  const panelId = `notifications-person-panel-${person.principal}`
  return (
    <div data-testid={`notifications-person-${person.principal}`} data-disabled={person.disabled || undefined}>
      <button
        type="button"
        data-testid={`notifications-person-${person.principal}-header`}
        aria-expanded={open}
        aria-controls={panelId}
        onClick={onToggleOpen}
        data-k2-focus=""
        className={`relative w-full flex items-center justify-between gap-3 px-4 py-3 text-left no-drag cursor-pointer outline-none ${open ? SETTING_LIST_ROW_BAND : SETTING_LIST_ROW_HOVER}`}
      >
        <div className="min-w-0 flex items-center gap-2">
          <span
            className={`type-name truncate ${person.disabled ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-primary)]'}`}
          >
            {person.principal}
            {isMe ? ' (you)' : ''}
          </span>
          <span className="type-hint text-[var(--color-text-secondary)] flex-shrink-0">
            {roleText}
            {person.disabled ? ' · Disabled' : ''}
          </span>
        </div>
        <div className="flex-shrink-0 flex items-center gap-2">
          <span data-testid="notifications-summary" className="type-hint text-[var(--color-text-secondary)]">
            {personSummary(person)}
          </span>
          <svg
            width="12"
            height="12"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
            className="flex-shrink-0 text-[var(--color-text-secondary)]"
          >
            <path d={open ? 'M18 15l-6-6-6 6' : 'M6 9l6 6 6-6'} />
          </svg>
        </div>
      </button>
      {open && (
        <div
          id={panelId}
          role="region"
          aria-label={`Notifications for ${person.principal}`}
          className="border-t border-[var(--color-border)] divide-y divide-[var(--color-border)] pl-8"
        >
          <PanelRow
            label="Pushes from this server"
            hint={deviceCountText(person.devices)}
            state={stateOf(`person:${person.principal}`)}
          >
            {editable ? (
              <Toggle
                aria-label={`Pushes from this server for ${person.principal}`}
                checked={person.server.enabled}
                disabled={busy}
                onChange={onBell}
              />
            ) : (
              <span className="type-row text-[var(--color-text-secondary)]">
                {person.server.enabled ? 'On' : 'Off'}
              </span>
            )}
          </PanelRow>
          {person.workspaces.map((w: NotifyWorkspacePref) => {
            const name = workspaceNames.get(w.projectId) ?? w.name
            return (
              <PanelRow
                key={w.projectId}
                testId={`notifications-person-${person.principal}-ws-${w.projectId}`}
                label={name}
                hint={sourceLabel(w, me)}
                state={stateOf(`person:${person.principal}:${w.projectId}`)}
              >
                {editable ? (
                  <>
                    {w.source === 'user' && (
                      <ResetButton
                        label={`Reset ${name} for ${person.principal}`}
                        disabled={busy}
                        onClick={() => onWorkspace(w.projectId, null)}
                      />
                    )}
                    <Toggle
                      aria-label={`Thread pushes from ${name} for ${person.principal}`}
                      checked={w.threadPush}
                      disabled={busy}
                      onChange={(next) => onWorkspace(w.projectId, next)}
                    />
                  </>
                ) : (
                  <span className="type-row text-[var(--color-text-secondary)]">{w.threadPush ? 'On' : 'Off'}</span>
                )}
              </PanelRow>
            )
          })}
          {isMe && (
            <p className={`${SETTING_ROW_HINT} px-4 py-3`}>Change yours under Your phone and Pushes by workspace.</p>
          )}
        </div>
      )}
    </div>
  )
}

/** A row inside an opened person: indented under their name. */
function PanelRow({
  label,
  hint,
  state,
  testId,
  children,
}: {
  label: string
  hint: string
  state: SaveState
  testId?: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div className="flex items-start justify-between gap-6 pr-4 py-2.5" data-testid={testId}>
      <div className="min-w-0">
        <div className={`${SETTING_ROW_LABEL} truncate`}>{label}</div>
        <div className={SETTING_ROW_HINT}>{hint}</div>
        <SaveError state={state} />
      </div>
      <div className="flex items-center gap-2 flex-shrink-0">
        <SaveNote state={state} />
        {children}
      </div>
    </div>
  )
}
