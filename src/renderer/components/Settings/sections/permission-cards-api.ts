// Permission cards (Beta), prd-agent-settings-cards-v1 §2, §8, §17.8.
//
// The renderer half of Settings → Access → Permission Cards, the Agent
// tab's "Can ask for permission cards" row and its "Asked for this" chip.
// Daemon-first: every fact here is read from the daemon and every switch is
// one person's gesture posted back. Nothing is derived or kept here.
//
//   - The global switch: `permissionCards` on `settings/get` (people only;
//     the off-golden's named exception 4) and `settings/update
//     {permissionCards}` (owner only, 403 otherwise). The daemon publishes
//     `sync:settings {permissionCards, feature, rehandshake}` on a flip.
//   - Per agent: `POST /cli/agent-access/set {toggle:"permission_cards",
//     workspace, value}` (Owner/Admin login or the owner token) →
//     `{ok, workspace, toggle, agentsCanRequestPermissions}`; the daemon
//     publishes `sync:settings {permissionCards:"agent", workspace}`. The
//     value is NOT on Project JSON (C8): it is read from
//     `GET /cli/permission/agents` (d2-asks.md).
//   - "Waiting on you": `GET /cli/permission/pending` (§17.8, lane A2).
//
// The `/cli/permission/` family exists only while the server switch is on
// (§2.4), so nothing here calls it unless `permissionCards` reads true.

import { listen } from '@tauri-apps/api/event'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { openOnAgentsPage } from '@/lib/page-selection'

// ── Shapes ──────────────────────────────────────────────────────────

/** One agent's "Can ask for permission cards" switch. */
export interface PermissionAgent {
  /** The workspace (project) id. */
  workspace: string
  handle: string
  name: string
  on: boolean
}

/** One recent request (A1's summary plus the agent and the setting). */
export interface PermissionRecent {
  id: string
  agent: string
  workspace: string
  action: string
  setting: string
  state: string
  askedAt: number
  answeredBy: string | null
  answeredAt: number | null
  addr: string | null
  card: string | null
}

export interface PermissionAgents {
  agents: PermissionAgent[]
  recent: PermissionRecent[]
}

/** One row of "Waiting on you" (§17.8): a request this viewer can answer. */
export interface PermissionPending {
  requestId: string
  addr: string
  cardId: string
  action: string
  setting: string
  whatChanges: string
  /** The asking agent's workspace handle. */
  agent: string
  askedFor: string | null
  floor: string
  proofKind: string
  createdAt: number
  expiresAt: number
}

/** The viewer's role on the focused server (`auth/whoami`). */
export type PermissionViewerRole = 'owner' | 'admin' | 'member'

// ── Parsers (a wrong top-level shape throws; a malformed row is dropped) ─

function obj(v: unknown): Record<string, unknown> | null {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : null
}

function str(r: Record<string, unknown>, k: string): string | null {
  const v = r[k]
  return typeof v === 'string' && v !== '' ? v : null
}

