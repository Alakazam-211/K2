// Settings → K2 Server → User Access. Principals (username + full name)
// plus that person's grants and access templates. Not Admin Access
// (Connect users) and not Sidecars → Apps.

import React, { useCallback, useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { SettingDropdown, SettingRow, SettingsGroup } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'

export const PEOPLE_MANIFEST: SettingEntry[] = [
  {
    id: 'people.roster',
    section: 'people',
    label: 'User Access',
    description: 'Skin principals on this box — username and full name. Not Admin Access.',
    keywords: ['people', 'user access', 'full name', 'principal', 'guest', 'username', 'grant'],
    group: 'People',
  },
  {
    id: 'people.templates',
    section: 'people',
    label: 'Access templates',
    description: 'Kind and role lines applied onto a person as grants. Editing a template does not rewrite grants.',
    keywords: ['template', 'access', 'grant', 'mailbox', 'role'],
    group: 'Access templates',
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
}

type TemplateLine = {
  id: string
  templateId: string
  kind: string
  roleId: string | null
  position: number
}

type TemplateRow = {
  id: string
  name: string
  lines: TemplateLine[]
}

type AppOption = { id: string; name: string; kind: string }

type Selection =
  | { kind: 'person'; username: string }
  | { kind: 'template'; id: string }

const KIND_OPTIONS = [
  { value: 'app', label: 'app' },
  { value: 'mailbox', label: 'mailbox' },
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
    }]
  })
}

