// Settings → K2 Server → User Templates. Named access_templates recipes.
// A line stores kind and, for kind=app, a role_id. No target_id on a line.
// Apply inserts real grants and collects target_id (and scope) then.
// Editing a template does not rewrite grants already written.

import React, { useCallback, useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { SettingDropdown, SettingRow, SettingsGroup } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'

export const USER_TEMPLATES_MANIFEST: SettingEntry[] = [
  {
    id: 'user-templates.list',
    section: 'user-templates',
    label: 'User Templates',
    description: 'Kind and role lines applied onto a person as grants. Editing a template does not rewrite grants.',
    keywords: ['template', 'access', 'grant', 'mailbox', 'role', 'user templates'],
    group: 'User Templates',
  },
]

type PersonRow = {
  id: string | null
  username: string
}

type RoleRow = {
  id: string
  name: string
  appId: string | null
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

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

function parsePeople(raw: unknown): PersonRow[] {
  return asList(raw, ['users']).flatMap((row) => {
    const rec = asRecord(row)
    const username = asString(rec.username)
    if (!username) return []
    return [{ id: asString(rec.id), username }]
  })
}

function parseRoles(raw: unknown): RoleRow[] {
  return asList(raw, ['roles']).flatMap((row) => {
    const rec = asRecord(row)
    const id = asString(rec.id)
    const name = asString(rec.name)
    if (!id || !name) return []
    return [{ id, name, appId: asString(rec.appId) ?? asString(rec.app_id) }]
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

function listButtonClass(active: boolean): string {
  return `w-full text-left px-3 py-2 no-drag cursor-pointer ${
    active ? 'bg-[var(--color-accent)]/15' : 'hover:bg-white/[0.03]'
  }`
}

export function UserTemplatesSection(): React.JSX.Element {
  const [people, setPeople] = useState<PersonRow[]>([])
  const [roles, setRoles] = useState<RoleRow[]>([])
  const [templates, setTemplates] = useState<TemplateRow[]>([])
  const [apps, setApps] = useState<AppOption[]>([])
  const [appsLoaded, setAppsLoaded] = useState(false)
  const [selection, setSelection] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
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
      setTemplates(parseTemplates(await daemonCliGet<unknown>('skin/templates')))
    } catch (e) {
      failures.push(errText(e))
      setTemplates([])
    }
    try {
      setRoles(parseRoles(await daemonCliGet<unknown>('skin/roles')))
    } catch (e) {
      failures.push(errText(e))
      setRoles([])
    }
    try {
      setPeople(parsePeople(await daemonCliGet<unknown>('skin/users')))
    } catch (e) {
      failures.push(errText(e))
      setPeople([])
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

  const template = selection ? templates.find((t) => t.id === selection) ?? null : null

  useEffect(() => {
    const needsApps = lineKind === 'app' || Boolean(template?.lines.some((line) => line.kind === 'app'))
    if (!needsApps) return
    void loadApps().catch((e) => setError(errText(e)))
  }, [lineKind, template, loadApps])

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

  return (
    <div className="flex h-full min-h-0">
      <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col min-h-0">
        <div className="px-3 pt-3 pb-2 border-b border-[var(--color-border)]">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">User Templates</h2>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
            A template is a named recipe. Applying it writes grants. Editing it does not rewrite grants already written.
          </p>
        </div>
        <div data-settings-id="user-templates.list" className="flex-1 overflow-y-auto min-h-0">
          <div className="px-3 pt-2 pb-1">
            <span className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
              Access templates
            </span>
          </div>
          {loading ? (
            <p className="px-3 text-[10px] text-[var(--color-text-muted)]">Loading…</p>
          ) : templates.length === 0 ? (
            <p className="px-3 text-[10px] text-[var(--color-text-muted)]">No access templates yet.</p>
          ) : (
            templates.map((row) => (
              <button
                key={row.id}
                type="button"
                aria-label={`Open template ${row.name}`}
                className={listButtonClass(selection === row.id)}
                onClick={() => setSelection(row.id)}
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
      <div className="flex-1 overflow-y-auto p-6 min-h-0">
        {error && (
          <p role="alert" className="text-[10px] text-[var(--color-status-error)] mb-3">
            {error}
          </p>
        )}
        {template ? (
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
          <p className="text-[10px] text-[var(--color-text-muted)]">Select an access template.</p>
        )}
      </div>
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
        <p className="text-[10px] text-[var(--color-text-muted)]">
          Saving this name or its lines does not rewrite grants already written.
        </p>
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
