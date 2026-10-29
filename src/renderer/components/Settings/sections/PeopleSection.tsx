// Settings → Access → User Access. Principals (username + full name)
// plus that person's grants. Access templates live under User Templates.
// Not Admin Access (Connect users) and not Cores → Apps.

import React, { useCallback, useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { SettingDropdown, SettingRow, SettingsGroup } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import { primaryScope } from '@/kessel/server-scope'
import { AccessAuditLink } from '../shared/AccessAuditLink'
import { SewPersonAccess } from './SewAccessParts'
import type { ChatGrant } from './sew-api'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { daemonErrorText } from '@/lib/daemon-error'
import {
  AddUserDialog,
  AddedUserDialog,
  ResetPasswordDialog,
  SetPasswordDialog,
  type AddedUser,
} from './AppUserDialogs'

export const PEOPLE_MANIFEST: SettingEntry[] = [
  {
    id: 'people.roster',
    section: 'people',
    label: 'User Access',
    description: 'App users on this box — username and full name. Not Admin Access.',
    keywords: ['people', 'user access', 'full name', 'principal', 'guest', 'username', 'grant', 'password', 'app'],
    group: 'People',
  },
]

export type PersonRow = {
  id: string | null
  username: string
  fullName: string | null
  roleId: string | null
  roleName: string | null
  defaultRooms: string[]
  defaultRoomHandles: string[]
  email: string | null
  /** An agent tie (agent-app-users R2): the workspace handle it holds them to. */
  tiedTo: string | null
  /** Tied, and given more than that workspace: the extra waits for a reset. */
  pendingReset: boolean
  /** Why the tie exists: created | password | email | invite | app_key | app_reset. */
  tiedReasons: string[]
  /** Why an owner reset won't mail the stored email (any tie, or unconfirmed). */
  storedEmailRefused: string | null
  /** The login is locked (wrong passwords) until then, epoch seconds. */
  lockedUntil: number | null
}

type RoomAccess = { handle: string; caps: string[] }

type RoleRow = {
  id: string
  name: string
  appId: string | null
  roomAccess: RoomAccess[]
}

type GrantRow = {
  id: string
  subjectKind: string
  subjectId: string
  kind: string
  targetId: string
  roleId: string | null
  scope: string | null
  enabled: boolean
  /** Sew S27 §17.1: hooked (chat) grants carry the sync state. */
  hookState?: string
  hookError?: string
}

type AppOption = { id: string; name: string; kind: string }

const KIND_OPTIONS = [
  { value: 'app', label: 'app' },
  { value: 'database', label: 'database' },
  { value: 'cli', label: 'cli' },
]

const INPUT_CLS =
  'flex-1 min-w-[8rem] px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

const BTN_ACCENT =
  'px-3 py-1 text-[11px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60'

function asRecord(raw: unknown): Record<string, unknown> {
  return raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
}

function asString(v: unknown): string | null {
  return typeof v === 'string' && v.trim() ? v.trim() : null
}

function asList(raw: unknown, keys: string[]): unknown[] {
  if (Array.isArray(raw)) return raw
  const rec = asRecord(raw)
  for (const k of keys) {
    if (Array.isArray(rec[k])) return rec[k] as unknown[]
  }
  return []
}

function parseStringList(raw: unknown): string[] {
  if (!Array.isArray(raw)) return []
  return raw.filter((c): c is string => typeof c === 'string' && Boolean(c.trim())).map((s) => s.trim())
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

export function parsePeople(raw: unknown): PersonRow[] {
  const list = asRecord(raw).users
  if (!Array.isArray(list)) return []
  return list.flatMap((row) => {
    const rec = asRecord(row)
    const username = asString(rec.username)
    if (!username) return []
    return [{
      id: asString(rec.id),
      username,
      fullName: asString(rec.fullName) ?? asString(rec.full_name),
      roleId: asString(rec.roleId) ?? asString(rec.role_id),
      roleName: asString(rec.roleName) ?? asString(rec.role_name),
      defaultRooms: parseStringList(rec.defaultRooms ?? rec.default_rooms),
      defaultRoomHandles: parseStringList(rec.defaultRoomHandles ?? rec.default_room_handles),
      email: asString(rec.email),
      tiedTo: asString(rec.tiedTo),
      pendingReset: rec.pendingReset === true,
      tiedReasons: parseStringList(rec.tiedReasons),
      storedEmailRefused: asString(rec.storedEmailRefused),
      lockedUntil: typeof rec.lockedUntil === 'number' ? rec.lockedUntil : null,
    }]
  })
}

function parseRoles(raw: unknown): RoleRow[] {
  return asList(raw, ['roles']).flatMap((row) => {
    const rec = asRecord(row)
    const id = asString(rec.id)
    const name = asString(rec.name)
    if (!id || !name) return []
    const accessRaw = rec.roomAccess ?? rec.room_access
    const roomAccess = Array.isArray(accessRaw)
      ? accessRaw.flatMap((item) => {
          const room = asRecord(item)
          const handle = asString(room.handle)
          if (!handle) return []
          return [{ handle, caps: parseStringList(room.caps) }]
        })
      : []
    return [{
      id,
      name,
      appId: asString(rec.appId) ?? asString(rec.app_id),
      roomAccess,
    }]
  })
}

function parseGrants(raw: unknown): GrantRow[] {
  return asList(raw, ['grants']).flatMap((row) => {
    const rec = asRecord(row)
    const id = asString(rec.id)
    const subjectKind = asString(rec.subjectKind) ?? asString(rec.subject_kind)
    const subjectId = asString(rec.subjectId) ?? asString(rec.subject_id)
    const kind = asString(rec.kind)
    const targetId = asString(rec.targetId) ?? asString(rec.target_id)
    if (!id || !subjectKind || !subjectId || !kind || !targetId) return []
    const enabled = rec.enabled
    return [{
      id,
      subjectKind,
      subjectId,
      kind,
      targetId,
      roleId: asString(rec.roleId) ?? asString(rec.role_id),
      scope: asString(rec.scope),
      enabled: typeof enabled === 'boolean' ? enabled : enabled !== 0,
      ...(asString(rec.hookState) ? { hookState: asString(rec.hookState) as string } : {}),
      ...(asString(rec.hookError) ? { hookError: asString(rec.hookError) as string } : {}),
    }]
  })
}

function parseApps(raw: unknown): AppOption[] {
  return asList(raw, ['services']).flatMap((row) => {
    const rec = asRecord(row)
    const id = asString(rec.id)
    const name = asString(rec.name)
    const kind = asString(rec.kind)
    if (!id || !name || (kind !== 'cmd' && kind !== 'skin')) return []
    return [{ id, name, kind }]
  })
}

function AccessSwitch({
  on,
  label,
  disabled,
  onToggle,
}: {
  on: boolean
  label: string
  disabled?: boolean
  onToggle: () => void
}): React.JSX.Element {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      disabled={disabled}
      onClick={onToggle}
      className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-40 ${
        on ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
      }`}
    >
      <span
        className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
          on ? 'translate-x-3.5' : 'translate-x-0.5'
        }`}
      />
    </button>
  )
}

