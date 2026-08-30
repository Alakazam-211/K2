// Settings → Data (prd-workspace-data-sidecar-v1 D23) — owner surface:
// supervised Postgres sidecar, workspace databases, and sql_grants
// (manage / read / write). Mail-shaped master-detail. Thin client:
// daemon `/cli/db/*` is the only catalog. No fetchProjects in render
// paths (workspace picker reads the already-loaded store; mutations
// are optimistic). Headless CLI still works with no UI.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useWindowModeStore } from '@/stores/window-mode'
import { Toggle } from '@/components/ui'
import { SettingDropdown } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import {
  SAMPLE_DATABASES,
  SAMPLE_STATUS,
  bindSqlRole,
  createSqlDatabase,
  dbTypeLabel,
  formatSqlListen,
  disableSqlServer,
  enableSqlServer,
  fetchSqlDatabases,
  fetchSqlStatus,
  grantSqlAccess,
  revokeSqlAccess,
  setSqlDbAgentAccess,
  sqlErrorMessage,
  type SqlDatabase,
  type SqlLevel,
  type SqlStatus,
} from './data-api'

export const DATA_MANIFEST: SettingEntry[] = [
  {
    id: 'data.server',
    section: 'data',
    label: 'Database sidecar',
    description: 'Enable and supervise Postgres on Linux deployments',
    keywords: ['sql', 'postgres', 'database', 'sidecar', 'db', 'enable'],
  },
  {
    id: 'data.databases',
    section: 'data',
    label: 'Databases',
    description: 'Per-workspace SQL databases (documents in the same DB)',
    keywords: ['database', 'sql', 'jsonb', 'documents', 'workspace', 'cap'],
  },
  {
    id: 'data.grants',
    section: 'data',
    label: 'Database access',
    description: 'Grant other workspaces manage / read / write',
    keywords: ['grant', 'access', 'read', 'write', 'manage', 'workspace'],
  },
  {
    id: 'data.bind',
    section: 'data',
    label: 'Bind role',
    description: 'Postgres role the workspace assistant uses',
    keywords: ['bind', 'role', 'pg_role', 'rls', 'agent'],
  },
  {
    id: 'data.agent-create',
    section: 'data',
    label: 'Agents can create databases',
    description: 'Allow the owning workspace agent to run k2 db create',
    keywords: ['db_agent_access', 'create', 'passport', 'agent', 'write'],
  },
]

const MAC_BANNER =
  'THIS FEATURE ONLY WORKS ON LINUX DEPLOYMENTS, THIS PAGE IS JUST HERE FOR EXAMPLE PURPOSES.'

type Selection = { kind: 'server' } | { kind: 'db'; id: string }

function SectionTitle({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
      {children}
    </h3>
  )
}

function stateColor(state: string): string {
  switch (state) {
    case 'running':
      return 'var(--color-status-ok)'
    case 'degraded':
      return 'var(--color-status-warn)'
    case 'installing':
      return 'var(--color-status-working)'
    case 'error':
      return 'var(--color-status-error-soft)'
    default:
      return 'var(--color-text-muted)'
  }
}

const LEVEL_LABEL: Record<SqlLevel, string> = {
  read: 'Read',
  write: 'Read + Write',
}

