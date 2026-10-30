// Per-agent App permissions (prd-agent-app-permissions-v1 rev 2, slices B
// and H; 0.46.3). Typed bindings and the pure rules the Settings UI uses.
// Thin client: the daemon owns the switches, the cores, the box view and
// every check (the owner raises, an Admin only lowers, an agent never);
// this file reads that truth and sends the person's gestures to the
// window's server (`primaryScope()`).
//
//   POST /cli/agent-access/set {workspace, toggle: apps_build|apps_manage_full|app_users, value}
//   POST /cli/agent-access/set {workspace, toggle: app_core, core, value}
//   POST /cli/agent-access/set {workspace, toggle: app_review, value: false}
//   GET  /cli/app/list?box=1            every App with its maintaining agent
//   GET  /cli/app/list                  `accessReview` (Q4 review list)
//   POST /cli/app/unpark {app, to, confirmToken}    a parked App to a workspace (owner)
//   POST /cli/app/handover {app, to, confirmToken}  an App to another workspace (owner)
//     Both need the owner in person: without `confirmToken` the daemon
//     answers vault_confirm_required (action `app-handover`, bound to the
//     App id). Refused (handoff_needs_v1) while the App has what the move
//     can't carry until the full hand-off (0.46.5).
//   POST /cli/app/park {app}            stop and hold it (owner or admin)
//   POST /cli/app/uninstall {app}       no purge (owner or admin)

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export type AppCoreId = 'db' | 'files' | 'thread' | 'mail' | 'sew'

export interface AppCoreInfo {
  id: AppCoreId
  label: string
  /** What a tick lets the agent's Apps use. */
  detail: string
  /** Not enforced yet: shown, stored, takes effect later. */
  later: string | null
}

/** The cores, in the daemon's order (`APP_CORES`). */
export const APP_CORES: readonly AppCoreInfo[] = [
  { id: 'db', label: 'Database', detail: 'its own tables through K2 (store), and a private database flag', later: null },
  { id: 'files', label: 'Files and rooms', detail: 'files, wiki, tickets and activity in its own room', later: null },
  { id: 'thread', label: 'Thread', detail: 'Thread with this agent, its heartbeats (read) and its name', later: null },
  { id: 'mail', label: 'Mail', detail: 'mail and calendar for its guests', later: 'Takes effect when App mail lands' },
  { id: 'sew', label: 'Chat / Voice Hosting', detail: 'chat, voice and calls for its guests (the chat caps)', later: null },
]

const CORE_IDS: ReadonlySet<string> = new Set(APP_CORES.map((c) => c.id))

/** `projects.app_cores` (a JSON array) → known cores in display order. */
export function parseAppCores(raw: unknown): AppCoreId[] {
  let list: unknown = raw
  if (typeof raw === 'string') {
    try {
      list = JSON.parse(raw)
    } catch {
      return []
    }
  }
  if (!Array.isArray(list)) return []
  const have = new Set(list.filter((x): x is string => typeof x === 'string'))
  return APP_CORES.map((c) => c.id).filter((id) => have.has(id))
}

export type ViewerRole = 'owner' | 'admin' | 'member'

/** Q2: the owner raises and lowers; an Admin only lowers; a Member (and
 *  an unknown role, until whoami answers) changes nothing here. */
export function canSet(role: ViewerRole | null, current: boolean, next: boolean): boolean {
  if (current === next) return false
  if (role === 'owner') return true
  if (role === 'admin') return !next
  return false
}

/** The reason a control is shown but can't be used, or null. */
export function lockedReason(role: ViewerRole | null, on: boolean): string | null {
  if (role === 'owner') return null
  if (role === null) return 'Checking your role on this server'
  if (role === 'admin') return on ? null : 'Only the server’s owner can turn this on'
  return 'Only the server’s owner or an admin can change this'
}

/** "Let this agent manage its Apps' users" (0225): the owner both ways
 *  (an Admin may not turn it off either). */
export function canSetUsers(role: ViewerRole | null, current: boolean, next: boolean): boolean {
  return current !== next && role === 'owner'
}

/** Why the users switch is shown but can't be used, or null. */
export function lockedUsersReason(role: ViewerRole | null): string | null {
  if (role === 'owner') return null
  if (role === null) return 'Checking your role on this server'
  return 'Only the server’s owner can change this'
}

