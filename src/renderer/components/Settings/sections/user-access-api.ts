// Settings → User Access and User Templates: the reads both pages share, and
// the plain words they use for what a person can reach.
//
// The daemon's words, for the code: a person is a *principal*
// (`/cli/skin/users`), and each thing they may use is a *grant*
// (`/cli/skin/grants`): `app` on a published App or on `host` (this server's
// own K2 page), with a role that names the agents' rooms they reach; `chat`
// (Chat / Voice Hosting, SewAccessParts); `mailbox`, `database` and `cli`,
// which K2 records but does not enforce. A template (`/cli/skin/templates`)
// is a named list of App + role lines; applying one writes grants.
// The pages say "people" and "what they can use", never these names.

import { daemonCliGet } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export type PersonRow = {
  id: string | null
  username: string
  fullName: string | null
  roleId: string | null
  roleName: string | null
  defaultRooms: string[]
  defaultRoomHandles: string[]
  email: string | null
  /** An agent tie (agent-app-users R2): the workspace handle it holds them to. */
  tiedTo: string | null
  /** Tied, and given more than that workspace: the extra waits for their own password. */
  pendingReset: boolean
  /** Why the tie exists: created | password | email | invite | app_key | app_reset. */
  tiedReasons: string[]
  /** Why an owner reset won't mail the stored email (any tie, or unconfirmed). */
  storedEmailRefused: string | null
  /** The login is locked (wrong passwords) until then, epoch seconds. */
  lockedUntil: number | null
  /** A box login (signs in to K2 itself): only the owner resets it (0.46.6, the daemon's rule). */
  boxLogin: boolean
}

export type RoomAccess = { handle: string; caps: string[] }

export type RoleRow = {
  id: string
  name: string
  appId: string | null
  roomAccess: RoomAccess[]
}

export type GrantRow = {
  id: string
  subjectKind: string
  subjectId: string
  kind: string
  targetId: string
  roleId: string | null
  scope: string | null
  enabled: boolean
  /** Sew S27 §17.1: hooked (chat) grants carry the sync state. */
  hookState?: string
  hookError?: string
}

export type AppOption = { id: string; name: string; kind: string }

export type TemplateLine = {
  id: string
  templateId: string
  kind: string
  roleId: string | null
  position: number
}

export type TemplateRow = {
  id: string
  name: string
  lines: TemplateLine[]
}

/** This server's own K2 page, as an App grant's target. */
export const HOST_APP = 'host'
export const HOST_LABEL = 'K2 on this server'

export function asRecord(raw: unknown): Record<string, unknown> {
  return raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
}

export function asString(v: unknown): string | null {
  return typeof v === 'string' && v.trim() ? v.trim() : null
}

export function asList(raw: unknown, keys: string[]): unknown[] {
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
      email: asString(rec.email),
      tiedTo: asString(rec.tiedTo),
      pendingReset: rec.pendingReset === true,
      tiedReasons: parseStringList(rec.tiedReasons),
      storedEmailRefused: asString(rec.storedEmailRefused),
      lockedUntil: typeof rec.lockedUntil === 'number' ? rec.lockedUntil : null,
      boxLogin: rec.boxLogin === true,
    }]
  })
}

export function parseRoles(raw: unknown): RoleRow[] {
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
    return [{ id, name, appId: asString(rec.appId) ?? asString(rec.app_id), roomAccess }]
  })
}

export function parseGrants(raw: unknown): GrantRow[] {
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
      ...(asString(rec.hookState) ? { hookState: asString(rec.hookState) as string } : {}),
      ...(asString(rec.hookError) ? { hookError: asString(rec.hookError) as string } : {}),
    }]
  })
}

export function parseTemplates(raw: unknown): TemplateRow[] {
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
    return [{ id, name, lines: [...lines].sort((a, b) => a.position - b.position) }]
  })
}

export function parseApps(raw: unknown): AppOption[] {
  return asList(raw, ['services']).flatMap((row) => {
    const rec = asRecord(row)
    const id = asString(rec.id)
    const name = asString(rec.name)
    const kind = asString(rec.kind)
    if (!id || !name || (kind !== 'cmd' && kind !== 'skin')) return []
    return [{ id, name, kind }]
  })
}

/** Every App published on this server: each workspace's list, read together. */
export async function fetchApps(): Promise<AppOption[]> {
  const projects = await daemonCliGet<unknown>(primaryScope(), 'projects/list')
  const list = Array.isArray(projects) ? projects : asList(projects, ['projects', 'items'])
  const ids = list.flatMap((row) => {
    const id = asString(asRecord(row).id)
    return id ? [id] : []
  })
  const listed = await Promise.all(ids.map((project) => daemonCliGet<unknown>(primaryScope(), 'publish/list', { project })))
  const seen = new Set<string>()
  return listed.flatMap(parseApps).filter((app) => (seen.has(app.id) ? false : (seen.add(app.id), true)))
}

/** An App grant's target in words: this server's K2 page, the App's name,
 *  or (an App no longer published) a plain fallback. */
export function appName(id: string, apps: AppOption[] | null): string {
  if (id === HOST_APP) return HOST_LABEL
  return apps?.find((a) => a.id === id)?.name ?? (apps ? 'An App that’s no longer published' : 'An App')
}

/** "notes and atlas", "notes, atlas and docs". */
export function listWords(items: string[]): string {
  if (items.length <= 1) return items[0] ?? ''
  return `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}`
}

/** What a role reaches, in a sentence: "As Reader: talks to the notes and
 *  atlas agents." (`named` false leaves out the role's name.) */
export function reachWords(role: RoleRow | null, fallbackRooms: string[] = [], named = true): string {
  const rooms = role ? role.roomAccess.map((r) => r.handle) : fallbackRooms
  const reach = rooms.length ? `talks to the ${listWords(rooms)} agent${rooms.length === 1 ? '' : 's'}` : 'reaches no agents yet'
  return role && named ? `As ${role.name}: ${reach}.` : `${reach[0].toUpperCase()}${reach.slice(1)}.`
}

/** A role, named for a picker: "Staff (Shop only)" when it belongs to one App. */
export function roleLabel(role: RoleRow, apps: AppOption[] | null): string {
  if (!role.appId) return role.name
  return `${role.name} (${appName(role.appId, apps)} only)`
}

// User Templates → "See ada in User Access": the person User Access opens on
// next, once.
let personHandoff: string | null = null
export function showPersonNext(username: string): void {
  personHandoff = username
}
export function takePersonHandoff(): string | null {
  const next = personHandoff
  personHandoff = null
  return next
}