function parseTemplates(raw: unknown): TemplateRow[] {
  return asList(raw, ['templates']).flatMap((row) => {
    const rec = asRecord(row)
    const id = asString(rec.id)
    const name = asString(rec.name)
    if (!id || !name) return []
    const lines = asList(rec.lines, ['lines']).flatMap((line) => {
      const item = asRecord(line)
      const lineId = asString(item.id)
      const kind = asString(item.kind)
      if (!lineId || !kind) return []
      const position = item.position
      return [{
        id: lineId,
        templateId: asString(item.templateId) ?? asString(item.template_id) ?? id,
        kind,
        roleId: asString(item.roleId) ?? asString(item.role_id),
        position: typeof position === 'number' ? position : 0,
      }]
    })
    return [{ id, name, lines }]
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
  const [templates, setTemplates] = useState<TemplateRow[]>([])
  const [apps, setApps] = useState<AppOption[]>([])
  const [appsLoaded, setAppsLoaded] = useState(false)
  const [selection, setSelection] = useState<Selection | null>(null)
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
  const [templateName, setTemplateName] = useState('')
  const [lineKind, setLineKind] = useState('')
  const [lineRole, setLineRole] = useState('')
  const [applyPerson, setApplyPerson] = useState('')
  const [applyTargets, setApplyTargets] = useState<Record<string, string>>({})
  const [applyScopes, setApplyScopes] = useState<Record<string, string>>({})

  const refresh = useCallback(async () => {
    setError(null)
    const failures: string[] = []
    try {
      setPeople(parsePeople(await daemonCliGet<unknown>('skin/users')))
    } catch (e) {
      failures.push(errText(e))
      setPeople([])
    }
    try {
      setGrants(parseGrants(await daemonCliGet<unknown>('skin/grants')))
    } catch (e) {
      failures.push(errText(e))
      setGrants([])
    }
    try {
      setRoles(parseRoles(await daemonCliGet<unknown>('skin/roles')))
    } catch (e) {
      failures.push(errText(e))
      setRoles([])
    }
    try {
      setTemplates(parseTemplates(await daemonCliGet<unknown>('skin/templates')))
    } catch (e) {
      failures.push(errText(e))
      setTemplates([])
    }
    if (failures.length) setError(failures.join(' · '))
    setLoading(false)
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const loadApps = useCallback(async () => {
    if (appsLoaded) return
    const projects = await daemonCliGet<unknown>('projects/list')
    const list = Array.isArray(projects) ? projects : asList(projects, ['projects', 'items'])
    const workspaces = list.flatMap((row) => {
      const rec = asRecord(row)
      const id = asString(rec.id)
      if (!id) return []
      return [id]
    })
    const next: AppOption[] = []
    for (const projectId of workspaces) {
      const listed = await daemonCliGet<unknown>('publish/list', { project: projectId })
      next.push(...parseApps(listed))
    }
    setApps(next)
    setAppsLoaded(true)
  }, [appsLoaded])

  useEffect(() => {
    const template = selection?.kind === 'template'
      ? templates.find((t) => t.id === selection.id)
      : undefined
    const needsApps =
      grantKind === 'app' ||
      lineKind === 'app' ||
      Boolean(template?.lines.some((line) => line.kind === 'app'))
    if (!needsApps) return
    void loadApps().catch((e) => setError(errText(e)))
  }, [grantKind, lineKind, selection, templates, loadApps])

  const person = selection?.kind === 'person'
    ? people.find((p) => p.username === selection.username) ?? null
    : null
  const template = selection?.kind === 'template'
    ? templates.find((t) => t.id === selection.id) ?? null
    : null

  const saveName = useCallback(
    async (username: string, fullName: string) => {
      setBusy(username)
      setError(null)
      try {
        await daemonCliPost('skin/users/full-name', { username, fullName })
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
        await daemonCliPost('skin/grants/enabled', { id: grant.id, enabled })
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
        await daemonCliPost('skin/grants/host', { username, present })
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
        await daemonCliPost('skin/grants/delete', { id })
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
        await daemonCliPost('skin/users/remove', { username })
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
      await daemonCliPost('skin/grants', {
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

  const addTemplate = useCallback(async () => {
    const name = templateName.trim()
    if (!name) return
    setBusy('add-template')
    setError(null)
    try {
      await daemonCliPost('skin/templates', { name })
      setTemplateName('')
      await refresh()
    } catch (e) {
      setError(errText(e))
    } finally {
      setBusy(null)
    }
  }, [templateName, refresh])

  const renameTemplate = useCallback(
    async (id: string, name: string) => {
      setBusy(id)
      setError(null)
      try {
        await daemonCliPost('skin/templates/update', { id, name })
        await refresh()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusy(null)
      }
    },
    [refresh],
  )

  const removeTemplate = useCallback(
    async (id: string) => {
      setBusy(id)
      setError(null)
      try {
        await daemonCliPost('skin/templates/delete', { id })
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

  const addLine = useCallback(async () => {
    if (!template) return
    setBusy('add-line')
    setError(null)
    try {
      await daemonCliPost('skin/templates/lines', {
        templateId: template.id,
        kind: lineKind,
        ...(lineKind === 'app' ? { roleId: lineRole } : {}),
      })
      await refresh()
    } catch (e) {
      setError(errText(e))
    } finally {
      setBusy(null)
    }
  }, [template, lineKind, lineRole, refresh])

  const removeLine = useCallback(
    async (id: string) => {
      setBusy(id)
      setError(null)
      try {
        await daemonCliPost('skin/templates/lines/delete', { id })
        await refresh()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusy(null)
      }
    },
    [refresh],
  )

  const applyTemplate = useCallback(async () => {
    if (!template) return
    const who = people.find((p) => p.id === applyPerson)
    if (!who?.id) {
      setError('missing subject id')
      return
    }
    setBusy('apply')
    setError(null)
    try {
      await daemonCliPost('skin/templates/apply', {
        principalId: who.id,
        templateId: template.id,
        lines: template.lines.map((line) => ({
          lineId: line.id,
          targetId: applyTargets[line.id] ?? '',
          ...(line.kind === 'app' ? {} : { scope: (applyScopes[line.id] ?? '').trim() }),
        })),
      })
      await refresh()
    } catch (e) {
      setError(errText(e))
    } finally {
      setBusy(null)
    }
  }, [template, people, applyPerson, applyTargets, applyScopes, refresh])

  const personGrants = person?.id
    ? grants.filter((g) => g.subjectKind === 'principal' && g.subjectId === person.id)
    : []
  const hostGrant = personGrants.find((g) => g.kind === 'app' && g.targetId === 'host')
  const appChoices = apps.map((app) => ({ value: app.id, label: `${app.name} (${app.kind})` }))
  const roleChoices = roles.map((role) => ({ value: role.id, label: role.name }))

  return (
    <div className="flex h-full min-h-0">
      <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col min-h-0">
        <div className="px-3 pt-3 pb-2 border-b border-[var(--color-border)]">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">User Access</h2>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
            Username and full name for app guests on this box. This reads principals.
            It is not Admin Access and not a separate people list.
          </p>
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
            <p className="px-3 text-[10px] text-[var(--color-text-muted)]">
              No skin users yet. Add them under Sidecars → Apps.
            </p>
          ) : (
            people.map((row) => (
              <button
                key={row.username}
                type="button"
                aria-label={`Open ${row.username}`}
                className={listButtonClass(selection?.kind === 'person' && selection.username === row.username)}
                onClick={() => setSelection({ kind: 'person', username: row.username })}
              >
                <span className="block text-xs font-mono text-[var(--color-text-primary)] truncate">
                  {row.username}
                </span>
                {row.fullName ? (
                  <span className="block text-[10px] text-[var(--color-text-muted)] truncate">
                    {row.fullName}
                  </span>
                ) : null}
              </button>
            ))
          )}
          <div data-settings-id="people.templates" className="border-t border-[var(--color-border)] mt-2">
            <div className="px-3 pt-2 pb-1">
              <span className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
                Access templates
              </span>
            </div>
            {templates.length === 0 ? (
              <p className="px-3 pb-2 text-[10px] text-[var(--color-text-muted)]">No access templates yet.</p>
            ) : (
              templates.map((row) => (
                <button
                  key={row.id}
                  type="button"
                  aria-label={`Open template ${row.name}`}
                  className={listButtonClass(selection?.kind === 'template' && selection.id === row.id)}
                  onClick={() => setSelection({ kind: 'template', id: row.id })}
                >
                  <span className="block text-xs text-[var(--color-text-primary)] truncate">{row.name}</span>
                </button>
              ))
            )}
            <form
              className="px-3 py-2 space-y-1.5"
              onSubmit={(e) => {
                e.preventDefault()
                void addTemplate()
              }}
            >
              <input
                className={`${INPUT_CLS} w-full`}
                aria-label="New access template name"
                placeholder="template name"
                value={templateName}
                onChange={(e) => setTemplateName(e.target.value)}
              />
              <button type="submit" className={BTN_ACCENT} disabled={busy === 'add-template' || !templateName.trim()}>
                Add template
              </button>
            </form>
          </div>
        </div>
      </div>
      <div className="flex-1 overflow-y-auto p-6 min-h-0">
        {error && (
          <p role="alert" className="text-[10px] text-[var(--color-status-error)] mb-3">
            {error}
          </p>
        )}
        {person ? (
          <PersonDetail
            person={person}
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
          />
        ) : template ? (
          <TemplateDetail
            template={template}
            people={people}
            roles={roles}
            apps={apps}
            busy={busy}
            lineKind={lineKind}
            lineRole={lineRole}
            applyPerson={applyPerson}
            applyTargets={applyTargets}
            applyScopes={applyScopes}
            onRename={(name) => void renameTemplate(template.id, name)}
            onDelete={() => void removeTemplate(template.id)}
            onLineKind={setLineKind}
            onLineRole={setLineRole}
            onAddLine={() => void addLine()}
            onRemoveLine={(id) => void removeLine(id)}
            onApplyPerson={setApplyPerson}
            onApplyTarget={(id, value) => setApplyTargets((prev) => ({ ...prev, [id]: value }))}
            onApplyScope={(id, value) => setApplyScopes((prev) => ({ ...prev, [id]: value }))}
            onApply={() => void applyTemplate()}
          />
        ) : (
          <p className="text-[10px] text-[var(--color-text-muted)]">
            Select a person or an access template.
          </p>
        )}
      </div>
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

function PersonDetail({
  person,
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
}: {
  person: PersonRow
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

function TemplateDetail({
  template,
  people,
  roles,
  apps,
  busy,
  lineKind,
  lineRole,
  applyPerson,
  applyTargets,
  applyScopes,
  onRename,
  onDelete,
  onLineKind,
  onLineRole,
  onAddLine,
  onRemoveLine,
  onApplyPerson,
  onApplyTarget,
  onApplyScope,
  onApply,
}: {
  template: TemplateRow
  people: PersonRow[]
  roles: RoleRow[]
  apps: AppOption[]
  busy: string | null
  lineKind: string
  lineRole: string
  applyPerson: string
  applyTargets: Record<string, string>
  applyScopes: Record<string, string>
  onRename: (name: string) => void
  onDelete: () => void
  onLineKind: (value: string) => void
  onLineRole: (value: string) => void
  onAddLine: () => void
  onRemoveLine: (id: string) => void
  onApplyPerson: (id: string) => void
  onApplyTarget: (lineId: string, value: string) => void
  onApplyScope: (lineId: string, value: string) => void
  onApply: () => void
}): React.JSX.Element {
  const [name, setName] = useState(template.name)
  useEffect(() => {
    setName(template.name)
  }, [template.name])
  const roleChoices = roles.map((role) => ({ value: role.id, label: role.name }))
  return (
    <div className="max-w-2xl space-y-4">
      <SettingsGroup title="Access template">
        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            onRename(name.trim())
          }}
        >
          <input
            className={INPUT_CLS}
            aria-label={`Template ${template.name} name`}
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <button type="submit" className="text-[10px] text-[var(--color-accent)] hover:underline no-drag cursor-pointer">
            Save template
          </button>
          <button
            type="button"
            className="text-[10px] text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
            onClick={onDelete}
          >
            Delete template
          </button>
        </form>
        {template.lines.length === 0 ? (
          <p className="text-[10px] text-[var(--color-text-muted)]">No lines yet. A line stores kind and, for app, a role. Not a target.</p>
        ) : (
          <div className="divide-y divide-[var(--color-border)]">
            {template.lines.map((line) => {
              const role = line.roleId ? roles.find((r) => r.id === line.roleId) : null
              const targets = apps
                .filter((app) => !role?.appId || app.id === role.appId)
                .map((app) => ({ value: app.id, label: `${app.name} (${app.kind})` }))
              return (
                <div key={line.id} className="py-2 space-y-1">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-xs font-mono text-[var(--color-text-primary)]">
                      {line.kind}{role ? ` · ${role.name}` : ''}
                    </span>
                    <button
                      type="button"
                      className="text-[10px] text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer"
                      onClick={() => onRemoveLine(line.id)}
                    >
                      Remove line
                    </button>
                  </div>
                  {line.kind === 'app' ? (
                    <SettingRow label="App">
                      <SettingDropdown
                        ariaLabel={`Apply target ${line.id}`}
                        value={applyTargets[line.id] ?? ''}
                        placeholder="app"
                        options={targets}
                        onChange={(value) => onApplyTarget(line.id, value)}
                        menuAlign="right"
                      />
                    </SettingRow>
                  ) : (
                    <>
                      <SettingRow label="Target">
                        <input
                          className={INPUT_CLS}
                          aria-label={`Apply target ${line.id}`}
                          value={applyTargets[line.id] ?? ''}
                          onChange={(e) => onApplyTarget(line.id, e.target.value)}
                        />
                      </SettingRow>
                      <SettingRow label="Scope">
                        <input
                          className={INPUT_CLS}
                          aria-label={`Apply scope ${line.id}`}
                          value={applyScopes[line.id] ?? ''}
                          onChange={(e) => onApplyScope(line.id, e.target.value)}
                        />
                      </SettingRow>
                    </>
                  )}
                </div>
              )
            })}
          </div>
        )}
        <SettingRow label="Kind">
          <SettingDropdown
            ariaLabel="Template line kind"
            value={lineKind}
            placeholder="kind"
            options={KIND_OPTIONS}
            onChange={onLineKind}
            menuAlign="right"
          />
        </SettingRow>
        {lineKind === 'app' ? (
          <SettingRow label="Role">
            <SettingDropdown
              ariaLabel="Template line role"
              value={lineRole}
              placeholder="role"
              options={roleChoices}
              onChange={onLineRole}
              menuAlign="right"
            />
          </SettingRow>
        ) : null}
        <button type="button" className={BTN_ACCENT} disabled={busy === 'add-line' || !lineKind} onClick={onAddLine}>
          Add line
        </button>
        <SettingRow label="Person">
          <SettingDropdown
            ariaLabel="Apply template to"
            value={applyPerson}
            placeholder="person"
            options={people.flatMap((p) => (p.id ? [{ value: p.id, label: p.username }] : []))}
            onChange={onApplyPerson}
            menuAlign="right"
          />
        </SettingRow>
        <button type="button" className={BTN_ACCENT} disabled={busy === 'apply' || template.lines.length === 0} onClick={onApply}>
          Apply template
        </button>
      </SettingsGroup>
    </div>
  )
}
