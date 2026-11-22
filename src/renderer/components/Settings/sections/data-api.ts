// Settings → Data (prd-workspace-data-sidecar-v1 D23) — typed renderer
// bindings for `/cli/db/*`. Thin client: the daemon catalog is the only
// source of truth. When GET /cli/db/status reports `supported: false`
// the page renders from SAMPLE_* (Linux banner) and makes no further
// network calls. Rows NEVER carry DSNs, passwords, or secret refs.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { CLI_NAME } from '@/lib/brand'

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
  // daemon-cli's DaemonHttpError already unpacked `{ok:false, error:{code,
  // hint}}` (the db routes' shape): the code rides on the error, the hint
  // is its message. Read that first; the JSON-in-message path is for
  // callers that throw the raw body.
  const carried = err as { code?: unknown; message?: unknown } | null
  if (carried && typeof carried === 'object' && typeof carried.code === 'string' && carried.code !== '') {
    return { code: carried.code, hint: typeof carried.message === 'string' && carried.message !== '' ? carried.message : undefined }
  }
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

/** The daemon refused on permission, not on a passing fault: the db routes
 *  answer 403 with `forbidden` (sql::ops OpsError::Forbidden, e.g. a grant
 *  by a workspace that doesn't own or manage the database) or `owner_only`
 *  (sql_routes::owner_only_response). Trying again won't help. */
export function sqlRefused(err: unknown): boolean {
  const { code } = sqlErrorInfo(err)
  return code === 'forbidden' || code === 'owner_only'
}

/** Enable refused because Postgres isn't on this server (sql::supervisor
 *  EnableError::NotBaked: enable never installs it). */
