// Settings → Data (prd-workspace-data-sidecar-v1 D23) — typed renderer
// bindings for `/cli/db/*`. Thin client: the daemon catalog is the only
// source of truth. When GET /cli/db/status reports `supported: false`
// the page renders from SAMPLE_* (Linux banner) and makes no further
// network calls. Rows NEVER carry DSNs, passwords, or secret refs.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export interface SqlStatus {
  ok: boolean
  /** From the DAEMON (Linux = true) — NEVER navigator.platform. */
  supported: boolean
  state: string
  installedMajor: number | null
  listen: string | null
  /** Loopback port (5432 unless catalog says otherwise). */
  port?: number
  /** Off-box recipe: k2 publish subdomain … --target localhost:<port>. */
  publishHint?: string | null
  lastError: string | null
  enableProgress?: unknown
  health?: unknown
  /** Postgres majors side by side (0.46.3): one cluster per major. */
  clusters?: SqlCluster[]
  /** Each database's major (by catalog id). */
  databases?: { id: string; name: string; major: number | null }[]
  /** Databases on a major with a newer supported one. */
  upgradeAvailable?: { id: string; name: string; from: number; to: number[]; check: string }[]
  /** What `k2 db create` uses without --major. */
  defaultMajor?: number
  defaultMajorPinned?: boolean
  /** `set` (k2 db default-major) | `primary` (the 5432 cluster) | `new-install`. */
  defaultMajorSource?: 'set' | 'primary' | 'new-install'
}

export interface SqlCluster {
  major: number
  name: string
  port: number
  unit: string
  role: string
  state: string
}

/** "PG 16 · upgrade available" / "PG 17" for a database, from status. */
export function dbMajorChip(status: Pick<SqlStatus, 'databases' | 'upgradeAvailable'> | null | undefined, dbId: string): string | null {
  const row = status?.databases?.find((d) => d.id === dbId)
  if (!row || row.major == null) return null
  const up = status?.upgradeAvailable?.some((u) => u.id === dbId)
  return up ? `PG ${row.major} · upgrade available` : `PG ${row.major}`
}

/** "PG 16 :5432 · PG 17 :5433" (the status card), or null with one or none. */
export function clustersLine(status: Pick<SqlStatus, 'clusters'> | null | undefined): string | null {
  const cl = status?.clusters ?? []
  if (cl.length < 2) return null
  return cl.map((c) => `PG ${c.major} :${c.port}`).join(' · ')
}

export type SqlLevel = 'read' | 'write'

export interface SqlParticipant {
  projectId: string
  workspace: string | null
  level: SqlLevel
  canManage: boolean
}

export interface SqlDatabase {
  id: string
  name: string
  status: string
  createdAt: number
  /** Always `sql` in v1 — documents live in the same DB. */
  type: 'sql'
  documents: boolean
  ownerProjectId: string
  ownerWorkspace: string | null
  bindRole: string | null
  cap: { used: number; cap: number }
  owner: SqlParticipant
  grants: SqlParticipant[]
  yourLevel: SqlLevel | null
  /** Owner workspace create-only passport (`off`/`read`/`write`). */
  dbAgentAccess?: 'off' | 'read' | 'write'
}

export function sqlErrorInfo(err: unknown): { code?: string; hint?: string } {
  const msg = err instanceof Error ? err.message : String(err)
  try {
    const parsed = JSON.parse(msg)
    const e = parsed?.error
    if (e && typeof e === 'object') {
      return {
        code: typeof e.code === 'string' ? e.code : undefined,
        hint: typeof e.hint === 'string' ? e.hint : undefined,
      }
    }
  } catch {
    /* not the JSON error shape */
  }
  return {}
}

export function sqlErrorMessage(err: unknown): string {
  const { hint } = sqlErrorInfo(err)
  if (hint) return hint
  return err instanceof Error ? err.message : String(err)
}

export function dbTypeLabel(row: Pick<SqlDatabase, 'type' | 'documents'>): string {
  if (row.type === 'sql' && row.documents) return 'sql / documents-in-same-DB'
  return row.type
}

/** Loopback listen line. Catalog `listen` is `localhost` or `localhost:<port>` — never append port twice. */
export function formatSqlListen(listen: string | null | undefined, port?: number): string | null {
  if (!listen) return null
  if (listen.includes(':')) return listen
  if (port != null) return `${listen}:${port}`
  return listen
}

