// Settings → Access → User Templates: starting sets of access for new
// people. A list and detail like Workspaces / Agents: the templates on the
// left; on the right one template, with what it gives (an App and a role per
// line) and Apply, which gives a person all of it at once.
//
// The daemon's words: a template (`/cli/skin/templates`) is a list of lines;
// a line stores a kind and, for an App, a role, never the App itself, so a
// role that works in any App asks which App when you apply. Applying writes
// grants for that person; changing the template later doesn't rewrite them.

import React, { useCallback, useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import { primaryScope } from '@/kessel/server-scope'
import { Button, Input, OverflowMenu, Surface, Tag, cx } from '@/components/ui'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useSettingsStore } from '@/stores/settings'
import {
  SETTING_FIELD_WIDTH,
  SaveError,
  SaveNote,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  useSaveStatus,
  type SaveState,
} from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import { AccessAuditLink } from '../shared/AccessAuditLink'
import {
  HOST_APP,
  HOST_LABEL,
  appName,
  asRecord,
  asString,
  fetchApps,
  listWords,
  parsePeople,
  parseRoles,
  parseTemplates,
  reachWords,
  roleLabel,
  showPersonNext,
  type AppOption,
  type PersonRow,
  type RoleRow,
  type TemplateLine,
  type TemplateRow,
} from './user-access-api'

export const USER_TEMPLATES_MANIFEST: SettingEntry[] = [
  {
    id: 'user-templates.list',
    section: 'user-templates',
    label: 'User Templates',
    description: 'Starting sets of access for new people: Apps and roles you give someone in one step.',
    keywords: ['template', 'access', 'grant', 'role', 'apply', 'new person', 'user templates'],
    group: 'User Templates',
  },
]

type Load = 'loading' | 'ready' | { error: string }
/** A template id, the new-template form, or nothing yet. */
type Selection = string | 'new' | null

const NOTED_KIND: Record<string, string> = { mailbox: 'Mailbox', database: 'Database', cli: 'Command-line tool' }

/** One write at a time: which part it was, whether it's running, and how it
 *  ended (the shared "Saved" note, or why it failed). */
function useWrites(): {
  busy: string | null
  at: (key: string) => SaveState
  run: (key: string, fn: () => Promise<unknown>) => Promise<boolean>
} {
  const status = useSaveStatus(3000)
  const [key, setKey] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  return {
    busy,
    at: (k) => (key === k ? status.state : 'idle'),
    run: async (k, fn) => {
      setKey(k)
      setBusy(k)
      status.reset()
      try {
        await fn()
        status.saved()
        return true
      } catch (e) {
        status.failed(new Error(daemonErrorText(e)))
        return false
      } finally {
        setBusy(null)
      }
    },
  }
}

function linesWords(t: TemplateRow): string {
  const n = t.lines.length
  if (n === 0) return 'Nothing in it yet'
  const apps = t.lines.every((l) => l.kind === 'app')
  return `${n} ${apps ? (n === 1 ? 'App' : 'Apps') : n === 1 ? 'item' : 'items'}`
}