function AccessPanel({
  db,
  canMutate,
  patchDb,
}: {
  db: SqlDatabase
  canMutate: boolean
  patchDb: (id: string, patch: (row: SqlDatabase) => SqlDatabase) => void
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const [error, setError] = useState<string | null>(null)
  const [pending, setPending] = useState<string | null>(null)
  const [newProject, setNewProject] = useState('')
  const [newLevel, setNewLevel] = useState<SqlLevel>('read')

  const grantedIds = useMemo(() => new Set(db.grants.map((g) => g.projectId)), [db.grants])
  const projectName = useCallback(
    (id: string): string => projects.find((p) => p.id === id)?.name ?? id,
    [projects],
  )
  const addOptions = useMemo(
    () =>
      projects
        .filter((p) => p.id !== db.ownerProjectId && !grantedIds.has(p.id))
        .map((p) => ({ value: p.id, label: p.name || p.path })),
    [projects, db.ownerProjectId, grantedIds],
  )

  const changeGrant = useCallback(
    async (projectId: string, level: SqlLevel, canManage: boolean): Promise<void> => {
      setError(null)
      setPending(projectId)
      const before = db.grants
      patchDb(db.id, (row) => ({
        ...row,
        grants: row.grants.map((g) =>
          g.projectId === projectId ? { ...g, level, canManage } : g,
        ),
      }))
      try {
        await grantSqlAccess({ project: projectId, db: db.id, level, manage: canManage })
      } catch (e) {
        patchDb(db.id, (row) => ({ ...row, grants: before }))
        setError(sqlErrorMessage(e))
      } finally {
        setPending(null)
      }
    },
    [db.grants, db.id, patchDb],
  )

  const addGrant = useCallback(async (): Promise<void> => {
    if (!newProject) return
    setError(null)
    setPending('add')
    const workspace = projects.find((p) => p.id === newProject)?.name ?? null
    const added = {
      projectId: newProject,
      workspace,
      level: newLevel,
      canManage: false,
    }
    const before = db.grants
    patchDb(db.id, (row) => ({ ...row, grants: [...row.grants, added] }))
    try {
      await grantSqlAccess({ project: newProject, db: db.id, level: newLevel })
      setNewProject('')
      setNewLevel('read')
    } catch (e) {
      patchDb(db.id, (row) => ({ ...row, grants: before }))
      setError(sqlErrorMessage(e))
    } finally {
      setPending(null)
    }
  }, [db.grants, db.id, newLevel, newProject, patchDb, projects])

  const removeGrant = useCallback(
    async (projectId: string): Promise<void> => {
      setError(null)
      setPending(projectId)
      const before = db.grants
      patchDb(db.id, (row) => ({
        ...row,
        grants: row.grants.filter((g) => g.projectId !== projectId),
      }))
      try {
        await revokeSqlAccess({ project: projectId, db: db.id })
      } catch (e) {
        patchDb(db.id, (row) => ({ ...row, grants: before }))
        setError(sqlErrorMessage(e))
      } finally {
        setPending(null)
      }
    },
    [db.grants, db.id, patchDb],
  )

  const ownerProject = projects.find((p) => p.id === db.ownerProjectId)
  const ownerPath = ownerProject?.path
  const agentCanCreate = db.dbAgentAccess === 'write'

  const toggleAgentCreate = useCallback(
    async (next: boolean): Promise<void> => {
      if (!ownerPath) return
      setError(null)
      const before = db.dbAgentAccess
      patchDb(db.id, (row) => ({ ...row, dbAgentAccess: next ? 'write' : 'off' }))
      try {
        await setSqlDbAgentAccess(ownerPath, next)
      } catch (e) {
        patchDb(db.id, (row) => ({ ...row, dbAgentAccess: before }))
        setError(sqlErrorMessage(e))
      }
    },
    [db.dbAgentAccess, db.id, ownerPath, patchDb],
  )

  return (
    <div className="space-y-3" data-settings-id="data.grants">
      <SectionTitle>Access</SectionTitle>
      <div
        className="border border-[var(--color-border)] px-3 py-2 space-y-1.5"
        data-settings-id="data.agent-create"
      >
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <p className="text-xs text-[var(--color-text-primary)]">Agents can create databases</p>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
              Existing databases this workspace owns or is granted are usable without this toggle;
              it only gates <span className="font-mono">k2 db create</span>.
            </p>
          </div>
          <Toggle
            checked={agentCanCreate}
            disabled={!canMutate || !ownerPath}
            onChange={(next) => void toggleAgentCreate(next)}
            aria-label="Agents can create databases"
          />
        </div>
      </div>
      <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
        <div className="px-3 py-2 space-y-1.5">
          <div className="flex items-center gap-2 min-w-0">
            <span className="text-[10px] font-semibold uppercase tracking-wide text-[var(--color-text-muted)]">
              Owner
            </span>
            <span className="text-xs text-[var(--color-text-primary)] truncate">
              {db.owner.workspace ?? projectName(db.owner.projectId)}
            </span>
            <span className="flex-1" />
            <span className="text-[10px] text-[var(--color-text-muted)]">
              {LEVEL_LABEL.write} · manage
            </span>
          </div>
        </div>
        {db.grants.map((g) => (
          <div key={g.projectId} className="px-3 py-2 space-y-1.5">
            <div className="flex items-center gap-2 min-w-0">
              <span className="text-xs text-[var(--color-text-primary)] truncate flex-1">
                {g.workspace ?? projectName(g.projectId)}
              </span>
              <SettingDropdown
                value={g.level}
                options={[
                  { value: 'read', label: LEVEL_LABEL.read },
                  { value: 'write', label: LEVEL_LABEL.write },
                ]}
                onChange={(next) => {
                  if (!canMutate || pending === g.projectId) return
                  void changeGrant(g.projectId, next as SqlLevel, g.canManage)
                }}
              />
              <button
                type="button"
                disabled={!canMutate || pending === g.projectId}
                onClick={() => void removeGrant(g.projectId)}
                className="px-2 py-0.5 text-[10px] text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
              >
                Remove
              </button>
            </div>
            <div className="flex items-center justify-between gap-2">
              <span className="text-[11px] text-[var(--color-text-secondary)]">Allow manage</span>
              <Toggle
                checked={g.canManage}
                disabled={!canMutate || pending === g.projectId}
                onChange={(next) => void changeGrant(g.projectId, g.level, next)}
                aria-label="Allow manage"
              />
            </div>
          </div>
        ))}
        {addOptions.length > 0 && (
          <div className="px-3 py-2 flex items-center gap-2">
            <SettingDropdown
              value={newProject}
              placeholder="Add workspace…"
              options={[{ value: '', label: 'Add workspace…' }, ...addOptions]}
              onChange={(v) => setNewProject(v)}
            />
            <SettingDropdown
              value={newLevel}
              options={[
                { value: 'read', label: LEVEL_LABEL.read },
                { value: 'write', label: LEVEL_LABEL.write },
              ]}
              onChange={(v) => setNewLevel(v as SqlLevel)}
            />
            <button
              type="button"
              disabled={!canMutate || !newProject || pending === 'add'}
              onClick={() => void addGrant()}
              className="px-2 py-0.5 text-[10px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
            >
              Grant
            </button>
          </div>
        )}
      </div>
      {error && (
        <p className="text-[11px] text-[var(--color-status-error-soft)] break-words">{error}</p>
      )}
    </div>
  )
}

function BindPanel({
  db,
  canMutate,
  patchDb,
}: {
  db: SqlDatabase
  canMutate: boolean
  patchDb: (id: string, patch: (row: SqlDatabase) => SqlDatabase) => void
}): React.JSX.Element {
  const [draft, setDraft] = useState(db.bindRole ?? '')
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<string | null>(null)

  useEffect(() => {
    setDraft(db.bindRole ?? '')
  }, [db.bindRole, db.id])

  const save = useCallback(async (): Promise<void> => {
    const role = draft.trim()
    if (!role) return
    setBusy(true)
    setNote(null)
    const before = db.bindRole
    patchDb(db.id, (row) => ({ ...row, bindRole: role }))
    try {
      const res = await bindSqlRole({
        project: db.ownerProjectId,
        db: db.id,
        role,
      })
      if (res.bindRole) patchDb(db.id, (row) => ({ ...row, bindRole: res.bindRole ?? role }))
      setNote('Saved.')
    } catch (e) {
      patchDb(db.id, (row) => ({ ...row, bindRole: before }))
      setNote(sqlErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }, [db.bindRole, db.id, db.ownerProjectId, draft, patchDb])

  return (
    <div className="space-y-2" data-settings-id="data.bind">
      <SectionTitle>Bind role</SectionTitle>
      <p className="text-[11px] text-[var(--color-text-muted)]">
        Postgres role the workspace assistant uses. Default is{' '}
        <span className="font-mono">ws_&lt;id&gt;_agent</span>. Does not mint RLS at spawn.
      </p>
      <div className="flex items-center gap-2">
        <input
          type="text"
          value={draft}
          disabled={!canMutate || busy}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="ws_<workspace>_agent"
          className="flex-1 min-w-0 px-2 py-1.5 text-xs font-mono bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag disabled:opacity-50"
        />
        <button
          type="button"
          disabled={!canMutate || busy || !draft.trim()}
          onClick={() => void save()}
          className="px-2.5 py-1 text-[10px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
        >
          {busy ? 'Saving…' : 'Bind'}
        </button>
      </div>
      {note && (
        <p
          className={`text-[10px] ${
            note === 'Saved.'
              ? 'text-[var(--color-text-muted)]'
              : 'text-[var(--color-status-error-soft)]'
          }`}
        >
          {note}
        </p>
      )}
    </div>
  )
}

function ServerPanel({
  status,
  canMutate,
  sample,
  onChanged,
}: {
  status: SqlStatus
  canMutate: boolean
  sample: boolean
  onChanged: () => void
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const notInstalled = status.state === 'not-installed'
  const stopped = status.state === 'disabled' || status.state === 'stopped'
  const listenLine = formatSqlListen(status.listen, status.port)

  const doEnable = useCallback(async (): Promise<void> => {
    if (sample) return
    setBusy(true)
    setError(null)
    try {
      await enableSqlServer()
      onChanged()
    } catch (e) {
      setError(sqlErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }, [onChanged, sample])

  const doDisable = useCallback(async (): Promise<void> => {
    if (sample) return
    setBusy(true)
    setError(null)
    try {
      await disableSqlServer()
      onChanged()
    } catch (e) {
      setError(sqlErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }, [onChanged, sample])

  return (
    <div className="grid gap-6 grid-cols-[minmax(0,42rem)]">
      <div className="min-w-0" data-settings-id="data.server">
        <h2 className="text-base font-medium text-[var(--color-text-primary)]">Database sidecar</h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          K2 supervises a Postgres engine on this box (one postmaster, loopback only). Agents mint
          a per-workspace database with <span className="font-mono">k2 db create</span>; documents
          live as JSONB in the same DB. Files stay in the workspace filesystem.
        </p>
      </div>

      <div className="space-y-2">
        <SectionTitle>Status</SectionTitle>
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          <div className="flex items-center gap-2 px-3 py-2">
            <span
              className="w-2 h-2 rounded-full flex-shrink-0"
              style={{ backgroundColor: stateColor(status.state) }}
            />
            <span className="text-xs text-[var(--color-text-primary)]">{status.state}</span>
            <span className="flex-1" />
            {status.installedMajor != null && (
              <span className="text-[10px] text-[var(--color-text-muted)]">
                PG {status.installedMajor}
              </span>
            )}
          </div>
          {listenLine && (
            <div className="flex items-center gap-3 px-3 py-2">
              <span className="text-[11px] text-[var(--color-text-secondary)] w-24 flex-shrink-0">
                Listen
              </span>
              <span className="text-xs font-mono text-[var(--color-text-primary)]">
                {listenLine}
              </span>
            </div>
          )}
          {status.publishHint && (
            <div className="px-3 py-2">
              <p className="text-[11px] text-[var(--color-text-muted)] break-words font-mono">
                {status.publishHint}
              </p>
            </div>
          )}
          {status.lastError && (
            <div className="px-3 py-2">
              <p className="text-[11px] text-[var(--color-status-error-soft)] break-words">
                {status.lastError}
              </p>
            </div>
          )}
        </div>
      </div>

      {(notInstalled || stopped) && (
        <div className="space-y-2">
          <SectionTitle>{notInstalled ? 'Enable' : 'Resume'}</SectionTitle>
          <p className="text-[11px] text-[var(--color-text-muted)]">
            Bake first with <span className="font-mono">provision-k2-server.sh --bake --with-db</span>
            . Enable is catalog + start — the daemon never apt-gets.
          </p>
          <button
            type="button"
            disabled={!canMutate || busy}
            onClick={() => void doEnable()}
            className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {busy ? 'Starting…' : 'Enable database sidecar'}
          </button>
        </div>
      )}

      {status.state === 'running' && (
        <div className="space-y-2 pb-6">
          <SectionTitle>Danger zone</SectionTitle>
          <div className="border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] px-3 py-2 flex items-center gap-3">
            <div className="flex-1 min-w-0">
              <p className="text-xs text-[var(--color-text-primary)]">Disable</p>
              <p className="text-[10px] text-[var(--color-text-muted)]">
                Stops the unit, keeps PGDATA.
              </p>
            </div>
            <button
              type="button"
              disabled={!canMutate || busy}
              onClick={() => void doDisable()}
              className="px-2.5 py-1 text-[10px] font-medium text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
            >
              Disable
            </button>
          </div>
        </div>
      )}

      {error && (
        <p className="text-[11px] text-[var(--color-status-error-soft)] break-words">{error}</p>
      )}
    </div>
  )
}

function DatabasePanel({
  db,
  canMutate,
  patchDb,
}: {
  db: SqlDatabase
  canMutate: boolean
  patchDb: (id: string, patch: (row: SqlDatabase) => SqlDatabase) => void
}): React.JSX.Element {
  return (
    <div className="grid gap-6 grid-cols-[minmax(0,44rem)]">
      <div className="min-w-0" data-settings-id="data.databases">
        <h2 className="text-base font-medium text-[var(--color-text-primary)] font-mono truncate">
          {db.name}
        </h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          {dbTypeLabel(db)} · owner {db.ownerWorkspace ?? db.ownerProjectId} · {db.status}
          {db.cap ? ` · cap ${db.cap.used}/${db.cap.cap === 0 ? '∞' : db.cap.cap}` : ''}
        </p>
        {db.bindRole && (
          <p className="text-[11px] text-[var(--color-text-secondary)] mt-0.5 font-mono">
            bind {db.bindRole}
          </p>
        )}
      </div>
      <AccessPanel db={db} canMutate={canMutate} patchDb={patchDb} />
      <BindPanel db={db} canMutate={canMutate} patchDb={patchDb} />
    </div>
  )
}

export function DataSection(): React.JSX.Element {
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const projects = useProjectsStore((s) => s.projects)

  const [status, setStatus] = useState<SqlStatus | null>(null)
  const [statusError, setStatusError] = useState<string | null>(null)
  const [databases, setDatabases] = useState<SqlDatabase[] | null>(null)
  const [listError, setListError] = useState<string | null>(null)
  const [selection, setSelection] = useState<Selection>({ kind: 'server' })
  const [revision, setRevision] = useState(0)
  const bump = useCallback(() => setRevision((r) => r + 1), [])

  const [createWs, setCreateWs] = useState('')
  const [createBusy, setCreateBusy] = useState(false)
  const [createError, setCreateError] = useState<string | null>(null)

  const sample = status !== null && !status.supported
  const canMutate = status !== null && status.supported && !viewerReadOnly
  const supported: boolean | null = status === null ? null : status.supported

  useEffect(() => {
    let cancelled = false
    fetchSqlStatus()
      .then((s) => {
        if (cancelled) return
        setStatus(s)
        setStatusError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setStatusError(sqlErrorMessage(e))
      })
    return () => {
      cancelled = true
    }
  }, [revision])

  useEffect(() => {
    if (supported === null) return
    if (!supported) {
      setDatabases(SAMPLE_DATABASES)
      setListError(null)
      return
    }
    let cancelled = false
    fetchSqlDatabases()
      .then((rows) => {
        if (cancelled) return
        setDatabases(rows)
        setListError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setDatabases([])
        setListError(sqlErrorMessage(e))
      })
    return () => {
      cancelled = true
    }
  }, [supported, revision])

  useEffect(() => {
    if (selection.kind !== 'db' || databases === null) return
    if (!databases.some((d) => d.id === selection.id)) {
      setSelection({ kind: 'server' })
    }
  }, [databases, selection])

  const patchDb = useCallback((id: string, patch: (row: SqlDatabase) => SqlDatabase): void => {
    setDatabases((prev) => prev?.map((row) => (row.id === id ? patch(row) : row)) ?? null)
  }, [])

  const createOptions = useMemo(
    () => projects.map((p) => ({ value: p.id, label: p.name || p.path })),
    [projects],
  )

  const submitCreate = useCallback(async (): Promise<void> => {
    if (!createWs || createBusy || sample) return
    setCreateBusy(true)
    setCreateError(null)
    try {
      const res = await createSqlDatabase(createWs)
      const owner = projects.find((p) => p.id === createWs)
      const optimistic: SqlDatabase = {
        id: `tmp_${Date.now()}`,
        name: res.name ?? 'ws_new',
        status: 'active',
        createdAt: Math.floor(Date.now() / 1000),
        type: 'sql',
        documents: true,
        ownerProjectId: createWs,
        ownerWorkspace: owner?.name ?? null,
        bindRole: null,
        cap: { used: 1, cap: 1 },
        owner: {
          projectId: createWs,
          workspace: owner?.name ?? null,
          level: 'write',
          canManage: true,
        },
        grants: [],
        yourLevel: 'write',
        dbAgentAccess: 'off',
      }
      setDatabases((prev) => [...(prev ?? []), optimistic])
      setCreateWs('')
      bump()
    } catch (e) {
      setCreateError(sqlErrorMessage(e))
    } finally {
      setCreateBusy(false)
    }
  }, [bump, createBusy, createWs, projects, sample])

  if (statusError) {
    return (
      <div className="p-6 space-y-2">
        <p className="text-[11px] text-[var(--color-status-error-soft)]">
          Couldn&rsquo;t read the database sidecar status: {statusError}
        </p>
        <button
          type="button"
          onClick={bump}
          className="px-2.5 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
        >
          Retry
        </button>
      </div>
    )
  }
  if (status === null) {
    return <p className="p-6 text-xs text-[var(--color-text-muted)]">Loading…</p>
  }

  const displayStatus = sample ? { ...SAMPLE_STATUS, ...status, supported: false } : status
  const selectedDb =
    selection.kind === 'db' ? (databases ?? []).find((d) => d.id === selection.id) : undefined

  return (
    <div className="flex flex-col h-full min-h-0">
      {sample && (
        <div className="flex-shrink-0 px-4 py-2 text-[10px] font-bold tracking-wide text-center text-[var(--color-status-warn-text)] bg-[color-mix(in_srgb,var(--color-status-warn)_18%,transparent)] border-b border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)]">
          {MAC_BANNER}
        </div>
      )}

      <div className="flex flex-1 min-h-0">
        <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col min-h-0">
          <button
            type="button"
            onClick={() => setSelection({ kind: 'server' })}
            className={`text-left px-3 py-2.5 border-b border-[var(--color-border)] transition-colors no-drag cursor-pointer ${
              selection.kind === 'server'
                ? 'bg-[var(--color-accent)]/10'
                : 'hover:bg-[var(--color-bg-hover)]'
            }`}
          >
            <div className="flex items-center gap-2">
              <span
                className="w-2 h-2 rounded-full flex-shrink-0"
                style={{ backgroundColor: stateColor(displayStatus.state) }}
              />
              <span className="text-xs font-medium text-[var(--color-text-primary)]">
                Database sidecar
              </span>
            </div>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 truncate">
              {displayStatus.state}
              {displayStatus.installedMajor != null ? ` · PG ${displayStatus.installedMajor}` : ''}
            </p>
          </button>

          <div className="px-3 pt-3 pb-1">
            <SectionTitle>Databases</SectionTitle>
          </div>
          <div className="flex-1 overflow-y-auto px-1 py-1">
            {databases === null ? (
              <p className="px-2 py-1 text-[11px] text-[var(--color-text-muted)]">Loading…</p>
            ) : listError ? (
              <p className="px-2 py-1 text-[11px] text-[var(--color-status-error-soft)] break-words">
                {listError}
              </p>
            ) : databases.length === 0 ? (
              <p className="px-2 py-1 text-[11px] text-[var(--color-text-muted)] italic">
                No databases yet.
              </p>
            ) : (
              databases.map((d) => {
                const isSelected = selection.kind === 'db' && selection.id === d.id
                return (
                  <button
                    key={d.id}
                    type="button"
                    onClick={() => setSelection({ kind: 'db', id: d.id })}
                    className={`w-full flex flex-col items-start gap-0.5 px-2 py-1.5 text-left transition-colors no-drag cursor-pointer min-w-0 ${
                      isSelected
                        ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                        : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)]'
                    }`}
                  >
                    <span className="text-xs font-mono truncate w-full">{d.name}</span>
                    <span className="text-[10px] text-[var(--color-text-muted)] truncate w-full">
                      {dbTypeLabel(d)} · {d.ownerWorkspace ?? 'workspace'} · {d.status}
                      {d.cap ? ` · ${d.cap.used}/${d.cap.cap === 0 ? '∞' : d.cap.cap}` : ''}
                    </span>
                  </button>
                )
              })
            )}
          </div>

          <div className="border-t border-[var(--color-border)] px-2 py-2 space-y-1.5">
            <p className="text-[10px] text-[var(--color-text-muted)] px-1">Create database</p>
            <SettingDropdown
              value={createWs}
              placeholder="Workspace…"
              options={[{ value: '', label: 'Workspace…' }, ...createOptions]}
              onChange={(v) => setCreateWs(v)}
              menuAlign="left"
              menuPlacement="up"
            />
            <button
              type="button"
              disabled={!canMutate || !createWs || createBusy}
              onClick={() => void submitCreate()}
              className="w-full px-2 py-1 text-[10px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {createBusy ? 'Creating…' : 'Create'}
            </button>
            {createError && (
              <p className="text-[10px] text-[var(--color-status-error-soft)] break-words px-1">
                {createError}
              </p>
            )}
          </div>
        </div>

        <div className="flex-1 min-w-0 overflow-y-auto p-6">
          {selection.kind === 'server' && (
            <ServerPanel
              status={displayStatus}
              canMutate={canMutate}
              sample={sample}
              onChanged={bump}
            />
          )}
          {selection.kind === 'db' && selectedDb && (
            <DatabasePanel db={selectedDb} canMutate={canMutate} patchDb={patchDb} />
          )}
        </div>
      </div>
    </div>
  )
}
