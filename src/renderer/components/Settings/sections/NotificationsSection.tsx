// Settings → Notifications (prd-thread-push-notifications-v1 §9, slice S5).
//
// Phone pushes when an agent posts in a Thread. The daemon owns every value
// (`/cli/notify/*`); this page renders it and sends the person's clicks.
// Three blocks:
//
//   1. Your notifications (every login): the server bell, then one switch
//      per workspace with where its value came from ("Default (on)",
//      "Set by you", "Set by gina" + Reset).
//   2. Workspace defaults (Owner/Admin): what new and unset people get.
//   3. People (Owner/Admin): everyone's settings. Admins see them; only the
//      owner changes other people's switches (Rosson 2026-10-11).
//
// Hidden entirely unless the window's server reports `thread-push-v1`.
// Refetches on `notify_prefs_changed` (no payload).

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { SettingsGroup } from '../controls/SettingControls'
import { Toggle } from '@/components/ui'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { useProjectsStore } from '@/stores/projects'
import { primaryScope } from '@/kessel/server-scope'
import { onNotifyPrefsChanged } from '@/stores/session-events'
import {
  deviceCountText,
  fetchNotifyAdmin,
  fetchNotifyCaller,
  pushStatusText,
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

export const NOTIFICATIONS_MANIFEST: SettingEntry[] = [
  {
    id: 'notifications.server-bell',
    section: 'notifications',
    label: 'Push to My Phones From This Server',
    description: 'Phone notifications from this server: Thread, ticket and project pushes',
    keywords: ['notification', 'notifications', 'push', 'phone', 'companion', 'bell', 'mute', 'quiet'],
    group: 'Your notifications',
  },
  {
    id: 'notifications.workspaces',
    section: 'notifications',
    label: 'Thread Pushes per Workspace',
    description: 'Which agents may push your phone when they post in a Thread',
    keywords: ['notification', 'push', 'phone', 'thread', 'workspace', 'agent', 'mute'],
    group: 'Your notifications',
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

function Row({
  settingId,
  title,
  hint,
  leading,
  children,
  testId,
}: {
  settingId?: string
  title: React.ReactNode
  hint?: React.ReactNode
  leading?: React.ReactNode
  children: React.ReactNode
  testId?: string
}): React.JSX.Element {
  return (
    <div
      className="flex items-center justify-between gap-4 py-2 border-b border-[var(--color-border)]"
      data-settings-id={settingId}
      data-testid={testId}
    >
      <div className="min-w-0 flex items-center gap-2">
        {leading}
        <div className="min-w-0">
          <div className="text-xs text-[var(--color-text-primary)] truncate">{title}</div>
          {hint ? <div className="text-[10px] text-[var(--color-text-muted)]">{hint}</div> : null}
        </div>
      </div>
      <div className="flex-shrink-0 flex items-center gap-2">{children}</div>
    </div>
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
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      className="text-[10px] px-1.5 py-0.5 border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
    >
      Reset
    </button>
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

export function NotificationsSection(): React.JSX.Element {
  const available = threadPushAvailable()
  const { prefs, error: loadError, adopt } = useNotifyPrefs(available)
  const [caller, setCaller] = useState<NotifyCaller | null>(null)
  const [admin, setAdmin] = useState<NotifyAdmin | null>(null)
  const [adminError, setAdminError] = useState<string | null>(null)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

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
        // No role known: the page stays on "Your notifications" only.
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

  useEffect(() => {
    if (!available || !isAdmin) {
      setAdmin(null)
      return
    }
    reloadAdmin()
    return onNotifyPrefsChanged(primaryScope(), reloadAdmin)
  }, [available, isAdmin, reloadAdmin])

  const run = useCallback(async (write: () => Promise<void>): Promise<void> => {
    setBusy(true)
    setSaveError(null)
    try {
      await write()
    } catch (e) {
      setSaveError(errorText(e))
    } finally {
      setBusy(false)
    }
  }, [])

  const mine = useCallback(
    (write: () => Promise<NotifyPrefs>) =>
      run(async () => {
        adopt(await write())
      }),
    [run, adopt],
  )

  const admins = useCallback(
    (write: () => Promise<void>) =>
      run(async () => {
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
      <div className="max-w-xl" data-testid="notifications-settings-unavailable">
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">Notifications</h2>
        <p className="text-xs text-[var(--color-text-muted)]">
          This server does not send Thread pushes yet. Update it to 0.45.5 or newer.
        </p>
      </div>
    )
  }

  const hint = ownerHint(caller, prefs, admin)
  const me = prefs?.principal ?? ''

  return (
    <div className="max-w-xl" data-testid="notifications-settings">
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">Notifications</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-2">
        Phone pushes from K2 Companion when an agent posts in a Thread. A push shows the agent’s name only, and
        opens that Thread when tapped. Reading a Thread here keeps its pushes quiet.
      </p>
      {prefs && (
        <p data-testid="notifications-status" className="text-[11px] text-[var(--color-text-secondary)] mb-6">
          Push: {pushStatusText(prefs.push)} · {deviceCountText(prefs.devices)}
        </p>
      )}

      <div className="space-y-6">
        <SettingsGroup title="Your notifications">
          {loadError && (
            <p role="alert" data-testid="notifications-load-error" className="text-[11px] text-[var(--color-status-error)] py-2">
              Could not load your notifications: {loadError}
            </p>
          )}
          {!prefs && !loadError && <p className="text-[11px] text-[var(--color-text-muted)] py-2">Loading…</p>}
          {prefs && (
            <>
              <Row
                settingId="notifications.server-bell"
                testId="notifications-server-bell"
                title="Push to my phones from this server"
                hint={`${deviceCountText(prefs.devices)}. Off silences every push from this server: Threads, tickets and projects.`}
              >
                <Toggle
                  aria-label="Push to my phones from this server"
                  checked={prefs.server.enabled}
                  disabled={busy}
                  onChange={(next) => void mine(() => setMyServerBell(next))}
                />
              </Row>
              {hint?.kind === 'aliases' && (
                <p data-testid="notifications-owner-hint" className="text-[10px] text-[var(--color-text-muted)] py-1">
                  Also you: <strong>{hint.logins.join(', ')}</strong>
                </p>
              )}
              {hint?.kind === 'separate' && (
                <p data-testid="notifications-owner-hint" className="text-[10px] text-[var(--color-text-muted)] py-1">
                  {hint.owners} logins have the Owner role, so this owner token is its own person here. Each owner sets
                  their own pushes on their phone; see People below.
                </p>
              )}
              <div data-settings-id="notifications.workspaces">
                {prefs.workspaces.length === 0 && (
                  <p className="text-[11px] text-[var(--color-text-muted)] py-2">No workspaces on this server.</p>
                )}
                {prefs.workspaces.map((w) => (
                  <Row
                    key={w.projectId}
                    testId={`notifications-ws-${w.projectId}`}
                    leading={<WorkspaceAvatar projectId={w.projectId} name={w.name} />}
                    title={w.name}
                    hint={<span data-testid="notifications-ws-source">{sourceLabel(w, me)}</span>}
                  >
                    {w.source === 'user' && (
                      <ResetButton
                        label={`Reset ${w.name} to the workspace default`}
                        disabled={busy}
                        onClick={() => void mine(() => setMyWorkspacePush(w.projectId, null))}
                      />
                    )}
                    <Toggle
                      aria-label={`Thread pushes from ${w.name}`}
                      checked={w.threadPush}
                      disabled={busy}
                      onChange={(next) => void mine(() => setMyWorkspacePush(w.projectId, next))}
                    />
                  </Row>
                ))}
              </div>
            </>
          )}
        </SettingsGroup>

        {isAdmin && (
          <SettingsGroup title="Workspace defaults">
            <div data-testid="notifications-defaults" data-settings-id="notifications.defaults">
              <p className="text-[10px] text-[var(--color-text-muted)] py-1">
                What new and unset people get for each workspace. A person’s own switch wins.
              </p>
              {adminError && (
                <p role="alert" className="text-[11px] text-[var(--color-status-error)] py-2">
                  Could not load everyone’s notifications: {adminError}
                </p>
              )}
              {admin?.defaults.map((d) => (
                <Row
                  key={d.projectId}
                  testId={`notifications-default-${d.projectId}`}
                  leading={<WorkspaceAvatar projectId={d.projectId} name={d.name} />}
                  title={d.name}
                  hint={d.source === 'default' ? 'Set for this workspace' : 'Not set (on)'}
                >
                  {d.source === 'default' && (
                    <ResetButton
                      label={`Clear the default for ${d.name}`}
                      disabled={busy}
                      onClick={() => void admins(() => setWorkspaceDefault(d.projectId, null))}
                    />
                  )}
                  <Toggle
                    aria-label={`New and unset people get pushes from ${d.name}`}
                    checked={d.threadPush}
                    disabled={busy}
                    onChange={(next) => void admins(() => setWorkspaceDefault(d.projectId, next))}
                  />
                </Row>
              ))}
            </div>
          </SettingsGroup>
        )}

        {isAdmin && (
          <SettingsGroup title="People">
            <div data-testid="notifications-people" data-settings-id="notifications.people" className="space-y-3">
              {!canEditPeople && (
                <p data-testid="notifications-people-readonly" className="text-[10px] text-[var(--color-text-muted)] py-1">
                  Only the owner can change other people’s notifications.
                </p>
              )}
              {admin?.people.map((p) => (
                <PersonCard
                  key={p.principal}
                  person={p}
                  me={me}
                  editable={canEditPeople && p.principal !== me}
                  busy={busy}
                  workspaceNames={workspaceNames}
                  onBell={(next) => void admins(() => setPersonServerBell(p.principal, next))}
                  onWorkspace={(projectId, next) =>
                    void admins(() => setPersonWorkspacePush(p.principal, projectId, next))
                  }
                />
              ))}
            </div>
          </SettingsGroup>
        )}

        {saveError && (
          <p role="alert" data-testid="notifications-save-error" className="text-[11px] text-[var(--color-status-error)]">
            {saveError}
          </p>
        )}
      </div>
    </div>
  )
}

function PersonCard({
  person,
  me,
  editable,
  busy,
  workspaceNames,
  onBell,
  onWorkspace,
}: {
  person: NotifyPerson
  me: string
  editable: boolean
  busy: boolean
  workspaceNames: Map<string, string>
  onBell: (next: boolean) => void
  onWorkspace: (projectId: string, next: boolean | null) => void
}): React.JSX.Element {
  const isMe = person.principal === me
  const roleText = person.role ? person.role.charAt(0).toUpperCase() + person.role.slice(1) : 'Unknown role'
  return (
    <div
      data-testid={`notifications-person-${person.principal}`}
      className={`border border-[var(--color-border)] p-2 ${person.disabled ? 'opacity-50' : ''}`}
    >
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="text-xs text-[var(--color-text-primary)] truncate">
            {person.principal}
            {isMe ? ' (you)' : ''}
          </div>
          <div className="text-[10px] text-[var(--color-text-muted)]">
            {roleText}
            {person.disabled ? ' · Disabled' : ''} · {deviceCountText(person.devices)}
          </div>
        </div>
        {editable ? (
          <Toggle
            aria-label={`Pushes from this server for ${person.principal}`}
            checked={person.server.enabled}
            disabled={busy}
            onChange={onBell}
          />
        ) : (
          <span className="text-[10px] text-[var(--color-text-secondary)]">
            Server: {person.server.enabled ? 'on' : 'off'}
          </span>
        )}
      </div>
      {person.workspaces.length > 0 && (
        <div className="mt-2 grid grid-cols-2 gap-x-4">
          {person.workspaces.map((w: NotifyWorkspacePref) => {
            const name = workspaceNames.get(w.projectId) ?? w.name
            return (
              <div
                key={w.projectId}
                data-testid={`notifications-person-${person.principal}-ws-${w.projectId}`}
                className="flex items-center justify-between gap-2 py-1"
              >
                <div className="min-w-0">
                  <div className="text-[11px] text-[var(--color-text-secondary)] truncate">{name}</div>
                  <div className="text-[9px] text-[var(--color-text-muted)]">{sourceLabel(w, me)}</div>
                </div>
                {editable ? (
                  <div className="flex items-center gap-1">
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
                  </div>
                ) : (
                  <span className="text-[10px] text-[var(--color-text-secondary)]">{w.threadPush ? 'on' : 'off'}</span>
                )}
              </div>
            )
          })}
        </div>
      )}
      {isMe && <div className="text-[9px] text-[var(--color-text-muted)] mt-1">Change yours above.</div>}
    </div>
  )
}