export async function setAppSwitch(
  workspace: string,
  toggle: 'apps_build' | 'apps_manage_full' | 'app_users',
  value: boolean,
): Promise<unknown> {
  return daemonCliPost<unknown>(primaryScope(), 'agent-access/set', { workspace, toggle, value })
}

export async function setAppCore(workspace: string, core: AppCoreId, value: boolean): Promise<AppCoreId[]> {
  const v = await daemonCliPost<{ cores?: unknown }>(primaryScope(), 'agent-access/set', {
    workspace,
    toggle: 'app_core',
    core,
    value,
  })
  return parseAppCores(v?.cores ?? [])
}

// ── the box view (Settings → Apps rows) ─────────────────────────────

export interface BoxMaintainer {
  kind: 'agent' | 'parked' | 'removed'
  agent: string | null
  handle: string | null
  address: string | null
  /** People only: the workspace, to open its card and paint its look. */
  workspaceId: string | null
}

export interface BoxApp {
  appId: string
  state: string
  parked: boolean
  maintainer: BoxMaintainer
  memoryLimitBytes: number | null
  hostnames: string[]
  buildAndRun: boolean | null
  manageEndToEnd: boolean | null
  cores: AppCoreId[]
  /** 0.46.4 (PA17): the App's Postgres database; null with none. */
  database: BoxAppDatabase | null
}

export interface BoxAppDatabase {
  name: string | null
  major: number | null
  /** Newer majors the App's requires.postgres allows. */
  available: number[]
  sharedWith: string[]
  /** K2 can't tell which database (a DATABASE_URL it can't map). */
  unknown: string | null
}

function parseDatabase(v: unknown): BoxAppDatabase | null {
  if (!isObj(v)) return null
  const nums = (x: unknown): number[] => (Array.isArray(x) ? x.filter((n): n is number => typeof n === 'number') : [])
  return {
    name: strOrNull(v.name),
    major: typeof v.major === 'number' ? v.major : null,
    available: nums(v.available),
    sharedWith: Array.isArray(v.sharedWith) ? v.sharedWith.filter((s): s is string => typeof s === 'string') : [],
    unknown: strOrNull(v.unknown),
  }
}