export async function fetchSqlStatus(): Promise<SqlStatus> {
  return daemonCliGet<SqlStatus>(primaryScope(), 'db/status')
}

export async function fetchSqlDatabases(): Promise<SqlDatabase[]> {
  const res = await daemonCliGet<{ ok: boolean; databases: SqlDatabase[] }>(primaryScope(), 'db/list')
  return Array.isArray(res?.databases) ? res.databases : []
}

export async function enableSqlServer(): Promise<{ ok: boolean; state?: string }> {
  return daemonCliPost(primaryScope(), 'db/server/enable', {})
}

export async function disableSqlServer(): Promise<{ ok: boolean; state?: string }> {
  return daemonCliPost(primaryScope(), 'db/server/disable', {})
}

export async function createSqlDatabase(project: string): Promise<{
  ok: boolean
  name?: string
  existing?: boolean
}> {
  return daemonCliPost(primaryScope(), 'db/create', { project })
}

export async function grantSqlAccess(body: {
  project: string
  db: string
  level: SqlLevel
  manage?: boolean
}): Promise<{ ok: boolean }> {
  return daemonCliPost(primaryScope(), 'db/grant', body)
}

export async function revokeSqlAccess(body: {
  project: string
  db: string
}): Promise<{ ok: boolean }> {
  return daemonCliPost(primaryScope(), 'db/revoke', body)
}

export async function bindSqlRole(body: {
  project: string
  db?: string
  role: string
}): Promise<{ ok: boolean; bindRole?: string }> {
  return daemonCliPost(primaryScope(), 'db/bind', body)
}

/** Owner/admin: agents may create new DBs on this owning workspace. */
export async function setSqlDbAgentAccess(
  ownerPath: string,
  canCreate: boolean,
): Promise<void> {
  await daemonCliPost(primaryScope(), 'workspace/set', {
    project: ownerPath,
    fields: { db_agent_access: canCreate ? 'write' : 'off' },
  })
}

/** "Allow this agent to manage Postgres versions" (`db_upgrade`, 0201):
 *  a local agent from the workspace may move its own (and its Apps')
 *  databases to a newer Postgres, create on a chosen version and roll
 *  back; deleting the old copy stays the owner's. The daemon lets the
 *  owner raise it and the owner or an Admin lower it; an agent never. */
export interface DbUpgradeSetResult {
  ok: boolean
  workspace?: string
  toggle?: string
  agentsCanUpgradePostgres?: boolean
}

export async function setWorkspaceDbUpgrade(workspace: string, value: boolean): Promise<DbUpgradeSetResult> {
  return daemonCliPost(primaryScope(), 'agent-access/set', { toggle: 'db_upgrade', value, workspace })
}

// ── Sample fixture (unsupported / Mac example mode) ──────────────────

export const SAMPLE_STATUS: SqlStatus = {
  ok: true,
  supported: false,
  state: 'running',
  installedMajor: 16,
  listen: 'localhost',
  port: 5432,
  publishHint:
    'off-box *.k2.dev: k2 publish subdomain create <label> --target localhost:5432 (port already listening — do not publish run Postgres)',
  lastError: null,
}

export const SAMPLE_DATABASES: SqlDatabase[] = [
  {
    id: 'db_sales',
    name: 'ws_sales',
    status: 'active',
    createdAt: 1751700000,
    type: 'sql',
    documents: true,
    ownerProjectId: 'p_sales',
    ownerWorkspace: 'sales',
    bindRole: 'ws_sales_agent',
    cap: { used: 1, cap: 1 },
    owner: {
      projectId: 'p_sales',
      workspace: 'sales',
      level: 'write',
      canManage: true,
    },
    grants: [
      {
        projectId: 'p_research',
        workspace: 'research',
        level: 'read',
        canManage: false,
      },
    ],
    yourLevel: 'write',
    dbAgentAccess: 'off',
  },
  {
    id: 'db_ops',
    name: 'ws_ops',
    status: 'active',
    createdAt: 1751710000,
    type: 'sql',
    documents: true,
    ownerProjectId: 'p_ops',
    ownerWorkspace: 'ops',
    bindRole: 'ops_app',
    cap: { used: 1, cap: 1 },
    owner: {
      projectId: 'p_ops',
      workspace: 'ops',
      level: 'write',
      canManage: true,
    },
    grants: [],
    yourLevel: 'write',
    dbAgentAccess: 'off',
  },
]