export function sqlNotBaked(err: unknown): boolean {
  return sqlErrorInfo(err).code === 'not_baked'
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

/** GET /cli/db/status, checked: the daemon always sends `supported` and
 *  `state` (sql::supervisor::status_json). A reply without them is not a
 *  status, so it throws (the page shows a failed-read card with Try again)
 *  instead of rendering an empty one. Optional fields are normalised. */
export function parseSqlStatus(raw: unknown): SqlStatus {
  const v = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : null
  if (!v || typeof v.supported !== 'boolean' || typeof v.state !== 'string' || !v.state) {
    throw new Error('the reply had no server state')
  }
  const str = (x: unknown): string | null => (typeof x === 'string' && x ? x : null)
  return {
    ...(v as unknown as SqlStatus),
    ok: v.ok !== false,
    supported: v.supported,
    state: v.state,
    installedMajor: num(v.installedMajor),
    listen: str(v.listen),
    port: num(v.port) ?? undefined,
    publishHint: str(v.publishHint),
    lastError: str(v.lastError),
    clusters: Array.isArray(v.clusters) ? (v.clusters as SqlCluster[]).filter((c) => c && typeof c.major === 'number') : [],
    databases: Array.isArray(v.databases) ? (v.databases as SqlStatus['databases']) : [],
    upgradeAvailable: Array.isArray(v.upgradeAvailable) ? (v.upgradeAvailable as SqlStatus['upgradeAvailable']) : [],
  }
}

export async function fetchSqlStatus(): Promise<SqlStatus> {
  return parseSqlStatus(await daemonCliGet<unknown>(primaryScope(), 'db/status'))
}

function parseParticipant(raw: unknown): SqlParticipant | null {
  const g = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : null
  if (!g || typeof g.projectId !== 'string') return null
  return {
    projectId: g.projectId,
    workspace: typeof g.workspace === 'string' ? g.workspace : null,
    level: g.level === 'write' ? 'write' : 'read',
    canManage: g.canManage === true,
  }
}

/** GET /cli/db/list, checked: `{ ok, databases: [...] }` (sql::ops::
 *  catalog_json). A reply without the list throws, so a broken read never
 *  looks like "no databases yet"; a row missing a field keeps a safe
 *  default instead of crashing the page. */
export function parseSqlDatabases(raw: unknown): SqlDatabase[] {
  const v = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : null
  if (!v || !Array.isArray(v.databases)) throw new Error('the reply had no list of databases')
  return v.databases
    .filter((r): r is Record<string, unknown> => !!r && typeof r === 'object' && typeof (r as Record<string, unknown>).id === 'string')
    .map((r) => {
      const ownerProjectId = typeof r.ownerProjectId === 'string' ? r.ownerProjectId : ''
      const ownerWorkspace = typeof r.ownerWorkspace === 'string' ? r.ownerWorkspace : null
      const cap = r.cap && typeof r.cap === 'object' ? (r.cap as Record<string, unknown>) : {}
      return {
        id: r.id as string,
        name: typeof r.name === 'string' && r.name ? r.name : (r.id as string),
        status: typeof r.status === 'string' ? r.status : 'active',
        createdAt: num(r.createdAt) ?? 0,
        type: 'sql',
        documents: r.documents !== false,
        ownerProjectId,
        ownerWorkspace,
        bindRole: typeof r.bindRole === 'string' && r.bindRole ? r.bindRole : null,
        cap: { used: num(cap.used) ?? 0, cap: num(cap.cap) ?? 0 },
        owner: parseParticipant(r.owner) ?? { projectId: ownerProjectId, workspace: ownerWorkspace, level: 'write', canManage: true },
        grants: (Array.isArray(r.grants) ? r.grants : []).map(parseParticipant).filter((g): g is SqlParticipant => g !== null),
        yourLevel: r.yourLevel === 'write' || r.yourLevel === 'read' ? r.yourLevel : null,
        dbAgentAccess: r.dbAgentAccess === 'read' || r.dbAgentAccess === 'write' ? r.dbAgentAccess : 'off',
      }
    })
}

export async function fetchSqlDatabases(): Promise<SqlDatabase[]> {
  return parseSqlDatabases(await daemonCliGet<unknown>(primaryScope(), 'db/list'))
}

/** The server's state in a word a person reads, and its dot. */
export function sqlStateWord(state: string): { word: string; tone: 'ok' | 'warn' | 'error' | 'neutral' } {
  switch (state) {
    case 'running':
      return { word: 'Running', tone: 'ok' }
    case 'degraded':
      return { word: 'Running with problems', tone: 'warn' }
    case 'installing':
      return { word: 'Starting', tone: 'warn' }
    case 'error':
      return { word: 'Stopped by an error', tone: 'error' }
    case 'disabled':
    case 'stopped':
      return { word: 'Off', tone: 'neutral' }
    case 'not-installed':
      return { word: 'Not set up', tone: 'neutral' }
    default:
      return { word: state.charAt(0).toUpperCase() + state.slice(1).replace(/-/g, ' '), tone: 'neutral' }
  }
}

/** "Postgres 16" for a database, from status, or null when unknown. */
export function dbVersionWord(status: Pick<SqlStatus, 'databases' | 'upgradeAvailable'> | null | undefined, dbId: string): string | null {
  const row = status?.databases?.find((d) => d.id === dbId)
  if (!row || row.major == null) return null
  const up = status?.upgradeAvailable?.some((u) => u.id === dbId)
  return up ? `Postgres ${row.major}, a newer version is available` : `Postgres ${row.major}`
}

/** A live move's progress in plain words: "3 of 3 tables copied, 2 MB behind." */
export function livePlainLine(r: LiveRun): string {
  const lag = r.lagBytes === null ? '' : r.lagBytes < 1024 ** 2 ? ', up to date' : `, ${Math.round(r.lagBytes / 1024 ** 2)} MB behind`
  return `${r.ready} of ${r.total} tables copied${lag}.`
}

/** A live move's state as a word. */
export function liveStateWord(state: string): string {
  switch (state) {
    case 'syncing':
      return 'Copying'
    case 'streaming':
      return 'Ready to switch'
    case 'stalled':
      return 'Stalled'
    case 'switched':
    case 'done':
      return 'Switched'
    case 'aborted':
      return 'Stopped'
    default:
      return state.charAt(0).toUpperCase() + state.slice(1)
  }
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
    `off-box *.k2.dev: ${CLI_NAME} publish subdomain create <label> --target localhost:5432 (port already listening — do not publish run Postgres)`,
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

// ── live moves (prd-app-pg-choice-dualrun-v1 PA40, 0.46.4) ──────────────

export interface LiveRun {
  db: string
  from: number
  to: number
  state: string
  ready: number
  total: number
  lagBytes: number | null
  walStatus: string | null
  lastError: string | null
}

export interface LiveStatus {
  runs: LiveRun[]
  /** Clusters a live move can't start from yet (wal_level not logical). */
  notLogical: number[]
  /** The root helper speaks live moves (protocol 10, pgdual). */
  helper: boolean
}

function num(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

/** GET /cli/db/upgrade (no db): the live runs and each cluster's wal_level. */
export function parseLiveStatus(raw: unknown): LiveStatus {
  const v = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}
  const runs = Array.isArray(v.live) ? v.live : []
  const levels = Array.isArray(v.walLevels) ? v.walLevels : []
  return {
    runs: runs
      .filter((r): r is Record<string, unknown> => !!r && typeof r === 'object')
      .map((r) => {
        const t = r.tables && typeof r.tables === 'object' ? (r.tables as Record<string, unknown>) : {}
        return {
          db: typeof r.db === 'string' ? r.db : '?',
          from: num(r.from) ?? 0,
          to: num(r.to) ?? 0,
          state: typeof r.state === 'string' ? r.state : 'unknown',
          ready: num(t.ready) ?? 0,
          total: num(t.total) ?? 0,
          lagBytes: num(r.lagBytes),
          walStatus: typeof r.walStatus === 'string' ? r.walStatus : null,
          lastError: typeof r.lastError === 'string' ? r.lastError : null,
        }
      }),
    notLogical: levels
      .filter((l): l is Record<string, unknown> => !!l && typeof l === 'object')
      .filter((l) => typeof l.walLevel === 'string' && l.walLevel.trim() !== 'logical')
      .map((l) => num(l.major))
      .filter((m): m is number => m !== null),
    helper: v.liveHelper === true,
  }
}

/** "3/3 tables · 2 MiB behind · WAL reserved" */
export function liveLine(r: LiveRun): string {
  const lag = r.lagBytes === null ? 'lag unknown' : `${Math.round(r.lagBytes / 1024 ** 2)} MiB behind`
  return `${r.ready}/${r.total} tables · ${lag}${r.walStatus ? ` · WAL ${r.walStatus}` : ''}`
}

export async function fetchLiveStatus(): Promise<LiveStatus> {
  return parseLiveStatus(await daemonCliGet<unknown>(primaryScope(), 'db/upgrade'))
}

export async function switchLiveMove(db: string): Promise<unknown> {
  return daemonCliPost(primaryScope(), 'db/upgrade', { db, switch: true })
}

export async function abortLiveMove(db: string): Promise<unknown> {
  return daemonCliPost(primaryScope(), 'db/upgrade', { db, abort: true })
}

export async function prepareLiveMoves(major: number): Promise<unknown> {
  return daemonCliPost(primaryScope(), 'db/upgrade', { prepareLive: major })
}