function num(r: Record<string, unknown>, k: string): number | null {
  const v = r[k]
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

function list(body: unknown, key: string, route: string): unknown[] {
  const o = obj(body)
  const v = o ? o[key] : undefined
  if (!Array.isArray(v)) throw new Error(`${route}: the daemon's answer has no "${key}" list`)
  return v
}

export function parsePermissionAgents(body: unknown): PermissionAgents {
  const agents: PermissionAgent[] = []
  for (const raw of list(body, 'agents', 'permission/agents')) {
    const r = obj(raw)
    if (!r) continue
    const workspace = str(r, 'workspace')
    const handle = str(r, 'handle')
    if (!workspace || !handle || typeof r.on !== 'boolean') continue
    agents.push({ workspace, handle, name: str(r, 'name') ?? handle, on: r.on })
  }
  const recent: PermissionRecent[] = []
  for (const raw of list(body, 'recent', 'permission/agents')) {
    const r = obj(raw)
    if (!r) continue
    const id = str(r, 'id')
    const state = str(r, 'state')
    const askedAt = num(r, 'askedAt')
    if (!id || !state || askedAt === null) continue
    const action = str(r, 'action') ?? ''
    recent.push({
      id,
      agent: str(r, 'agent') ?? '',
      workspace: str(r, 'workspace') ?? '',
      action,
      setting: str(r, 'setting') ?? action,
      state,
      askedAt,
      answeredBy: str(r, 'answeredBy'),
      answeredAt: num(r, 'answeredAt'),
      addr: str(r, 'addr'),
      card: str(r, 'card'),
    })
  }
  return { agents, recent }
}

export function parsePermissionPending(body: unknown): PermissionPending[] {
  const out: PermissionPending[] = []
  for (const raw of list(body, 'requests', 'permission/pending')) {
    const r = obj(raw)
    if (!r) continue
    const requestId = str(r, 'requestId')
    const addr = str(r, 'addr')
    const cardId = str(r, 'cardId')
    const agent = str(r, 'agent')
    const createdAt = num(r, 'createdAt')
    const expiresAt = num(r, 'expiresAt')
    if (!requestId || !addr || !cardId || !agent || createdAt === null || expiresAt === null) continue
    const action = str(r, 'action') ?? ''
    const proof = obj(r.proof)
    out.push({
      requestId,
      addr,
      cardId,
      action,
      setting: str(r, 'setting') ?? action,
      whatChanges: str(r, 'whatChanges') ?? '',
      agent,
      askedFor: str(r, 'askedFor'),
      floor: str(r, 'floor') ?? 'admin',
      proofKind: (proof && str(proof, 'kind')) ?? 'click',
      createdAt,
      expiresAt,
    })
  }
  return out
}

/** `permissionCards` from a `settings/get` (or `settings/update`) body:
 *  true/false, or null when the server has no permission cards at all (a
 *  daemon before 0.46.3 sends no such key). */
export function permissionCardsSetting(body: unknown): boolean | null {
  const o = obj(body)
  if (!o || !('permissionCards' in o)) return null
  if (typeof o.permissionCards !== 'boolean') {
    throw new Error('settings: "permissionCards" is not true or false')
  }
  return o.permissionCards
}

// ── Daemon calls (the focused server) ───────────────────────────────

export async function fetchPermissionCardsSetting(): Promise<boolean | null> {
  return permissionCardsSetting(await daemonCliGet<unknown>(primaryScope(), 'settings/get'))
}

/** The owner's global switch. Returns what the daemon stored. */
export async function setPermissionCardsSetting(on: boolean): Promise<boolean> {
  const body = await daemonCliPost<unknown>(primaryScope(), 'settings/update', { permissionCards: on })
  const stored = permissionCardsSetting(body)
  if (stored === null) throw new Error('settings/update: the daemon did not answer with "permissionCards"')
  return stored
}

export async function fetchPermissionAgents(workspace?: string): Promise<PermissionAgents> {
  const body = await daemonCliGet<unknown>(primaryScope(), 'permission/agents', workspace ? { workspace } : undefined)
  return parsePermissionAgents(body)
}

export async function fetchPermissionPending(): Promise<PermissionPending[]> {
  return parsePermissionPending(await daemonCliGet<unknown>(primaryScope(), 'permission/pending'))
}

/** One agent's "Can ask for permission cards". `workspace` is the project
 *  id (the route also takes a handle or path). Returns the stored value. */
export async function setAgentPermissionCards(workspace: string, value: boolean): Promise<boolean> {
  const body = await daemonCliPost<unknown>(primaryScope(), 'agent-access/set', {
    toggle: 'permission_cards',
    workspace,
    value,
  })
  const o = obj(body)
  if (!o || typeof o.agentsCanRequestPermissions !== 'boolean') {
    throw new Error('agent-access/set: the daemon did not answer with "agentsCanRequestPermissions"')
  }
  return o.agentsCanRequestPermissions
}

/** "hint (code)" for a daemon refusal, else the plain message. */
export function permissionErrorMessage(err: unknown): string {
  const msg = err instanceof Error ? err.message : String(err)
  const code = err && typeof err === 'object' ? (err as { code?: unknown }).code : null
  return typeof code === 'string' && code !== '' && !msg.includes(code) ? `${msg} (${code})` : msg
}

// ── Live updates ────────────────────────────────────────────────────

/** True for a `sync:settings` payload about permission cards: the global
 *  flip (`permissionCards: bool`) or one agent's switch
 *  (`permissionCards: "agent"`). */
export function isPermissionCardsSync(payload: unknown): boolean {
  const o = obj(payload)
  return !!o && 'permissionCards' in o
}

/** Re-read when the daemon says a permission-cards switch moved (any
 *  window, any client, a terminal). Returns the unlisten. */
export function onPermissionCardsSync(fn: () => void): () => void {
  let off: (() => void) | null = null
  let cancelled = false
  void listen<unknown>('sync:settings', (e) => {
    if (isPermissionCardsSync(e.payload)) fn()
  })
    .then((u) => {
      if (cancelled) u()
      else off = u
    })
    .catch((err) => console.warn('[permission-cards] sync:settings listen failed', err))
  return () => {
    cancelled = true
    off?.()
  }
}

// ── The Agent tab's "Asked for this" chip ───────────────────────────

/** Catalog action (§5.2) → the Agent tab row (`data-settings-id`) that
 *  controls the same thing. Held requests (apps.approve, mail.approve,
 *  garden.proposal.approve, sources.approve, compute.admin-approve) and
 *  Keep as mine have no switch row: they show on the Permission Cards page
 *  only. */
export const PERMISSION_ACTION_ROWS: Readonly<Record<string, string>> = {
  'compute.use': 'projects.agents-can-use-compute',
  'compute.operate': 'projects.agents-can-operate-compute',
  'compute.manage': 'projects.agents-can-manage-compute',
  'agents.manage': 'projects.agents-can-manage-agents',
  'projects.manage': 'projects.agents-can-manage-projects',
  'apps.build': 'projects.agents-can-manage-skin',
  'apps.manage-full': 'projects.agents-can-manage-skin',
  'apps.core': 'projects.agents-can-manage-skin',
  'vault.manage': 'projects.agents-can-manage-vault',
  'gardens.manage': 'projects.agents-can-manage-gardens',
  'dns.manage': 'projects.dns-manage-enabled',
  'connections.create': 'projects.agents-can-create-connections',
  'connections.add': 'projects.agents-can-create-connections',
  'mail.manage': 'projects.mail-manage-enabled',
  'mail.send-mode': 'projects.mail-agent-send',
  'db.agent-access': 'projects.db-agent-create',
}

/** The pending requests of one agent (by handle) that ask for `row`. */
export function asksForRow(pending: readonly PermissionPending[], handle: string, row: string): PermissionPending[] {
  return pending.filter((p) => p.agent === handle && PERMISSION_ACTION_ROWS[p.action] === row)
}

// ── Opening a card ──────────────────────────────────────────────────

/** "Opens the card" (§8): the asking agent's room on the Agents page, where
 *  its Thread carries the card. False when this server lists no workspace
 *  with that handle (the caller says so). */
export function openPermissionCard(agentHandle: string): boolean {
  const project = useProjectsStore.getState().projects.find((p) => p.handle === agentHandle)
  if (!project) return false
  useSettingsStore.getState().closeSettings()
  openOnAgentsPage(project.id)
  return true
}
