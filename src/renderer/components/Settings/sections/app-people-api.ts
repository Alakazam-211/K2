// Settings → Apps → <App> → People (app-mobile-access-v1, 0.46.3): the
// daemon's person × agent grid, the App's devices that stay signed in, its
// sign-in settings and the suggested role templates. The daemon owns every
// rule (the grid is built from the same grant → role → room line a login
// gets); this file only parses its answers. Every call goes to the window's
// server (primaryScope).

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export type PeopleCan = {
  chat: boolean
  send: boolean
  files: 'none' | 'read' | 'write'
  tickets: 'none' | 'read' | 'write'
  store: 'none' | 'read' | 'write'
  heartbeats: 'none' | 'read' | 'write'
  activity: boolean
  members: boolean
}

export type PeopleCell = {
  room: string
  handle: string
  caps: string[]
  can: PeopleCan
  folders: { read: string[]; write: string[]; uploadDir: string | null } | null
}

export type PeopleAgent = { room: string; handle: string; displayName: string; color: string | null }

export type PeopleRow = {
  principalId: string
  username: string
  fullName: string | null
  kind: 'guest' | 'agent'
  enabled: boolean
  role: string | null
  cells: PeopleCell[]
}

export type DeviceRow = {
  id: string
  username: string
  name: string
  lastUsedAt: number
  expiresAt: number
}

export type AppAccess = {
  app: string
  agents: PeopleAgent[]
  people: PeopleRow[]
  devices: DeviceRow[]
  listed: boolean
  staySignedInDays: number
}

export type RoleTemplate = { id: string; label: string; description: string; note: string; caps: string[] }

function rec(v: unknown): Record<string, unknown> {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {}
}
function str(v: unknown): string {
  return typeof v === 'string' ? v : ''
}
function strs(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []
}
function lvl(v: unknown): 'none' | 'read' | 'write' {
  return v === 'read' || v === 'write' ? v : 'none'
}

function parseCell(raw: unknown): PeopleCell | null {
  const c = rec(raw)
  if (!str(c.room)) return null
  const can = rec(c.can)
  const f = c.folders == null ? null : rec(c.folders)
  return {
    room: str(c.room),
    handle: str(c.handle),
    caps: strs(c.caps),
    can: {
      chat: can.chat === true,
      send: can.send === true,
      files: lvl(can.files),
      tickets: lvl(can.tickets),
      store: lvl(can.store),
      heartbeats: lvl(can.heartbeats),
      activity: can.activity === true,
      members: can.members === true,
    },
    folders: f ? { read: strs(f.read), write: strs(f.write), uploadDir: str(f.uploadDir) || null } : null,
  }
}

export function parseAccess(raw: unknown): AppAccess {
  const r = rec(raw)
  const s = rec(r.settings)
  return {
    app: str(r.app),
    agents: (Array.isArray(r.agents) ? r.agents : []).map((a) => {
      const x = rec(a)
      return { room: str(x.room), handle: str(x.handle), displayName: str(x.displayName) || str(x.handle), color: str(x.color) || null }
    }),
    people: (Array.isArray(r.people) ? r.people : []).map((p) => {
      const x = rec(p)
      return {
        principalId: str(x.principalId),
        username: str(x.username),
        fullName: str(x.fullName) || null,
        kind: x.kind === 'agent' ? 'agent' : 'guest',
        enabled: x.enabled !== false,
        role: str(x.role) || null,
        cells: (Array.isArray(x.cells) ? x.cells : []).map(parseCell).filter((c): c is PeopleCell => c != null),
      }
    }),
    devices: (Array.isArray(r.devices) ? r.devices : []).map((d) => {
      const x = rec(d)
      return {
        id: str(x.id),
        username: str(x.username),
        name: str(x.name),
        lastUsedAt: typeof x.lastUsedAt === 'number' ? x.lastUsedAt : 0,
        expiresAt: typeof x.expiresAt === 'number' ? x.expiresAt : 0,
      }
    }),
    listed: s.listed === true,
    staySignedInDays: typeof s.staySignedInDays === 'number' ? s.staySignedInDays : 30,
  }
}

export function parseTemplates(raw: unknown): RoleTemplate[] {
  const list = rec(raw).templates
  return (Array.isArray(list) ? list : [])
    .map((t) => {
      const x = rec(t)
      return { id: str(x.id), label: str(x.label), description: str(x.description), note: str(x.note), caps: strs(x.caps) }
    })
    .filter((t) => t.id !== '')
}

/** One cell in words: what that person may do with that agent. */
export function cellSummary(c: PeopleCell): string {
  const out: string[] = []
  if (c.can.chat) out.push(c.can.send ? 'chat' : 'read chat')
  if (c.can.files !== 'none') out.push(c.can.files === 'write' ? 'files' : 'read files')
  if (c.can.tickets !== 'none') out.push(c.can.tickets === 'write' ? 'tickets' : 'read tickets')
  if (c.can.store !== 'none') out.push(c.can.store === 'write' ? 'data' : 'read data')
  if (c.can.activity) out.push('activity')
  if (c.can.members) out.push('members')
  if (c.can.heartbeats !== 'none') out.push('heartbeats')
  return out.length ? out.join(' · ') : 'no access'
}

export async function fetchAccess(app: string): Promise<AppAccess> {
  return parseAccess(await daemonCliGet<unknown>(primaryScope(), 'skin/access', { app }))
}

export async function fetchTemplates(): Promise<RoleTemplate[]> {
  return parseTemplates(await daemonCliGet<unknown>(primaryScope(), 'skin/roles/suggest'))
}

export async function addRoleFromTemplate(app: string, template: string): Promise<void> {
  await daemonCliPost(primaryScope(), 'skin/roles/from-template', { app, template })
}

export async function setStaySignedIn(app: string, staySignedInDays: number): Promise<void> {
  await daemonCliPost(primaryScope(), 'skin/access/settings', { app, staySignedInDays })
}

export async function setListed(app: string, listed: boolean): Promise<void> {
  await daemonCliPost(primaryScope(), 'skin/access/listed', { app, listed })
}

export async function revokeDevice(id: string): Promise<void> {
  await daemonCliPost(primaryScope(), 'skin/access/devices/revoke', { id })
}