export function UserTemplatesSection(): React.JSX.Element {
  const [templates, setTemplates] = useState<TemplateRow[]>([])
  const [roles, setRoles] = useState<RoleRow[]>([])
  const [people, setPeople] = useState<PersonRow[]>([])
  const [load, setLoad] = useState<Load>('loading')
  const [apps, setApps] = useState<AppOption[] | null>(null)
  const [appsError, setAppsError] = useState<string | null>(null)
  const [selection, setSelection] = useState<Selection>(null)
  const writes = useWrites()

  /** The first read shows the loading card; later ones keep the page. */
  const refresh = useCallback(async () => {
    try {
      const [t, r, u] = await Promise.all([
        daemonCliGet<unknown>(primaryScope(), 'skin/templates'),
        daemonCliGet<unknown>(primaryScope(), 'skin/roles'),
        daemonCliGet<unknown>(primaryScope(), 'skin/users'),
      ])
      const next = parseTemplates(t)
      setTemplates(next)
      setRoles(parseRoles(r))
      setPeople(parsePeople(u))
      setLoad('ready')
      // Open on the first template, never an empty pane.
      setSelection((s) => (s === 'new' || (s && next.some((x) => x.id === s)) ? s : next[0]?.id ?? null))
    } catch (e) {
      setLoad({ error: daemonErrorText(e) })
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  // App names: for lines, and for roles made for one App.
  const needsApps = templates.some((t) => t.lines.some((l) => l.kind === 'app')) || roles.some((r) => r.appId)
  const loadApps = useCallback(async () => {
    setAppsError(null)
    try {
      setApps(await fetchApps())
    } catch (e) {
      setAppsError(daemonErrorText(e))
    }
  }, [])
  useEffect(() => {
    if (needsApps && !apps) void loadApps()
  }, [needsApps, apps, loadApps])

  const create = async (name: string): Promise<boolean> => {
    let id: string | null = null
    const ok = await writes.run('create', async () => {
      id = asString(asRecord(await daemonCliPost<unknown>(primaryScope(), 'skin/templates', { name })).id)
    })
    if (!ok) return false
    await refresh()
    setSelection((s) => id ?? s)
    return true
  }

  const template = selection && selection !== 'new' ? templates.find((t) => t.id === selection) ?? null : null
  const ready = load === 'ready'

  return (
    <div className="h-full min-h-0 flex">
      <aside aria-label="Templates" className="w-72 flex-shrink-0 min-h-0 flex flex-col border-r border-[var(--color-border)]">
        <div className="flex-shrink-0 px-4 pt-6 pb-3">
          <h2 className="flex items-center h-7 type-heading text-[var(--color-text-primary)]">User Templates</h2>
          <p className="type-hint text-[var(--color-text-secondary)] leading-relaxed">
            Starting sets of access for new people. Apply one to give someone all of it at once.
          </p>
          <AccessAuditLink className="mt-1" />
        </div>
        <div data-settings-id="user-templates.list" className="flex-1 min-h-0 overflow-y-auto px-4 pb-4 space-y-3 [scrollbar-gutter:stable]">
          {ready && templates.length > 0 ? (
            <section>
              <div className="flex items-center h-7 mb-1 type-heading text-[var(--color-text-primary)]">Templates</div>
              {/* overflow-hidden: a selected or hovered row keeps to the card's corners. */}
              <Surface chamfer material="tile" className="overflow-hidden divide-y divide-[var(--color-border)]">
                {templates.map((row) => {
                  const selected = selection === row.id
                  return (
                    <button
                      key={row.id}
                      type="button"
                      aria-label={`Open template ${row.name}`}
                      aria-current={selected ? 'true' : undefined}
                      data-template-row={row.id}
                      data-k2-focus=""
                      onClick={() => setSelection(row.id)}
                      className={cx(
                        'relative w-full flex flex-col px-3 py-2 text-left no-drag cursor-pointer transition-colors outline-none',
                        selected ? 'bg-[var(--color-accent)]/15' : 'hover:bg-[var(--color-wash-1)]',
                      )}
                    >
                      {selected && <span aria-hidden="true" className="absolute left-0 top-0 bottom-0 w-[3px] bg-[var(--color-accent)]" />}
                      <span className="block type-name text-[var(--color-text-primary)] truncate">{row.name}</span>
                      <span className="block type-hint text-[var(--color-text-secondary)] truncate">{linesWords(row)}</span>
                    </button>
                  )
                })}
              </Surface>
            </section>
          ) : null}
          {ready && templates.length > 0 ? (
            <Button variant="quiet" className="w-full justify-center" disabled={selection === 'new'} onClick={() => setSelection('new')}>
              New template
            </Button>
          ) : null}
        </div>
      </aside>

      <div className="flex-1 min-w-0 min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]">
        <div className="w-full max-w-[52rem] space-y-6 pb-6">
          {load === 'loading' ? (
            <SettingsLoadState kind="loading" title="Loading templates…" testId="templates-loading">
              Reading this server’s templates and roles.
            </SettingsLoadState>
          ) : typeof load === 'object' ? (
            <SettingsLoadState
              kind="error"
              title="Couldn’t load templates"
              detail={load.error}
              testId="templates-error"
              action={<Button onClick={() => { setLoad('loading'); void refresh() }}>Try again</Button>}
            >
              K2 couldn’t read this server’s templates. Nothing has changed.
            </SettingsLoadState>
          ) : selection === 'new' ? (
            <NewTemplate
              busy={writes.busy === 'create'}
              state={writes.at('create')}
              onCancel={templates.length ? () => setSelection(templates[0].id) : null}
              onCreate={create}
            />
          ) : template ? (
            <TemplateDetail
              key={template.id}
              template={template}
              people={people}
              roles={roles}
              apps={apps}
              appsError={appsError}
              created={writes.at('create') === 'saved'}
              refresh={refresh}
              onDeleted={() => setSelection(null)}
            />
          ) : (
            <SettingsLoadState
              kind="empty"
              title="No templates yet"
              testId="templates-empty"
              action={
                <Button variant="accent" onClick={() => setSelection('new')}>
                  New template
                </Button>
              }
            >
              A template is a starting set of access, such as the Shop App as Staff. Make one, then give it to each new person
              in one step.
            </SettingsLoadState>
          )}
        </div>
      </div>
    </div>
  )
}

function NewTemplate({
  busy,
  state,
  onCancel,
  onCreate,
}: {
  busy: boolean
  state: SaveState
  onCancel: (() => void) | null
  onCreate: (name: string) => Promise<boolean>
}): React.JSX.Element {
  const [name, setName] = useState('')
  const submit = (): void => {
    if (name.trim() && !busy) void onCreate(name.trim())
  }
  return (
    <>
      <header>
        <h2 className="type-title text-[var(--color-text-primary)]">New template</h2>
        <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed">
          Name it for the people it’s for. Next you’ll add the Apps it gives.
        </p>
      </header>
      <SettingsGroup title="Name" card testId="template-new">
        <SettingRow label="Template name" hint="For example Front desk, or Support team." below={<SaveError state={state} />}>
          <Input
            autoFocus
            value={name}
            aria-label="New template name"
            placeholder="Front desk"
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') submit()
            }}
            className={cx(SETTING_FIELD_WIDTH, 'type-control')}
          />
        </SettingRow>
        <div className="px-4 py-3 flex items-center justify-end gap-2">
          {onCancel ? (
            <Button variant="quiet" onClick={onCancel}>
              Cancel
            </Button>
          ) : null}
          <Button variant="accent" disabled={!name.trim() || busy} onClick={submit}>
            {busy ? 'Creating…' : 'Create template'}
          </Button>
        </div>
      </SettingsGroup>
    </>
  )
}

/** A line in words: "Shop, as Staff" for a role tied to one App, "An App
 *  you pick, as Reader" for a role that works in any. */
function lineLabel(line: TemplateLine, role: RoleRow | null, apps: AppOption[] | null): string {
  if (line.kind !== 'app') return NOTED_KIND[line.kind] ?? line.kind
  if (!role) return 'An App, with a role that no longer exists'
  return role.appId ? `${appName(role.appId, apps)}, as ${role.name}` : `An App you pick, as ${role.name}`
}

function TemplateDetail({
  template,
  people,
  roles,
  apps,
  appsError,
  created,
  refresh,
  onDeleted,
}: {
  template: TemplateRow
  people: PersonRow[]
  roles: RoleRow[]
  apps: AppOption[] | null
  appsError: string | null
  created: boolean
  refresh: () => Promise<void>
  onDeleted: () => void
}): React.JSX.Element {
  const writes = useWrites()
  const [renaming, setRenaming] = useState(false)
  const [name, setName] = useState(template.name)
  const [adding, setAdding] = useState(template.lines.length === 0)
  const [pickRole, setPickRole] = useState('')
  const roleOf = (id: string | null): RoleRow | null => (id ? roles.find((r) => r.id === id) ?? null : null)

  const rename = async (): Promise<void> => {
    const next = name.trim()
    if (!next || next === template.name) {
      setRenaming(false)
      return
    }
    if (await writes.run('name', () => daemonCliPost(primaryScope(), 'skin/templates/update', { id: template.id, name: next }))) {
      setRenaming(false)
      await refresh()
    }
  }
  const remove = async (): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Delete ${template.name}?`,
      message: 'The template goes away. People it was applied to keep everything it gave them.',
      confirmLabel: 'Delete template',
      destructive: true,
    })
    if (!ok) return
    if (await writes.run('delete', () => daemonCliPost(primaryScope(), 'skin/templates/delete', { id: template.id }))) {
      onDeleted()
      await refresh()
    }
  }
  const addLine = async (): Promise<void> => {
    if (!pickRole) return
    if (await writes.run('lines', () => daemonCliPost(primaryScope(), 'skin/templates/lines', { templateId: template.id, kind: 'app', roleId: pickRole }))) {
      setPickRole('')
      setAdding(false)
      await refresh()
    }
  }
  const removeLine = async (id: string): Promise<void> => {
    if (await writes.run('lines', () => daemonCliPost(primaryScope(), 'skin/templates/lines/delete', { id }))) await refresh()
  }

  return (
    <>
      <header className="flex items-start justify-between gap-4">
        {renaming ? (
          <div className="flex items-center gap-2 min-w-0">
            <Input
              autoFocus
              value={name}
              aria-label={`Template ${template.name} name`}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void rename()
                if (e.key === 'Escape') {
                  e.preventDefault()
                  setName(template.name)
                  setRenaming(false)
                }
              }}
              className={cx(SETTING_FIELD_WIDTH, 'type-control')}
            />
            <Button variant="quiet" onClick={() => { setName(template.name); setRenaming(false) }}>
              Cancel
            </Button>
            <Button variant="accent" disabled={!name.trim() || writes.busy === 'name'} onClick={() => void rename()}>
              Save name
            </Button>
          </div>
        ) : (
          <div className="min-w-0">
            <div className="flex items-center gap-2 min-w-0">
              <h2 className="type-title text-[var(--color-text-primary)] truncate">{template.name}</h2>
              <SaveNote state={writes.at('name')} />
            </div>
            <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed">
              {created && template.lines.length === 0
                ? 'Created. Now add the Apps it gives, then apply it to someone.'
                : 'Changing it later doesn’t change anyone it was already applied to.'}
            </p>
          </div>
        )}
        {!renaming ? (
          <OverflowMenu
            label={`More actions for ${template.name}`}
            items={[
              { id: 'rename', label: 'Rename…', onSelect: () => setRenaming(true) },
              { id: 'delete', label: 'Delete template…', danger: true, group: true, hint: 'People keep what it gave them.', onSelect: () => void remove() },
            ]}
          />
        ) : null}
      </header>
      <SaveError state={writes.at('name')} />
      <SaveError state={writes.at('delete')} />

      <SettingsGroup
        title="What it gives"
        hint="Each line is an App and the role a person gets there. The role sets which agents they reach."
        card
        testId="template-lines"
        action={
          <span className="flex items-center gap-2">
            <SaveNote state={writes.at('lines')} />
            {!adding ? <Button onClick={() => setAdding(true)}>Add an App</Button> : null}
          </span>
        }
      >
        {template.lines.map((line) => {
          const role = roleOf(line.roleId)
          return (
            <SettingRow
              key={line.id}
              label={lineLabel(line, role, apps)}
              hint={line.kind === 'app' ? (role ? reachWords(role, [], false) : 'Remove this line and add the App again with a current role.') : 'Kept as a note. K2 doesn’t enforce it.'}
              testId={`template-line-${line.id}`}
            >
              {line.kind !== 'app' ? <Tag tone="muted">Noted only</Tag> : null}
              <Button variant="quiet" aria-label={`Remove ${lineLabel(line, role, apps)}`} disabled={writes.busy === 'lines'} onClick={() => void removeLine(line.id)}>
                Remove
              </Button>
            </SettingRow>
          )
        })}
        {template.lines.length === 0 && !adding ? (
          <SettingRow label="Nothing yet" hint="Add an App and a role, and everyone you apply this to gets it.">
            {null}
          </SettingRow>
        ) : null}
        {adding ? (
          <div data-testid="template-add-line">
            <SettingRow
              label="App and role"
              hint={
                roles.length
                  ? 'A role made for one App gives that App. A role for any App lets you pick the App when you apply.'
                  : 'No roles yet. Roles are made in Settings → Apps.'
              }
              field={roles.length > 0}
            >
              {roles.length ? (
                <SettingDropdown
                  ariaLabel="Role to add"
                  value={pickRole}
                  placeholder="Choose a role"
                  fullWidth
                  options={roles.map((r) => ({ value: r.id, label: roleLabel(r, apps) }))}
                  onChange={setPickRole}
                />
              ) : (
                <Button onClick={() => useSettingsStore.getState().setSection('skin-access')}>Open Apps</Button>
              )}
            </SettingRow>
            <div className="px-4 py-3 flex items-center justify-end gap-2">
              {template.lines.length > 0 ? (
                <Button variant="quiet" onClick={() => { setAdding(false); setPickRole('') }}>
                  Cancel
                </Button>
              ) : null}
              <Button variant="accent" disabled={!pickRole || writes.busy === 'lines'} onClick={() => void addLine()}>
                {writes.busy === 'lines' ? 'Adding…' : 'Add to template'}
              </Button>
            </div>
          </div>
        ) : null}
        {typeof writes.at('lines') === 'object' ? (
          <div className="px-4 py-3">
            <SaveError state={writes.at('lines')} />
          </div>
        ) : null}
      </SettingsGroup>

      <ApplyCard template={template} people={people} roles={roles} apps={apps} appsError={appsError} refresh={refresh} />
    </>
  )
}

/** Apply: pick the person, pick an App for any role that works in any App,
 *  and give it all at once. Ends by saying what they got, with a way to see
 *  them in User Access. */
function ApplyCard({
  template,
  people,
  roles,
  apps,
  appsError,
  refresh,
}: {
  template: TemplateRow
  people: PersonRow[]
  roles: RoleRow[]
  apps: AppOption[] | null
  appsError: string | null
  refresh: () => Promise<void>
}): React.JSX.Element {
  const writes = useWrites()
  const [who, setWho] = useState('')
  const [targets, setTargets] = useState<Record<string, string>>({})
  const [scopes, setScopes] = useState<Record<string, string>>({})
  const [applied, setApplied] = useState<{ username: string; gave: string[] } | null>(null)
  const roleOf = (id: string | null): RoleRow | null => (id ? roles.find((r) => r.id === id) ?? null : null)
  const choosable = people.filter((p) => p.id)
  const person = choosable.find((p) => p.id === who) ?? null

  /** Where each line goes: fixed by its role, or picked here. */
  const targetOf = (line: TemplateLine): string => {
    if (line.kind === 'app') return roleOf(line.roleId)?.appId ?? targets[line.id] ?? ''
    return (targets[line.id] ?? '').trim()
  }
  const missing = template.lines.some((l) => !targetOf(l) || (l.kind !== 'app' && !(scopes[l.id] ?? '').trim()) || (l.kind === 'app' && !roleOf(l.roleId)))
  const canApply = Boolean(person) && template.lines.length > 0 && !missing

  const apply = async (): Promise<void> => {
    if (!person?.id) return
    const gave = template.lines.map((l) => (l.kind === 'app' ? appName(targetOf(l), apps) : `${NOTED_KIND[l.kind] ?? l.kind} ${targetOf(l)}`))
    const ok = await writes.run('apply', () =>
      daemonCliPost(primaryScope(), 'skin/templates/apply', {
        principalId: person.id,
        templateId: template.id,
        lines: template.lines.map((line) => ({
          lineId: line.id,
          targetId: targetOf(line),
          ...(line.kind === 'app' ? {} : { scope: (scopes[line.id] ?? '').trim() }),
        })),
      }),
    )
    if (ok) {
      setApplied({ username: person.username, gave })
      setWho('')
      setTargets({})
      setScopes({})
      await refresh()
    }
  }

  const appChoices = [{ value: HOST_APP, label: HOST_LABEL }, ...(apps ?? []).map((a) => ({ value: a.id, label: a.name }))]

  return (
    <SettingsGroup title="Apply to a person" hint="Gives one person everything above at once." card testId="template-apply">
      {choosable.length === 0 ? (
        <SettingRow label="No people yet" hint="Add someone in User Access first, then come back to apply this.">
          <Button onClick={() => useSettingsStore.getState().setSection('people')}>Open User Access</Button>
        </SettingRow>
      ) : (
        <>
          <SettingRow label="Person" hint="Who gets it. What they already have stays." field>
            <SettingDropdown
              ariaLabel="Apply template to"
              value={who}
              placeholder="Choose a person"
              fullWidth
              options={choosable.map((p) => ({ value: p.id as string, label: p.fullName ? `${p.fullName} (${p.username})` : p.username }))}
              onChange={(v) => {
                setWho(v)
                setApplied(null)
              }}
            />
          </SettingRow>
          {template.lines.map((line) => {
            const role = roleOf(line.roleId)
            if (line.kind === 'app') {
              if (!role || role.appId) return null
              return (
                <SettingRow
                  key={line.id}
                  label={`App for ${role.name}`}
                  hint={appsError ? undefined : `${reachWords(role)} Pick where.`}
                  below={appsError ? <p role="alert" className="type-hint text-[var(--color-status-error-soft)]">Couldn’t list the Apps: {appsError.replace(/[.\s]+$/, '')}. You can still pick {HOST_LABEL}.</p> : undefined}
                  field
                >
                  <SettingDropdown
                    ariaLabel={`App for ${role.name}`}
                    value={targets[line.id] ?? ''}
                    placeholder="Choose an App"
                    fullWidth
                    options={appChoices}
                    onChange={(v) => setTargets((prev) => ({ ...prev, [line.id]: v }))}
                  />
                </SettingRow>
              )
            }
            const kind = NOTED_KIND[line.kind] ?? line.kind
            return (
              <SettingRow key={line.id} label={`${kind} (noted only)`} hint="Its name, and the access to note down." testId={`apply-noted-${line.id}`}>
                <Input
                  value={targets[line.id] ?? ''}
                  aria-label={`${kind} name`}
                  placeholder="Name"
                  onChange={(e) => setTargets((prev) => ({ ...prev, [line.id]: e.target.value }))}
                  className="w-28 type-control"
                />
                <Input
                  value={scopes[line.id] ?? ''}
                  aria-label={`${kind} access`}
                  placeholder="Access"
                  onChange={(e) => setScopes((prev) => ({ ...prev, [line.id]: e.target.value }))}
                  className="w-28 type-control"
                />
              </SettingRow>
            )
          })}
          <SettingRow
            label={applied ? `Applied to ${applied.username}` : 'Apply'}
            hint={
              applied
                ? `${applied.username} can now use ${listWords(applied.gave)}.`
                : template.lines.length === 0
                  ? 'Add an App above first.'
                  : person
                    ? `${person.username} gets ${listWords(template.lines.map((l) => (l.kind === 'app' ? (targetOf(l) ? appName(targetOf(l), apps) : 'the App you pick') : NOTED_KIND[l.kind] ?? l.kind)))}.`
                    : 'Choose a person first.'
            }
            below={<SaveError state={writes.at('apply')} />}
            testId="template-apply-row"
          >
            {applied ? (
              <>
                <SaveNote state={writes.at('apply')} />
                <Button
                  onClick={() => {
                    showPersonNext(applied.username)
                    useSettingsStore.getState().setSection('people')
                  }}
                >
                  See {applied.username} in User Access
                </Button>
              </>
            ) : (
              <Button variant="accent" disabled={!canApply || writes.busy === 'apply'} onClick={() => void apply()}>
                {writes.busy === 'apply' ? 'Applying…' : person ? `Apply to ${person.username}` : 'Apply'}
              </Button>
            )}
          </SettingRow>
        </>
      )}
    </SettingsGroup>
  )
}
