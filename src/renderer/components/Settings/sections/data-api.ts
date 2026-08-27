// Settings → Data (prd-workspace-data-sidecar-v1 D23) — typed renderer
// bindings for `/cli/db/*`. Thin client: the daemon catalog is the only
// source of truth. When GET /cli/db/status reports `supported: false`
// the page renders from SAMPLE_* (Linux banner) and makes no further
// network calls. Rows NEVER carry DSNs, passwords, or secret refs.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'

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
  return daemonCliGet<SqlStatus>('db/status')
}

export async function fetchSqlDatabases(): Promise<SqlDatabase[]> {
  const res = await daemonCliGet<{ ok: boolean; databases: SqlDatabase[] }>('db/list')
  return Array.isArray(res?.databases) ? res.databases : []
}

export async function enableSqlServer(): Promise<{ ok: boolean; state?: string }> {
  return daemonCliPost('db/server/enable', {})
}

export async function disableSqlServer(): Promise<{ ok: boolean; state?: string }> {
  return daemonCliPost('db/server/disable', {})
}

export async function createSqlDatabase(project: string): Promise<{
  ok: boolean
  name?: string
  existing?: boolean
}> {
  return daemonCliPost('db/create', { project })
}

export async function grantSqlAccess(body: {
  project: string
  db: string
  level: SqlLevel
  manage?: boolean
}): Promise<{ ok: boolean }> {
  return daemonCliPost('db/grant', body)
}

export async function revokeSqlAccess(body: {
  project: string
  db: string
}): Promise<{ ok: boolean }> {
  return daemonCliPost('db/revoke', body)
}

export async function bindSqlRole(body: {
  project: string
  db?: string
  role: string
}): Promise<{ ok: boolean; bindRole?: string }> {
  return daemonCliPost('db/bind', body)
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
  },
]