function listButtonClass(active: boolean): string {
  return `w-full text-left px-3 py-2 no-drag cursor-pointer ${
    active ? 'bg-[var(--color-accent)]/15' : 'hover:bg-white/[0.03]'
  }`
}

export function PeopleSection(): React.JSX.Element {
  const [people, setPeople] = useState<PersonRow[]>([])
  const [roles, setRoles] = useState<RoleRow[]>([])
  const [grants, setGrants] = useState<GrantRow[]>([])
  const [apps, setApps] = useState<AppOption[]>([])
  const [appsLoaded, setAppsLoaded] = useState(false)
  const [selection, setSelection] = useState<string | null>(null)
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [expandedGrant, setExpandedGrant] = useState<string | null>(null)
  const [deleteConfirm, setDeleteConfirm] = useState<string | null>(null)
  const [grantKind, setGrantKind] = useState('')
  const [grantRole, setGrantRole] = useState('')
  const [grantApp, setGrantApp] = useState('')
  const [grantTarget, setGrantTarget] = useState('')
  const [grantScope, setGrantScope] = useState('')
  // Owner only (0.46.6): the daemon lets only the Owner manage App users,
  // so nobody else sees actions that would fail.
  const [isOwner, setIsOwner] = useState(false)
  const [menuFor, setMenuFor] = useState<string | null>(null)
  const [dialog, setDialog] = useState<{ kind: 'reset' | 'set'; username: string } | null>(null)
  const [adding, setAdding] = useState(false)
  // The one-time result of Add user; dropped when its sheet closes.
  const [added, setAdded] = useState<AddedUser | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  useEffect(() => {
    let live = true
    void fetchViewerRole().then((role) => {
      if (live) setIsOwner(role === 'owner')
    })
    return () => {
      live = false
    }
  }, [])

  const refresh = useCallback(async () => {
    setError(null)
    const failures: string[] = []
    try {
      setPeople(parsePeople(await daemonCliGet<unknown>(primaryScope(), 'skin/users')))
    } catch (e) {
      failures.push(errText(e))
      setPeople([])
    }
    try {
      setGrants(parseGrants(await daemonCliGet<unknown>(primaryScope(), 'skin/grants')))
    } catch (e) {
      failures.push(errText(e))
      setGrants([])
    }
    try {
      setRoles(parseRoles(await daemonCliGet<unknown>(primaryScope(), 'skin/roles')))
    } catch (e) {
      failures.push(errText(e))
      setRoles([])
    }
    if (failures.length) setError(failures.join(' · '))
    setLoading(false)
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const loadApps = useCallback(async () => {
    if (appsLoaded) return
    const projects = await daemonCliGet<unknown>(primaryScope(), 'projects/list')
    const list = Array.isArray(projects) ? projects : asList(projects, ['projects', 'items'])
    const workspaces = list.flatMap((row) => {
      const rec = asRecord(row)
      const id = asString(rec.id)
      if (!id) return []
      return [id]
    })
    const next: AppOption[] = []
    for (const projectId of workspaces) {
      const listed = await daemonCliGet<unknown>(primaryScope(), 'publish/list', { project: projectId })
      next.push(...parseApps(listed))
    }
    setApps(next)
    setAppsLoaded(true)
  }, [appsLoaded])

  useEffect(() => {
    if (grantKind !== 'app') return
    void loadApps().catch((e) => setError(errText(e)))
  }, [grantKind, loadApps])

  const person = selection ? people.find((p) => p.username === selection) ?? null : null

  const saveName = useCallback(
    async (username: string, fullName: string) => {
      setBusy(username)
      setError(null)
      try {
        await daemonCliPost(primaryScope(), 'skin/users/full-name', { username, fullName })
        setDrafts((prev) => {
          const next = { ...prev }
          delete next[username]
          return next
        })
        await refresh()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusy(null)
      }
    },
    [refresh],
  )

  const setEnabled = useCallback(
    async (grant: GrantRow, enabled: boolean) => {
      setBusy(grant.id)
      setError(null)
      try {
        await daemonCliPost(primaryScope(), 'skin/grants/enabled', { id: grant.id, enabled })
        await refresh()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusy(null)
      }
    },
    [refresh],
  )

  const setHost = useCallback(
    async (username: string, present: boolean) => {
      setBusy(`host:${username}`)
      setError(null)
      try {
        await daemonCliPost(primaryScope(), 'skin/grants/host', { username, present })
        await refresh()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusy(null)
      }
    },
    [refresh],
  )

  const revoke = useCallback(
    async (id: string) => {
      setBusy(id)
      setError(null)
      try {
        await daemonCliPost(primaryScope(), 'skin/grants/delete', { id })
        await refresh()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusy(null)
      }
    },
    [refresh],
  )

  const removeAccount = useCallback(
    async (username: string) => {
      setBusy(`delete:${username}`)
      setError(null)
      try {
        await daemonCliPost(primaryScope(), 'skin/users/remove', { username })
        setDeleteConfirm(null)
        setSelection(null)
        await refresh()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusy(null)
      }
    },
    [refresh],
  )

  const addGrant = useCallback(async () => {
    if (!person?.id) {
      setError('missing subject id')
      return
    }
    setBusy('add-grant')
    setError(null)
    try {
      const targetId = grantKind === 'app' ? grantApp : grantTarget.trim()
      await daemonCliPost(primaryScope(), 'skin/grants', {
        subjectKind: 'principal',
        subjectId: person.id,
        kind: grantKind,
        targetId,
        ...(grantKind === 'app' ? { roleId: grantRole } : { scope: grantScope.trim() }),
      })
      setGrantTarget('')
      setGrantScope('')
      await refresh()
    } catch (e) {
      setError(errText(e))
    } finally {
      setBusy(null)
    }
  }, [person, grantKind, grantApp, grantTarget, grantRole, grantScope, refresh])

  const unlock = useCallback(
    async (username: string) => {
      setBusy(`unlock:${username}`)
      setError(null)
      setNotice(null)
      try {
        const r = asRecord(await daemonCliPost(primaryScope(), 'skin/users/unlock', { username }))
        setNotice(r.cleared === true ? `${username} is unlocked and can sign in again.` : `${username} wasn’t locked.`)
        await refresh()
      } catch (e) {
        setError(daemonErrorText(e))
      } finally {
        setBusy(null)
      }
    },
    [refresh],
  )

  // Chat (Sew) grants have their own row (SewPersonAccess), not the
  // generic list.
  const personGrants = person?.id
    ? grants.filter(
        (g) => g.subjectKind === 'principal' && g.subjectId === person.id && g.kind !== 'mailbox' && g.kind !== 'chat',
      )
    : []
  const sewGrantOf = (id: string | null): ChatGrant | null => {
    if (!id) return null
    const g = grants.find((x) => x.kind === 'chat' && x.subjectKind === 'principal' && x.subjectId === id)
    return g ? { id: g.id, subjectKind: g.subjectKind, subjectId: g.subjectId, scope: g.scope, enabled: g.enabled, hookState: g.hookState, hookError: g.hookError } : null
  }
  const hostGrant = personGrants.find((g) => g.kind === 'app' && g.targetId === 'host')
  const appChoices = apps.map((app) => ({
    value: app.id,
    label: `${app.name} (${app.kind === 'skin' ? 'app' : app.kind})`,
  }))
  const roleChoices = roles.map((role) => ({ value: role.id, label: role.name }))
  const dialogPerson = dialog ? people.find((p) => p.username === dialog.username) ?? null : null
  // Closing a sheet drops whatever it showed once, and re-reads the list
  // (a reset or set can change the lock and the tie).
  const postHere = (route: string, body: Record<string, unknown>): Promise<unknown> =>
    daemonCliPost(primaryScope(), route, body)
  const closeDialog = (): void => {
    setDialog(null)
    void refresh()
  }

  return (
    <div className="flex h-full min-h-0">
      <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col min-h-0">
        <div className="px-3 pt-3 pb-2 border-b border-[var(--color-border)]">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">User Access</h2>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
            Username and full name for app guests on this box. This reads principals.
            It is not Admin Access and not a separate people list.
          </p>
          <AccessAuditLink className="mt-1" />
        </div>
        <div data-settings-id="people.roster" className="flex-1 overflow-y-auto min-h-0">
          <div className="px-3 pt-2 pb-1">
            <span className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
              People
            </span>
          </div>
          {loading ? (
            <p className="px-3 text-[10px] text-[var(--color-text-muted)]">Loading…</p>
          ) : people.length === 0 ? (
            <p className="px-3 text-[10px] text-[var(--color-text-muted)]">No app users yet.</p>
          ) : (
            people.map((row) => (
              <div key={row.username} className="relative flex items-stretch">
                <button
                  type="button"
                  aria-label={`Open ${row.username}`}
                  className={`${listButtonClass(selection === row.username)} min-w-0`}
                  onClick={() => setSelection(row.username)}
                >
                  <span className="block text-xs font-mono text-[var(--color-text-primary)] truncate">
                    {row.username}
                    {row.pendingReset ? (
                      <span className="ml-1.5 text-[9px] font-sans uppercase tracking-wider px-1 py-0.5 bg-[var(--color-status-warning-soft,#b7791f)]/20 text-[var(--color-text-secondary)]">
                        Waiting for reset
                      </span>
                    ) : null}
                    {row.lockedUntil ? (
                      <span className="ml-1.5 text-[9px] font-sans uppercase tracking-wider px-1 py-0.5 bg-[var(--color-status-error)]/15 text-[var(--color-text-secondary)]">
                        Locked
                      </span>
                    ) : null}
                  </span>
                  {row.fullName ? (
                    <span className="block text-[10px] text-[var(--color-text-muted)] truncate">
                      {row.fullName}
                    </span>
                  ) : null}
                  {sewGrantOf(row.id) ? (
                    <span data-testid={`sew-tag-${row.username}`} className="block text-[9px] text-[var(--color-text-muted)] truncate">
                      Sew {sewGrantOf(row.id)?.scope === 'guest' ? 'guest' : 'member'}
                    </span>
                  ) : null}
                </button>
                {isOwner ? (
                  <button
                    type="button"
                    aria-label={`Password actions for ${row.username}`}
                    aria-haspopup="menu"
                    aria-expanded={menuFor === row.username}
                    className="px-2 text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer flex-shrink-0"
                    onClick={() => setMenuFor((m) => (m === row.username ? null : row.username))}
                  >
                    ⋯
                  </button>
                ) : null}
                {isOwner && menuFor === row.username ? (
                  <div
                    role="menu"
                    aria-label={`Password actions for ${row.username}`}
                    className="absolute right-1 top-full z-20 min-w-[10rem] py-1 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] shadow-lg"
                    onKeyDown={(e) => {
                      if (e.key === 'Escape') setMenuFor(null)
                    }}
                  >
                    <button
                      type="button"
                      role="menuitem"
                      className="block w-full text-left px-3 py-1 text-[11px] text-[var(--color-text-primary)] hover:bg-white/[0.05] no-drag cursor-pointer"
                      onClick={() => {
                        setMenuFor(null)
                        setDialog({ kind: 'reset', username: row.username })
                      }}
                    >
                      Reset password…
                    </button>
                    <button
                      type="button"
                      role="menuitem"
                      className="block w-full text-left px-3 py-1 text-[11px] text-[var(--color-text-primary)] hover:bg-white/[0.05] no-drag cursor-pointer"
                      onClick={() => {
                        setMenuFor(null)
                        setDialog({ kind: 'set', username: row.username })
                      }}
                    >
                      Set password…
                    </button>
                    {row.lockedUntil ? (
                      <button
                        type="button"
                        role="menuitem"
                        disabled={busy === `unlock:${row.username}`}
                        className="block w-full text-left px-3 py-1 text-[11px] text-[var(--color-text-primary)] hover:bg-white/[0.05] no-drag cursor-pointer disabled:opacity-40"
                        onClick={() => {
                          setMenuFor(null)
                          void unlock(row.username)
                        }}
                      >
                        Unlock
                      </button>
                    ) : null}
                  </div>
                ) : null}
              </div>
            ))
          )}
        </div>
        {isOwner ? (
          <div className="border-t border-[var(--color-border)] px-3 py-2">
            <button type="button" className={BTN_ACCENT} onClick={() => setAdding(true)}>
              Add user
            </button>
          </div>
        ) : null}
      </div>
      <div className="flex-1 overflow-y-auto p-6 min-h-0">
        {error && (
          <p role="alert" className="text-[10px] text-[var(--color-status-error)] mb-3">
            {error}
          </p>
        )}
        {notice && <p className="text-[10px] text-[var(--color-text-secondary)] mb-3">{notice}</p>}
        {person ? (
          <PersonDetail
            person={person}
            sewGrant={sewGrantOf(person.id)}
            onSewChanged={refresh}
            grants={personGrants}
            roles={roles}
            hostOn={Boolean(hostGrant)}
            drafts={drafts}
            busy={busy}
            expandedGrant={expandedGrant}
            deleteConfirm={deleteConfirm}
            grantKind={grantKind}
            grantRole={grantRole}
            grantApp={grantApp}
            grantTarget={grantTarget}
            grantScope={grantScope}
            appChoices={appChoices}
            roleChoices={roleChoices}
            onDraft={(username, value) => setDrafts((prev) => ({ ...prev, [username]: value }))}
            onSave={(username, value) => void saveName(username, value)}
            onExpand={setExpandedGrant}
            onEnabled={(grant, enabled) => void setEnabled(grant, enabled)}
            onRevoke={(id) => void revoke(id)}
            onHost={(present) => void setHost(person.username, present)}
            onDeleteConfirm={setDeleteConfirm}
            onDelete={() => void removeAccount(person.username)}
            onGrantKind={setGrantKind}
            onGrantRole={setGrantRole}
            onGrantApp={setGrantApp}
            onGrantTarget={setGrantTarget}
            onGrantScope={setGrantScope}
            onAddGrant={() => void addGrant()}
            onReset={isOwner ? () => setDialog({ kind: 'reset', username: person.username }) : null}
          />
        ) : (
          <p className="text-[10px] text-[var(--color-text-muted)]">Select a person.</p>
        )}
      </div>
      {dialogPerson && dialog?.kind === 'reset' ? (
        <ResetPasswordDialog key={`reset:${dialogPerson.username}`} person={dialogPerson} onClose={closeDialog} post={postHere} />
      ) : null}
      {dialogPerson && dialog?.kind === 'set' ? (
        <SetPasswordDialog
          key={`set:${dialogPerson.username}`}
          person={dialogPerson}
          onClose={closeDialog}
          post={postHere}
          onReset={() => setDialog({ kind: 'reset', username: dialogPerson.username })}
        />
      ) : null}
      {adding ? (
        <AddUserDialog
          onClose={() => setAdding(false)}
          apps={appChoices}
          roles={roles.map((r) => ({ value: r.id, label: r.name, appId: r.appId }))}
          loadApps={loadApps}
          post={postHere}
          onAdded={(result) => {
            setAdding(false)
            setSelection(result.username)
            if (result.secret || result.note) setAdded(result)
            void refresh()
          }}
        />
      ) : null}
      {added ? <AddedUserDialog added={added} onClose={() => setAdded(null)} /> : null}
    </div>
  )
}

function roleBits(role: RoleRow | null, fallbackHandles: string[]): React.JSX.Element {
  if (!role) {
    return (
      <p className="text-[10px] text-[var(--color-text-muted)]">
        default rooms: {fallbackHandles.length ? fallbackHandles.join(', ') : 'none'}
      </p>
    )
  }
  return (
    <div className="space-y-1">
      <p className="text-[10px] font-mono text-[var(--color-text-primary)]">{role.name}</p>
      {role.roomAccess.length === 0 ? (
        <span className="text-[10px] text-[var(--color-text-muted)]">no rooms</span>
      ) : (
        role.roomAccess.map((room) => (
          <div key={room.handle} className="flex flex-wrap items-center gap-1">
            <span className="text-[9px] font-mono px-1.5 py-0.5 bg-[var(--color-bg-surface)] text-[var(--color-text-secondary)] border border-[var(--color-border)]">
              {room.handle}
            </span>
            {room.caps.map((cap) => (
              <span
                key={cap}
                className="text-[9px] font-mono uppercase tracking-wider px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-text-secondary)]"
              >
                {cap}
              </span>
            ))}
          </div>
        ))
      )}
    </div>
  )
}

/**
 * agent-app-users §5.5: a person an agent set up is tied to that agent's
 * workspace. When you gave them more, the extra waits for their own reset.
 * The owner's Reset password (a one-time link, or mailed to an address
 * you give) is what lifts the tie. The stored email is never used: an
 * agent could have chosen it. Nothing for an untied person. `onReset`:
 * opens the Reset password sheet (the owner only; null hides it).
 */
export function ResetPanel({ person, onReset }: { person: PersonRow; onReset: (() => void) | null }): React.JSX.Element | null {
  if (!person.tiedTo) return null
  return (
    <SettingsGroup title={person.pendingReset ? 'Waiting for reset' : 'Set up by an agent'}>
      <p className="text-[10px] text-[var(--color-text-muted)]">
        {person.pendingReset
          ? `${person.tiedTo}'s agent set ${person.username} up, so the extra access you gave starts only after ${person.username} resets their password. Until then they keep ${person.tiedTo}'s access.`
          : `${person.tiedTo}'s agent set ${person.username} up. More access you give waits for their password reset.`}
        {` ${person.tiedTo}'s agent could have chosen the stored email${person.email ? ` (${person.email})` : ''}, so a reset never goes there: send it to an email you know is theirs, or get a one-time link.`}
        {' '}Links open K2's own page.
      </p>
      {onReset ? (
        <div className="pt-1">
          <button
            type="button"
            className="text-[10px] text-[var(--color-accent)] hover:underline no-drag cursor-pointer"
            onClick={onReset}
          >
            Reset password…
          </button>
        </div>
      ) : null}
    </SettingsGroup>
  )
}

function PersonDetail({
  person,
  sewGrant,
  onSewChanged,
  grants,
  roles,
  hostOn,
  drafts,
  busy,
  expandedGrant,
  deleteConfirm,
  grantKind,
  grantRole,
  grantApp,
  grantTarget,
  grantScope,
  appChoices,
  roleChoices,
  onDraft,
  onSave,
  onExpand,
  onEnabled,
  onRevoke,
  onHost,
  onDeleteConfirm,
  onDelete,
  onGrantKind,
  onGrantRole,
  onGrantApp,
  onGrantTarget,
  onGrantScope,
  onAddGrant,
  onReset,
}: {
  person: PersonRow
  sewGrant: ChatGrant | null
  onSewChanged: () => Promise<void>
  grants: GrantRow[]
  roles: RoleRow[]
  hostOn: boolean
  drafts: Record<string, string>
  busy: string | null
  expandedGrant: string | null
  deleteConfirm: string | null
  grantKind: string
  grantRole: string
  grantApp: string
  grantTarget: string
  grantScope: string
  appChoices: { value: string; label: string }[]
  roleChoices: { value: string; label: string }[]
  onDraft: (username: string, value: string) => void
  onSave: (username: string, value: string) => void
  onExpand: (id: string | null) => void
  onEnabled: (grant: GrantRow, enabled: boolean) => void
  onRevoke: (id: string) => void
  onHost: (present: boolean) => void
  onDeleteConfirm: (username: string | null) => void
  onDelete: () => void
  onGrantKind: (value: string) => void
  onGrantRole: (value: string) => void
  onGrantApp: (value: string) => void
  onGrantTarget: (value: string) => void
  onGrantScope: (value: string) => void
  onAddGrant: () => void
  onReset: (() => void) | null
}): React.JSX.Element {
  const value = drafts[person.username] ?? person.fullName ?? ''
  const grantRoles = roleChoices.filter((choice) => {
    if (!grantApp) return true
    const role = roles.find((r) => r.id === choice.value)
    return !role?.appId || role.appId === grantApp
  })
  return (
    <div className="max-w-2xl space-y-6">
      <form
        className="flex flex-wrap items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault()
          onSave(person.username, value.trim())
        }}
      >
        <span className="w-28 text-xs font-mono text-[var(--color-text-primary)] truncate">
          {person.username}
        </span>
        <input
          className={INPUT_CLS}
          aria-label={`${person.username} full name`}
          placeholder="full name"
          value={value}
          disabled={busy === person.username}
          onChange={(e) => onDraft(person.username, e.target.value)}
        />
        <button
          type="submit"
          aria-label={`Save ${person.username} full name`}
          disabled={busy === person.username}
          className="text-[10px] text-[var(--color-accent)] hover:underline no-drag cursor-pointer disabled:opacity-40"
        >
          Save
        </button>
        {person.fullName ? (
          <button
            type="button"
            aria-label={`Clear ${person.username} full name`}
            disabled={busy === person.username}
            onClick={() => onSave(person.username, '')}
            className="text-[10px] text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer disabled:opacity-40"
          >
            Clear
          </button>
        ) : null}
      </form>

      <ResetPanel key={person.username} person={person} onReset={onReset} />

      <SewPersonAccess principalId={person.id} username={person.username} grant={sewGrant} onChanged={onSewChanged} />

      <SettingsGroup title="Grants">
        {grants.length === 0 ? (
          <p className="text-[10px] text-[var(--color-text-muted)]">No grants for this person.</p>
        ) : (
          <div className="divide-y divide-[var(--color-border)]">
            {grants.map((grant) => {
              const open = expandedGrant === grant.id
              const isHost = grant.kind === 'app' && grant.targetId === 'host'
              const role = isHost
                ? (person.roleId ? roles.find((r) => r.id === person.roleId) ?? null : null)
                : (grant.roleId ? roles.find((r) => r.id === grant.roleId) ?? null : null)
              return (
                <div key={grant.id} className="py-2 space-y-2">
                  <div className="flex items-center justify-between gap-2">
                    <button
                      type="button"
                      className="text-xs font-mono text-[var(--color-text-primary)] no-drag cursor-pointer"
                      aria-label={`Show ${grant.kind} ${grant.targetId}`}
                      onClick={() => onExpand(open ? null : grant.id)}
                    >
                      {grant.kind} {isHost ? 'Host' : grant.targetId}
                      {grant.scope ? ` · ${grant.scope}` : ''}
                      {grant.kind !== 'app' ? ' · recorded, not enforced' : ''}
                    </button>
                    <span className="flex items-center gap-2">
                      <AccessSwitch
                        on={grant.enabled}
                        label={`Enable ${grant.kind} ${grant.targetId}`}
                        disabled={busy === grant.id}
                        onToggle={() => onEnabled(grant, !grant.enabled)}
                      />
                      <button
                        type="button"
                        aria-label={`Revoke ${grant.id}`}
                        className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
                        onClick={() => onRevoke(grant.id)}
                      >
                        Revoke
                      </button>
                    </span>
                  </div>
                  {open && grant.kind === 'app' ? roleBits(role, isHost ? person.defaultRoomHandles : []) : null}
                </div>
              )
            })}
          </div>
        )}
        <SettingRow label="Kind">
          <SettingDropdown
            ariaLabel="Grant kind"
            value={grantKind}
            placeholder="kind"
            options={KIND_OPTIONS}
            onChange={onGrantKind}
            menuAlign="right"
          />
        </SettingRow>
        {grantKind === 'app' ? (
          <>
            <SettingRow label="Role">
              <SettingDropdown
                ariaLabel="Grant role"
                value={grantRole}
                placeholder="role"
                options={grantRoles}
                onChange={onGrantRole}
                menuAlign="right"
              />
            </SettingRow>
            <SettingRow label="App">
              <SettingDropdown
                ariaLabel="Grant app"
                value={grantApp}
                placeholder="app"
                options={appChoices}
                onChange={onGrantApp}
                menuAlign="right"
              />
            </SettingRow>
          </>
        ) : grantKind ? (
          <>
            <SettingRow label="Target">
              <input
                className={INPUT_CLS}
                aria-label="Grant target"
                value={grantTarget}
                onChange={(e) => onGrantTarget(e.target.value)}
              />
            </SettingRow>
            <SettingRow label="Scope">
              <input
                className={INPUT_CLS}
                aria-label="Grant scope"
                value={grantScope}
                onChange={(e) => onGrantScope(e.target.value)}
              />
            </SettingRow>
          </>
        ) : null}
        <button
          type="button"
          className={BTN_ACCENT}
          disabled={busy === 'add-grant' || !grantKind}
          onClick={onAddGrant}
        >
          Add grant
        </button>
      </SettingsGroup>

      <SettingRow label="Host login">
        <AccessSwitch
          on={hostOn}
          label={`Host login for ${person.username}`}
          disabled={busy === `host:${person.username}`}
          onToggle={() => onHost(!hostOn)}
        />
      </SettingRow>
      {deleteConfirm === person.username ? (
        <span className="flex items-center gap-2">
          <button
            type="button"
            className="text-[10px] text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
            onClick={onDelete}
          >
            Confirm delete account
          </button>
          <button
            type="button"
            className="text-[10px] text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer"
            onClick={() => onDeleteConfirm(null)}
          >
            Cancel
          </button>
        </span>
      ) : (
        <button
          type="button"
          aria-label={`Delete account ${person.username}`}
          className="text-[10px] text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
          onClick={() => onDeleteConfirm(person.username)}
        >
          Delete account
        </button>
      )}
    </div>
  )
}