/** The Apps-row chip: "Postgres 16 · 17 available", or null with no database. */
export function postgresChip(db: BoxAppDatabase | null): string | null {
  if (!db) return null
  if (db.unknown) return 'Postgres: not a K2 database'
  if (db.major === null) return null
  const avail = db.available.length ? ` · ${db.available.join(', ')} available` : ''
  return `Postgres ${db.major}${avail}`
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

function strOrNull(v: unknown): string | null {
  return typeof v === 'string' && v ? v : null
}

const KINDS: ReadonlySet<string> = new Set(['agent', 'parked', 'removed'])

export function parseBoxApps(raw: unknown): BoxApp[] {
  const v = isObj(raw) ? raw : {}
  const apps = Array.isArray(v.apps) ? v.apps.filter(isObj) : []
  return apps
    .map((a): BoxApp => {
      const m = isObj(a.maintainer) ? a.maintainer : {}
      const mem = isObj(a.memory) ? a.memory : {}
      const sw = isObj(a.switches) ? a.switches : null
      return {
        appId: typeof a.appId === 'string' ? a.appId : '?',
        state: typeof a.state === 'string' ? a.state : 'unknown',
        parked: a.parked === true,
        maintainer: {
          kind: (typeof m.kind === 'string' && KINDS.has(m.kind) ? m.kind : 'removed') as BoxMaintainer['kind'],
          agent: strOrNull(m.agent),
          handle: strOrNull(m.handle),
          address: strOrNull(m.address),
          workspaceId: strOrNull(m.workspaceId),
        },
        memoryLimitBytes: typeof mem.limitBytes === 'number' ? mem.limitBytes : null,
        hostnames: Array.isArray(a.hostnames) ? a.hostnames.filter((h): h is string => typeof h === 'string') : [],
        buildAndRun: sw ? sw.buildAndRun === true : null,
        manageEndToEnd: sw ? sw.manageEndToEnd === true : null,
        cores: sw ? parseAppCores(sw.cores) : [],
        database: parseDatabase(a.database),
      }
    })
    .sort((x, y) => x.appId.localeCompare(y.appId))
}

/** 512 MiB / 2 GiB / "no limit". */
export function memoryLabel(bytes: number | null): string {
  if (bytes === null || bytes <= 0) return 'no memory limit'
  const gib = bytes / 1024 ** 3
  if (gib >= 1) return `${Number.isInteger(gib) ? gib : gib.toFixed(1)} GiB`
  return `${Math.round(bytes / 1024 ** 2)} MiB`
}

/** The App's state in plain words. */
export function stateLabel(app: Pick<BoxApp, 'state' | 'parked' | 'maintainer'>): string {
  if (app.parked) return 'Parked (held by the owner)'
  if (app.maintainer.kind === 'removed') return 'Unassigned (its workspace was removed)'
  return app.state
}

/** Parked or orphaned: the owner hands it over, unparks or uninstalls. */
export function needsOwner(app: Pick<BoxApp, 'parked' | 'maintainer'>): boolean {
  return app.parked || app.maintainer.kind !== 'agent'
}

export async function fetchBoxApps(): Promise<BoxApp[]> {
  return parseBoxApps(await daemonCliGet<unknown>(primaryScope(), 'app/list', { box: '1' }))
}

// ── Q4 review list ───────────────────────────────────────────────────

export interface ReviewRow {
  workspaceId: string
  handle: string
  level: 'off' | 'build' | 'full'
}

export function parseReview(raw: unknown): ReviewRow[] {
  const v = isObj(raw) ? raw : {}
  const rows = Array.isArray(v.accessReview) ? v.accessReview.filter(isObj) : []
  return rows
    .filter((r) => typeof r.workspace === 'string' && r.workspace)
    .map((r) => ({
      workspaceId: r.workspace as string,
      handle: typeof r.handle === 'string' && r.handle ? r.handle : (r.workspace as string),
      level: (r.level === 'build' || r.level === 'full' ? r.level : 'off') as ReviewRow['level'],
    }))
}

export async function fetchReview(): Promise<ReviewRow[]> {
  return parseReview(await daemonCliGet<unknown>(primaryScope(), 'app/list'))
}

export async function dismissReview(workspaceId: string): Promise<void> {
  await daemonCliPost<unknown>(primaryScope(), 'agent-access/set', {
    workspace: workspaceId,
    toggle: 'app_review',
    value: false,
  })
}

// ── owner actions on a parked or orphaned App ────────────────────────

/** The vault confirm a hand-over or unpark needs (bound to the App id). */
export const HANDOVER_CONFIRM_ACTION = 'app-handover'

export async function unparkApp(app: string, to: string, confirmToken?: string): Promise<void> {
  await daemonCliPost<unknown>(primaryScope(), 'app/unpark', { app, to, ...(confirmToken ? { confirmToken } : {}) })
}

export async function handOverApp(app: string, to: string, confirmToken?: string): Promise<void> {
  await daemonCliPost<unknown>(primaryScope(), 'app/handover', { app, to, ...(confirmToken ? { confirmToken } : {}) })
}

export async function parkApp(app: string): Promise<void> {
  await daemonCliPost<unknown>(primaryScope(), 'app/park', { app })
}

export async function uninstallApp(app: string): Promise<void> {
  await daemonCliPost<unknown>(primaryScope(), 'app/uninstall', { app })
}

// ── App handover (prd-app-handoff-v1, `k2 app handover`, 0.46.5) ─────

export interface HandoverBlocker {
  code: string
  hint: string
  names: string[]
}

/** What a handover would move (GET /cli/app/handover/plan). The daemon
 *  owns the rules; this is the card's view of them. */
export interface HandoverPlan {
  lines: string[]
  blockers: HandoverBlocker[]
  ready: boolean
  planHash: string
  databaseCase: string
  collisions: boolean
  sharedGuests: number
  toHandle: string
}

export interface HandoverOptions {
  db?: 'share'
  collisions?: 'keep' | 'use_b'
  resetShared?: boolean
}

export function parseHandoverPlan(raw: unknown): HandoverPlan {
  const v = isObj(raw) ? raw : {}
  const blockers = Array.isArray(v.blockers) ? v.blockers.filter(isObj) : []
  const secrets = Array.isArray(v.secrets) ? v.secrets.filter(isObj) : []
  const guests = isObj(v.guests) ? v.guests : {}
  const byClass = isObj(guests.byClass) ? guests.byClass : {}
  const db = isObj(v.database) ? v.database : {}
  const to = isObj(v.to) ? v.to : {}
  return {
    lines: Array.isArray(v.lines) ? v.lines.filter((l): l is string => typeof l === 'string') : [],
    blockers: blockers.map((b) => ({
      code: typeof b.code === 'string' ? b.code : 'blocked',
      hint: typeof b.hint === 'string' ? b.hint : '',
      names: Array.isArray(b.names) ? b.names.filter((n): n is string => typeof n === 'string') : [],
    })),
    ready: v.ready === true,
    planHash: typeof v.planHash === 'string' ? v.planHash : '',
    databaseCase: typeof db.case === 'string' ? db.case : 'none',
    collisions: secrets.some((s) => s.collision === true),
    sharedGuests: typeof byClass.shared === 'number' ? byClass.shared : 0,
    toHandle: typeof to.handle === 'string' ? to.handle : '',
  }
}

function handoverParams(app: string, to: string, o: HandoverOptions): Record<string, string> {
  const p: Record<string, string> = { app, to }
  if (o.db) p.db = o.db
  if (o.collisions) p.collisions = o.collisions
  if (o.resetShared) p.resetShared = '1'
  return p
}

export async function fetchHandoverPlan(app: string, to: string, o: HandoverOptions = {}): Promise<HandoverPlan> {
  return parseHandoverPlan(await daemonCliGet<unknown>(primaryScope(), 'app/handover/plan', handoverParams(app, to, o)))
}

/** File the owner's card (`unpark` for a parked App). Returns the held
 *  request the owner approves with a fresh vault confirm. */
export async function requestHandover(app: string, to: string, o: HandoverOptions, unpark: boolean): Promise<{ request: string }> {
  const body: Record<string, unknown> = { app, to }
  if (o.db) body.db = o.db
  if (o.collisions) body.collisions = o.collisions
  if (o.resetShared) body.resetShared = true
  const v = await daemonCliPost<unknown>(primaryScope(), unpark ? 'app/unpark' : 'app/handover', body)
  const r = isObj(v) && isObj(v.request) ? v.request : null
  if (!r || typeof r.id !== 'string') throw new Error(JSON.stringify({ error: { code: 'no_card', hint: 'the daemon filed no handover card' } }))
  return { request: r.id }
}

export interface HandoverRow {
  id: string
  appId: string
  state: string
  fromHandle: string | null
  toHandle: string
  inWindow: boolean
  windowUntil: number | null
  rekeyState: string | null
}

export function parseHandovers(raw: unknown): HandoverRow[] {
  const v = isObj(raw) ? raw : {}
  const rows = Array.isArray(v.handovers) ? v.handovers.filter(isObj) : []
  return rows
    .filter((r) => typeof r.id === 'string' && typeof r.appId === 'string')
    .map((r) => ({
      id: r.id as string,
      appId: r.appId as string,
      state: typeof r.state === 'string' ? r.state : '',
      fromHandle: typeof r.fromHandle === 'string' ? r.fromHandle : null,
      toHandle: typeof r.toHandle === 'string' ? r.toHandle : '',
      inWindow: r.inWindow === true,
      windowUntil: typeof r.windowUntil === 'number' ? r.windowUntil : null,
      rekeyState: typeof r.rekeyState === 'string' ? r.rekeyState : null,
    }))
}

export async function fetchHandovers(): Promise<HandoverRow[]> {
  return parseHandovers(await daemonCliGet<unknown>(primaryScope(), 'app/handover/status'))
}

/** The owner's undo card (approved like the handover's own). */
export async function requestUndo(app: string): Promise<{ request: string }> {
  const v = await daemonCliPost<unknown>(primaryScope(), 'app/handover/undo', { app })
  const r = isObj(v) && isObj(v.request) ? v.request : null
  if (!r || typeof r.id !== 'string') throw new Error(JSON.stringify({ error: { code: 'no_card', hint: 'the daemon filed no undo card' } }))
  return { request: r.id }
}

export async function finishHandover(app: string): Promise<void> {
  await daemonCliPost<unknown>(primaryScope(), 'app/handover/finish', { app })
}
