// Settings → Sidecars → Apps. Master-detail: Host plus each published
// service. Roles and app grants live on the selected detail. Platform
// tokens and Hydra are box-level and render on Host only.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { SquareCheckbox, Toggle } from '@/components/ui'
import { SettingDropdown, SettingRow, SettingsGroup } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import { primaryScope } from '@/kessel/server-scope'
import { useServerSupports } from '@/lib/server-capabilities'

export const SKIN_ACCESS_MANIFEST: SettingEntry[] = [
  {
    id: 'skin-access.roles',
    section: 'skin-access',
    label: 'Roles',
    description: 'Named bundles of scopes + agents for guests. Not Connect owner/admin/member/viewer.',
    keywords: [
      'skin role',
      'roles',
      'caps',
      'scopes',
      'assign',
      'dentist',
      'bundle',
      'skin',
      'app',
    ],
    group: 'Roles',
  },
  {
    id: 'skin-access.keys',
    section: 'skin-access',
    label: 'Platform tokens',
    description: 'Mint platform k2skn_ tokens (name, caps, rooms); secret shown once. Not for a user.',
    keywords: [
      'skin token',
      'k2skn',
      'scopes',
      'caps',
      'thread',
      'overlay',
      'files',
      'revoke',
      'mint',
      'platform',
      'name',
      'skin',
      'app',
    ],
    group: 'Platform tokens',
  },
  {
    id: 'skin-access.hydra',
    section: 'skin-access',
    label: 'OIDC issuer (Hydra)',
    description: 'Opt-in Hydra sidecar — Linux loopback 4444/4445; enabling apps does not start Hydra',
    keywords: ['oidc', 'hydra', 'issuer', 'openid', 'skin', 'app'],
    group: 'OIDC issuer (Hydra)',
  },
]

export const DEFAULT_SKIN_CAPS = ['thread:read', 'thread:post'] as const
export const SKIN_FILE_CAPS = ['files:read', 'files:write'] as const
export const SKIN_TICKET_CAPS = ['tickets:read', 'tickets:post'] as const
export const SKIN_WIKI_CAPS = ['wiki:read'] as const
export const SKIN_STORE_CAPS = ['store:read', 'store:write'] as const
// Own group. Not a files cap — files:read / files:write do not imply it.
export const SKIN_ACTIVITY_CAPS = ['activity:read'] as const
// Own group (prd-app-heartbeats-surface-v1 AH1). Not implied by files or
// activity, and neither implies the other. Offered only when the window's
// server supports `app-heartbeats` (AH24).
export const SKIN_HEARTBEAT_CAPS = ['heartbeats:read', 'heartbeats:write'] as const
export const SKIN_CAP_CHOICES = [...DEFAULT_SKIN_CAPS, ...SKIN_FILE_CAPS, ...SKIN_TICKET_CAPS, ...SKIN_WIKI_CAPS, ...SKIN_STORE_CAPS, ...SKIN_ACTIVITY_CAPS, ...SKIN_HEARTBEAT_CAPS] as const
export type SkinCap = (typeof SKIN_CAP_CHOICES)[number]

/** What each cap lets an app do in a room — the checkbox tooltip. */
export const SKIN_CAP_LABELS: Record<SkinCap, string> = {
  'thread:read': 'Read the Thread',
  'thread:post': 'Post to the Thread',
  'files:read': 'Read files (also shows heartbeat instructions)',
  'files:write': 'Write files (not .k2/heartbeats)',
  'tickets:read': 'Read tickets and get live ticket updates',
  'tickets:post': 'Open, answer, assign and set status on tickets',
  'wiki:read': 'Read the wiki',
  'store:read': 'Read the workspace store',
  'store:write': 'Change dump-table rows',
  'activity:read': 'See when the agent is working',
  'heartbeats:read': 'See heartbeats: schedule, next fire, history',
  'heartbeats:write': 'Add, edit, turn on or off, archive and fire heartbeats',
}

/** AH24: the heartbeat caps are offered only when the server ships them. */
export function skinCapChoices(appHeartbeats: boolean): readonly SkinCap[] {
  if (appHeartbeats) return SKIN_CAP_CHOICES
  return SKIN_CAP_CHOICES.filter(
    (c) => !(SKIN_HEARTBEAT_CAPS as readonly string[]).includes(c),
  )
}

export type FrontDoorMode = 'connect' | 'direct'

export type SkinFrontDoorCaddy = {
  running?: boolean
  pid?: number | null
  binary?: string | null
  configPath?: string | null
  missing?: boolean
}

export type SkinFrontDoorNested = {
  label?: string | null
  host?: string | null
  target?: string | null
  registered?: boolean
}

export type SkinFrontDoor = {
  mode: FrontDoorMode
  url: string | null
  hint: string | null
  connectUrl: string
  listen: string
  uiPort: number | null
  applied?: boolean
  caddy?: SkinFrontDoorCaddy
  nested?: SkinFrontDoorNested
  error: string | null
  subdomain: string | null
}

export type SkinUser = {
  id: string | null
  username: string
  createdAt?: string | null
  defaultRooms: string[]
  defaultRoomHandles: string[]
  hasPassword: boolean
  roleId: string | null
  roleName: string | null
  email: string | null
}

export type SkinRoomAccess = {
  handle: string
  caps: string[]
}

export type SkinRole = {
  id: string
  name: string
  caps: string[]
  rooms: string[]
  roomHandles: string[]
  roomAccess: SkinRoomAccess[]
  appId: string | null
}

export type PublishedApp = {
  id: string
  projectId: string
  name: string
  status: string
  expose: string | null
  url: string | null
  kind: string
}

export type SkinGrantRow = {
  id: string
  subjectKind: string
  subjectId: string
  kind: string
  targetId: string
  roleId: string | null
  scope: string | null
  enabled: boolean
}

const PUBLISH_STATUS = new Set(['running', 'starting', 'exited', 'stopped', 'unhealthy'])

export type SkinTokenRow = {
  id: string
  prefix: string
  name: string
  caps: string[]
  rooms: string[]
  roomHandles: string[]
}

export type SkinWorkspace = {
  id: string
  handle: string
  name: string
}

export type SkinHydra = {
  supported: boolean
  enabled: boolean
  running: boolean
  publicUrl: string | null
  adminUrl: string | null
  hint: string | null
}

const HYDRA_LINUX_BANNER =
  'THIS FEATURE ONLY WORKS ON LINUX DEPLOYMENTS, THIS PAGE IS JUST HERE FOR EXAMPLE PURPOSES.'

export const DEFAULT_HYDRA: SkinHydra = {
  supported: false,
  enabled: false,
  running: false,
  publicUrl: 'http://127.0.0.1:4444/',
  adminUrl: 'http://127.0.0.1:4445/',
  hint: HYDRA_LINUX_BANNER,
}

const CONNECT_URL_STUB = 'https://skin.<sub>.k2.dev'
const DIRECT_LISTEN_STUB = 'Caddy :443 (or LAN port) → 127.0.0.1:daemon'

const INPUT_CLS =
  'w-full px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

function asRecord(raw: unknown): Record<string, unknown> {
  return raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
}

function asString(v: unknown): string | null {
  return typeof v === 'string' && v.trim() ? v.trim() : null
}

function asBool(v: unknown): boolean | undefined {
  return typeof v === 'boolean' ? v : undefined
}

function asInt(v: unknown): number | null {
  if (typeof v === 'number' && Number.isFinite(v)) return Math.trunc(v)
  if (typeof v === 'string' && v.trim()) {
    const n = Number(v.trim())
    if (Number.isFinite(n)) return Math.trunc(n)
  }
  return null
}

function hostFromUrl(url: string): string | null {
  try {
    const host = new URL(url).hostname.trim()
    return host || null
  } catch {
    return null
  }
}

function deriveConnectUrl(rec: Record<string, unknown>, subdomain: string | null): string {
  const explicit = asString(rec.connectUrl) ?? asString(rec.connect_url)
  if (explicit) return explicit
  if (subdomain) return `https://skin.${subdomain}.k2.dev`
  const url = asString(rec.url)
  if (url) {
    const host = hostFromUrl(url)
    if (host?.endsWith('.k2.dev')) {
      return host.startsWith('skin.') ? `https://${host}` : `https://skin.${host}`
    }
  }
  return CONNECT_URL_STUB
}

function parseCaddy(raw: unknown): SkinFrontDoorCaddy | undefined {
  if (raw == null || typeof raw !== 'object' || Array.isArray(raw)) return undefined
  const rec = asRecord(raw)
  const pidRaw = rec.pid
  return {
    running: asBool(rec.running),
    pid: pidRaw === null ? null : asInt(pidRaw),
    binary: asString(rec.binary),
    configPath: asString(rec.configPath) ?? asString(rec.config_path),
    missing: asBool(rec.missing),
  }
}

function parseNested(raw: unknown): SkinFrontDoorNested | undefined {
  if (raw == null || typeof raw !== 'object' || Array.isArray(raw)) return undefined
  const rec = asRecord(raw)
  return {
    label: asString(rec.label),
    host: asString(rec.host),
    target: asString(rec.target),
    registered: asBool(rec.registered),
  }
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

function parseMode(raw: unknown): FrontDoorMode {
  const rec = asRecord(raw)
  const m = asString(rec.mode) ?? asString(rec.frontDoor) ?? asString(rec.front_door)
  return m === 'direct' ? 'direct' : 'connect'
}

export function parseFrontDoor(raw: unknown): SkinFrontDoor {
  const rec = asRecord(raw)
  const subdomain = asString(rec.subdomain) ?? asString(rec.sub)
  const listen =
    asString(rec.listen) ??
    asString(rec.directListen) ??
    asString(rec.direct_listen) ??
    DIRECT_LISTEN_STUB
  const uiPortRaw = rec.uiPort ?? rec.ui_port
  const applied = asBool(rec.applied)
  const caddy = parseCaddy(rec.caddy)
  const nested = parseNested(rec.nested)
  return {
    mode: parseMode(raw),
    url: asString(rec.url),
    hint: asString(rec.hint),
    connectUrl: deriveConnectUrl(rec, subdomain),
    listen,
    uiPort: uiPortRaw === null || uiPortRaw === undefined ? null : asInt(uiPortRaw),
    ...(applied !== undefined ? { applied } : {}),
    ...(caddy ? { caddy } : {}),
    ...(nested ? { nested } : {}),
    error: asString(rec.error),
    subdomain,
  }
}

export const DEFAULT_FRONT_DOOR: SkinFrontDoor = {
  mode: 'connect',
  url: null,
  hint: null,
  connectUrl: CONNECT_URL_STUB,
  listen: DIRECT_LISTEN_STUB,
  uiPort: null,
  error: null,
  subdomain: null,
}

function parseStringList(raw: unknown): string[] {
  if (!Array.isArray(raw)) return []
  return raw.filter((c): c is string => typeof c === 'string' && Boolean(c.trim())).map((s) => s.trim())
}

export function parseSkinUsers(raw: unknown): SkinUser[] {
  return asList(raw, ['users', 'roster']).flatMap((row) => {
    const rec = asRecord(row)
    const usernameField = asString(rec.username)
    const username = usernameField ?? asString(rec.id) ?? asString(rec.principal)
    if (!username) return []
    return [{
      id: usernameField ? asString(rec.id) : null,
      username,
      createdAt: asString(rec.createdAt) ?? asString(rec.created_at),
      defaultRooms: parseStringList(rec.defaultRooms ?? rec.default_rooms),
      defaultRoomHandles: parseStringList(rec.defaultRoomHandles ?? rec.default_room_handles),
      hasPassword: asBool(rec.hasPassword) ?? asBool(rec.has_password) ?? false,
      roleId: asString(rec.roleId) ?? asString(rec.role_id),
      roleName: asString(rec.roleName) ?? asString(rec.role_name),
      email: asString(rec.email),
    }]
  })
}

export function parseSkinRoles(raw: unknown): SkinRole[] {
  return asList(raw, ['roles']).flatMap((row) => {
    const rec = asRecord(row)
    const id = asString(rec.id)
    const name = asString(rec.name)
    if (!id || !name) return []
    const roomAccess = parseRoomAccess(rec.roomAccess ?? rec.room_access)
    const roomHandles = parseStringList(rec.roomHandles ?? rec.room_handles)
    return [{
      id,
      name,
      caps: parseCaps(rec.caps ?? rec.scopes ?? rec.capabilities),
      rooms: parseStringList(rec.rooms),
      roomHandles: roomHandles.length ? roomHandles : roomAccess.map((r) => r.handle),
      roomAccess,
      appId: asString(rec.appId) ?? asString(rec.app_id),
    }]
  })
}

/** Both `cmd` and `skin` publishes are apps. Key is `published_services.id`, not the name. */
export function parsePublishedApps(raw: unknown): PublishedApp[] {
  return asList(raw, ['services']).flatMap((row) => {
    const rec = asRecord(row)
    const id = asString(rec.id)
    const name = asString(rec.name)
    const projectId = asString(rec.projectId) ?? asString(rec.project_id)
    const kind = asString(rec.kind)
    if (!id || !name || !projectId || (kind !== 'cmd' && kind !== 'skin')) return []
    return [{
      id,
      projectId,
      name,
      status: asString(rec.status) ?? '',
      expose: asString(rec.expose),
      url: asString(rec.url),
      kind,
    }]
  })
}

export function parseSkinGrants(raw: unknown): SkinGrantRow[] {
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
    }]
  })
}

function grantSubjectLabel(grant: SkinGrantRow, users: SkinUser[], workspaces: SkinWorkspace[]): string {
  if (grant.subjectKind === 'workspace') {
    const ws = workspaces.find((w) => w.id === grant.subjectId)
    return ws ? ws.name : grant.subjectId
  }
  if (grant.subjectKind === 'principal') {
    const user = users.find((u) => u.id != null && u.id === grant.subjectId)
    return user ? user.username : grant.subjectId
  }
  return grant.subjectId
}

export function parseWorkspaces(raw: unknown): SkinWorkspace[] {
  const list = Array.isArray(raw) ? raw : asList(raw, ['projects', 'items'])
  return list.flatMap((row) => {
    const rec = asRecord(row)
    const id = asString(rec.id)
    const handle = asString(rec.handle)
    if (!id || !handle) return []
    return [{ id, handle, name: asString(rec.name) ?? handle }]
  })
}

function parseRoomAccess(raw: unknown): SkinRoomAccess[] {
  if (!Array.isArray(raw)) return []
  return raw.flatMap((row) => {
    const rec = asRecord(row)
    const handle = asString(rec.handle) ?? asString(rec.id)
    if (!handle) return []
    return [{ handle, caps: parseCaps(rec.caps ?? rec.scopes ?? rec.capabilities) }]
  })
}

function parseCaps(raw: unknown): string[] {
  if (Array.isArray(raw)) {
    return raw.filter((c): c is string => typeof c === 'string' && Boolean(c.trim()))
  }
  const rec = asRecord(raw)
  const on: string[] = []
  for (const [k, v] of Object.entries(rec)) {
    if (v === true) on.push(k)
  }
  return on
}

export function parseSkinTokens(raw: unknown): SkinTokenRow[] {
  return asList(raw, ['tokens', 'keys']).flatMap((row) => {
    const rec = asRecord(row)
    const id = asString(rec.id)
    if (!id) return []
    const name =
      asString(rec.name) ?? asString(rec.username) ?? asString(rec.user) ?? ''
    const caps = parseCaps(rec.caps ?? rec.scopes ?? rec.capabilities)
    const prefix =
      asString(rec.prefix) ??
      (id.startsWith('k2skn_') ? id : `k2skn_…${id.slice(-4)}`)
    return [{
      id,
      prefix,
      name,
      caps,
      rooms: parseStringList(rec.rooms),
      roomHandles: parseStringList(rec.roomHandles ?? rec.room_handles),
    }]
  })
}

export function parseHydra(raw: unknown): SkinHydra {
  const rec = asRecord(raw)
  return {
    supported: asBool(rec.supported) ?? false,
    enabled: asBool(rec.enabled) ?? false,
    running: asBool(rec.running) ?? false,
    publicUrl: asString(rec.publicUrl) ?? asString(rec.public_url),
    adminUrl: asString(rec.adminUrl) ?? asString(rec.admin_url),
    hint: asString(rec.hint),
  }
}

export function mintSecretFrom(raw: unknown): string | null {
  const rec = asRecord(raw)
  for (const k of ['secret', 'key', 'token']) {
    const v = rec[k]
    if (typeof v === 'string' && v.trim()) return v.trim()
  }
  return null
}

export function prefixLabel(prefix: string): string {
  if (prefix.startsWith('k2skn_') && prefix.length > 10) {
    return `k2skn_…${prefix.slice(-4)}`
  }
  return prefix
}

export function SkinAccessSection(): React.JSX.Element {
  const appHeartbeats = useServerSupports('app-heartbeats')
  const capChoices = useMemo(() => skinCapChoices(appHeartbeats), [appHeartbeats])
  const [users, setUsers] = useState<SkinUser[]>([])
  const [roles, setRoles] = useState<SkinRole[]>([])
  const [tokens, setTokens] = useState<SkinTokenRow[]>([])
  const [workspaces, setWorkspaces] = useState<SkinWorkspace[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [mintName, setMintName] = useState('')
  const [mintCaps, setMintCaps] = useState<Set<string>>(() => new Set(DEFAULT_SKIN_CAPS))
  const [mintRooms, setMintRooms] = useState<Set<string>>(() => new Set())
  const [mintBusy, setMintBusy] = useState(false)
  const [mintError, setMintError] = useState<string | null>(null)
  const [mintedSecret, setMintedSecret] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [hydra, setHydra] = useState<SkinHydra>(DEFAULT_HYDRA)
  const [hydraBusy, setHydraBusy] = useState(false)
  const [bannerDismissed, setBannerDismissed] = useState(false)
  const [editKeyId, setEditKeyId] = useState<string | null>(null)
  const [editKeyRooms, setEditKeyRooms] = useState<Set<string>>(() => new Set())
  const [newRoleName, setNewRoleName] = useState('')
  const [newRoleRooms, setNewRoleRooms] = useState<Set<string>>(() => new Set())
  const [newRoleCapsByRoom, setNewRoleCapsByRoom] = useState<Record<string, Set<string>>>({})
  const [roleBusy, setRoleBusy] = useState(false)
  const [roleError, setRoleError] = useState<string | null>(null)
  const [editRoleId, setEditRoleId] = useState<string | null>(null)
  const [editRoleRooms, setEditRoleRooms] = useState<Set<string>>(() => new Set())
  const [editRoleCapsByRoom, setEditRoleCapsByRoom] = useState<Record<string, Set<string>>>({})
  const [removeRoleConfirm, setRemoveRoleConfirm] = useState<string | null>(null)
  const [publishedApps, setPublishedApps] = useState<PublishedApp[]>([])
  const [grants, setGrants] = useState<SkinGrantRow[]>([])
  const [selectedApp, setSelectedApp] = useState<string | null>(null)
  const [addPersonId, setAddPersonId] = useState('')
  const [addRoleId, setAddRoleId] = useState('')
  const [publishBusy, setPublishBusy] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    setError(null)
    const failures: string[] = []
    try {
      const roster = await daemonCliGet<unknown>(primaryScope(), 'skin/users')
      setUsers(parseSkinUsers(roster))
    } catch (e) {
      failures.push(`users: ${errText(e)}`)
      setUsers([])
    }
    try {
      const roleList = await daemonCliGet<unknown>(primaryScope(), 'skin/roles')
      setRoles(parseSkinRoles(roleList))
    } catch (e) {
      failures.push(`roles: ${errText(e)}`)
      setRoles([])
    }
    try {
      const keys = await daemonCliGet<unknown>(primaryScope(), 'skin-tokens')
      setTokens(parseSkinTokens(keys))
    } catch (e) {
      failures.push(`keys: ${errText(e)}`)
      setTokens([])
    }
    let workspaceRows: SkinWorkspace[] = []
    try {
      const projects = await daemonCliGet<unknown>(primaryScope(), 'projects/list')
      workspaceRows = parseWorkspaces(projects)
      setWorkspaces(workspaceRows)
    } catch (e) {
      failures.push(`workspaces: ${errText(e)}`)
      setWorkspaces([])
    }
    const apps: PublishedApp[] = []
    for (const workspace of workspaceRows) {
      try {
        const listed = await daemonCliGet<unknown>(primaryScope(), 'publish/list', { project: workspace.id })
        apps.push(...parsePublishedApps(listed))
      } catch (e) {
        failures.push(`publish ${workspace.handle}: ${errText(e)}`)
      }
    }
    setPublishedApps(apps)
    try {
      const listed = await daemonCliGet<unknown>(primaryScope(), 'skin/grants')
      setGrants(parseSkinGrants(listed))
    } catch (e) {
      failures.push(`grants: ${errText(e)}`)
      setGrants([])
    }
    try {
      const h = await daemonCliGet<unknown>(primaryScope(), 'skin/hydra')
      setHydra(parseHydra(h))
    } catch (e) {
      failures.push(`hydra: ${errText(e)}`)
      setHydra(DEFAULT_HYDRA)
    }
    if (failures.length) setError(failures.join(' · '))
    setLoading(false)
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const mintKey = useCallback(async () => {
    const name = mintName.trim().toLowerCase()
    if (!name) {
      setMintError('Name is required')
      return
    }
    const caps = [...mintCaps]
    if (caps.length === 0) {
      setMintError('Pick at least one scope')
      return
    }
    const rooms = [...mintRooms]
    if (rooms.length === 0) {
      setMintError('Pick at least one agent')
      return
    }
    setMintBusy(true)
    setMintError(null)
    try {
      const res = await daemonCliPost<unknown>(primaryScope(), 'skin-tokens', { name, caps, rooms })
      const secret = mintSecretFrom(res)
      if (!secret) throw new Error('mint returned no secret')
      setMintedSecret(secret)
      await refresh()
    } catch (e) {
      setMintError(errText(e))
    } finally {
      setMintBusy(false)
    }
  }, [mintName, mintCaps, mintRooms, refresh])

  const createRole = useCallback(async () => {
    const name = newRoleName.trim().toLowerCase()
    if (!name) {
      setRoleError('Name is required')
      return
    }
    setRoleBusy(true)
    setRoleError(null)
    try {
      const roomAccess = [...newRoleRooms].map((handle) => ({
        handle,
        caps: [...(newRoleCapsByRoom[handle] ?? new Set(DEFAULT_SKIN_CAPS))],
      }))
      const body: { name: string; roomAccess: { handle: string; caps: string[] }[]; appId?: string } = {
        name,
        roomAccess,
      }
      if (selectedApp && selectedApp !== 'host') body.appId = selectedApp
      await daemonCliPost(primaryScope(), 'skin/roles', body)
      setNewRoleName('')
      setNewRoleRooms(new Set())
      setNewRoleCapsByRoom({})
      await refresh()
    } catch (e) {
      setRoleError(errText(e))
    } finally {
      setRoleBusy(false)
    }
  }, [newRoleName, newRoleRooms, newRoleCapsByRoom, refresh, selectedApp])

  const saveRole = useCallback(
    async (id: string, handles: string[], capsByRoom: Record<string, Set<string>>) => {
      setRoleError(null)
      setRoleBusy(true)
      try {
        const roomAccess = handles.map((handle) => ({
          handle,
          caps: [...(capsByRoom[handle] ?? new Set(DEFAULT_SKIN_CAPS))],
        }))
        await daemonCliPost(primaryScope(), 'skin/roles/update', { id, roomAccess })
        setEditRoleId(null)
        await refresh()
      } catch (e) {
        setRoleError(errText(e))
      } finally {
        setRoleBusy(false)
      }
    },
    [refresh],
  )

  const removeRole = useCallback(
    async (id: string) => {
      setRoleError(null)
      try {
        await daemonCliPost(primaryScope(), 'skin/roles/remove', { id })
        setRemoveRoleConfirm(null)
        await refresh()
      } catch (e) {
        setRoleError(errText(e))
      }
    },
    [refresh],
  )

  const setUserRole = useCallback(
    async (username: string, role: string | null) => {
      setError(null)
      try {
        if (role) {
          await daemonCliPost(primaryScope(), 'skin/roles/assign', { username, role })
        } else {
          await daemonCliPost(primaryScope(), 'skin/roles/unassign', { username })
        }
        await refresh()
      } catch (e) {
        setError(errText(e))
      }
    },
    [refresh],
  )

  const saveKeyRooms = useCallback(
    async (id: string, handles: string[]) => {
      setMintError(null)
      setBusyId(id)
      try {
        await daemonCliPost(primaryScope(), 'skin-tokens/rooms', { id, rooms: handles })
        setEditKeyId(null)
        await refresh()
      } catch (e) {
        setMintError(errText(e))
      } finally {
        setBusyId(null)
      }
    },
    [refresh],
  )

  const darkKeys = useMemo(
    () => tokens.filter((t) => t.rooms.length === 0 && t.roomHandles.length === 0),
    [tokens],
  )

  const visibleRoles = useMemo(() => {
    if (selectedApp == null) return []
    if (selectedApp === 'host') return roles.filter((r) => !r.appId)
    return roles.filter((r) => r.appId === selectedApp)
  }, [roles, selectedApp])

  const selectedPublished = publishedApps.find((app) => app.id === selectedApp) ?? null

  const appGrants = useMemo(() => {
    if (!selectedApp) return []
    const target = selectedApp === 'host' ? 'host' : selectedApp
    return grants.filter((g) => g.kind === 'app' && g.targetId === target)
  }, [grants, selectedApp])

  const publishAction = useCallback(
    async (action: 'start' | 'stop', app: PublishedApp) => {
      setPublishBusy(app.id)
      setError(null)
      try {
        await daemonCliPost(primaryScope(), `publish/${action}`, { name: app.name, project: app.projectId })
        await refresh()
      } catch (e) {
        setError(errText(e))
      } finally {
        setPublishBusy(null)
      }
    },
    [refresh],
  )

  const setGrantEnabled = useCallback(
    async (grant: SkinGrantRow, enabled: boolean) => {
      setBusyId(grant.id)
      setError(null)
      try {
        await daemonCliPost(primaryScope(), 'skin/grants/enabled', { id: grant.id, enabled })
        await refresh()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusyId(null)
      }
    },
    [refresh],
  )

  const revokeGrant = useCallback(
    async (id: string) => {
      setBusyId(id)
      setError(null)
      try {
        await daemonCliPost(primaryScope(), 'skin/grants/delete', { id })
        await refresh()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusyId(null)
      }
    },
    [refresh],
  )

  const persistHydra = useCallback(
    async (enabled: boolean) => {
      if (!hydra.supported) {
        setError(hydra.hint ?? HYDRA_LINUX_BANNER)
        return
      }
      setHydraBusy(true)
      setError(null)
      const prev = hydra
      setHydra((h) => ({ ...h, enabled }))
      try {
        const posted = await daemonCliPost<unknown>(primaryScope(), 'skin/hydra', { enabled, apply: true })
        setHydra(parseHydra(posted))
      } catch (e) {
        setHydra(prev)
        setError(`hydra: ${errText(e)}`)
      } finally {
        setHydraBusy(false)
      }
    },
    [hydra],
  )

  const revokeKey = useCallback(
    async (id: string) => {
      const ok = window.confirm(
        'Permanently revoke this skin key? It cannot be re-enabled — mint a new secret.',
      )
      if (!ok) return
      setBusyId(id)
      setMintError(null)
      try {
        await daemonCliPost(primaryScope(), 'skin-tokens/revoke', { id })
        await refresh()
      } catch (e) {
        setMintError(errText(e))
      } finally {
        setBusyId(null)
      }
    },
    [refresh],
  )

  const addAppUser = useCallback(async () => {
    if (!selectedApp || !addPersonId || !addRoleId) {
      setError('Pick a person and a role')
      return
    }
    setBusyId('add-user')
    setError(null)
    try {
      await daemonCliPost(primaryScope(), 'skin/grants', {
        subjectKind: 'principal',
        subjectId: addPersonId,
        kind: 'app',
        targetId: selectedApp === 'host' ? 'host' : selectedApp,
        roleId: addRoleId,
      })
      setAddPersonId('')
      setAddRoleId('')
      await refresh()
    } catch (e) {
      setError(errText(e))
    } finally {
      setBusyId(null)
    }
  }, [selectedApp, addPersonId, addRoleId, refresh])

  const peopleChoices = users.flatMap((u) => (u.id ? [{ value: u.id, label: u.username }] : []))
  const appRoleChoices = visibleRoles.map((r) => ({ value: r.id, label: r.name }))
  const loginRoleChoices = [
    { value: '', label: 'None' },
    ...visibleRoles.map((r) => ({ value: r.name, label: r.name })),
  ]
  const workspaceLabel = selectedPublished
    ? (workspaces.find((w) => w.id === selectedPublished.projectId)?.name ?? selectedPublished.projectId)
    : ''

  return (
    <div className="flex h-full min-h-0">
      <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col min-h-0">
        <div className="px-3 pt-3 pb-2 border-b border-[var(--color-border)]">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Apps</h2>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
            Host plus each published service. Not Admin Access.
          </p>
        </div>
        <div className="flex-1 overflow-y-auto min-h-0">
          {loading ? <p className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">Loading…</p> : null}
          <button
            type="button"
            aria-label="Host"
            className={`w-full text-left px-3 py-2 text-xs no-drag cursor-pointer ${
              selectedApp === 'host' ? 'bg-[var(--color-accent)]/15' : 'hover:bg-white/[0.03]'
            }`}
            onClick={() => setSelectedApp('host')}
          >
            Host
          </button>
          {publishedApps.map((app) => (
            <button
              key={app.id}
              type="button"
              aria-label={`Open ${app.name}`}
              className={`w-full text-left px-3 py-2 no-drag cursor-pointer ${
                selectedApp === app.id ? 'bg-[var(--color-accent)]/15' : 'hover:bg-white/[0.03]'
              }`}
              onClick={() => setSelectedApp(app.id)}
            >
              <span className="block text-xs font-mono text-[var(--color-text-primary)] truncate">
                {app.name}
              </span>
              <span className="block text-[10px] text-[var(--color-text-muted)]">
                {app.kind}
                {PUBLISH_STATUS.has(app.status) ? ` · ${app.status}` : ''}
                {app.expose === 'local' ? ' · local-only' : ''}
              </span>
            </button>
          ))}
        </div>
      </div>
      <div className="flex-1 overflow-y-auto p-6 min-h-0">
        {error && (
          <p role="alert" className="text-[11px] text-[var(--color-status-error-soft)] max-w-2xl mb-3">
            {error}
          </p>
        )}
        {selectedApp == null ? (
          <p className="text-[10px] text-[var(--color-text-muted)]">Select Host or an app.</p>
        ) : (
          <div className="max-w-2xl space-y-6">
            {selectedPublished ? (
              <div className="space-y-2">
                <div className="text-xs font-mono text-[var(--color-text-primary)]">
                  {selectedPublished.name}
                </div>
                <p className="text-[10px] text-[var(--color-text-muted)]">
                  {workspaceLabel}
                  {` · ${selectedPublished.kind}`}
                  {PUBLISH_STATUS.has(selectedPublished.status) ? ` · ${selectedPublished.status}` : ''}
                  {selectedPublished.expose === 'local'
                    ? ' · local-only'
                    : selectedPublished.url
                      ? ` · ${selectedPublished.url}`
                      : ''}
                </p>
                <div className="flex gap-2">
                  <button
                    type="button"
                    className="px-3 py-1 text-[11px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
                    disabled={publishBusy === selectedPublished.id}
                    onClick={() => void publishAction('start', selectedPublished)}
                  >
                    Start {selectedPublished.name}
                  </button>
                  <button
                    type="button"
                    className="px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] no-drag cursor-pointer disabled:opacity-60"
                    disabled={publishBusy === selectedPublished.id}
                    onClick={() => void publishAction('stop', selectedPublished)}
                  >
                    Stop {selectedPublished.name}
                  </button>
                </div>
              </div>
            ) : (
              <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
                Host has no publish row. Login role is principals.role_id. Host roles keep app_id empty.
              </p>
            )}

            <SettingsGroup title="Roles">
          <div data-settings-id="skin-access.roles" className="space-y-3">
            <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
              Skin roles are not Connect owner/admin/member/viewer. They never include the
              terminal. Access is per agent. Files on Documents does not grant files on Anna.
              Adding a room starts Thread-only. Zero agents is Thread dark. Platform tokens
              stay flat.
            </p>
            <form
              className="space-y-2"
              onSubmit={(e) => {
                e.preventDefault()
                void createRole()
              }}
            >
              <input
                className={INPUT_CLS}
                placeholder="name (dentist)"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                value={newRoleName}
                onChange={(e) => setNewRoleName(e.target.value)}
                aria-label="New skin role name"
              />
              {workspaces.length > 0 ? (
                <div className="space-y-2">
                  {workspaces.map((ws) => {
                    const included = newRoleRooms.has(ws.handle) || newRoleRooms.has(ws.id)
                    const caps = newRoleCapsByRoom[ws.handle] ?? new Set(DEFAULT_SKIN_CAPS)
                    return (
                      <div key={`role-mint-${ws.id}`} className="space-y-1">
                        <label className="flex items-center gap-1.5 cursor-pointer select-none no-drag">
                          <SquareCheckbox
                            aria-label={`Role agent ${ws.handle}`}
                            checked={included}
                            onChange={(e) => {
                              setNewRoleRooms((prev) => {
                                const next = new Set(prev)
                                if (e.target.checked) next.add(ws.handle)
                                else {
                                  next.delete(ws.handle)
                                  next.delete(ws.id)
                                }
                                return next
                              })
                              setNewRoleCapsByRoom((prev) => {
                                const next = { ...prev }
                                if (e.target.checked) {
                                  next[ws.handle] = new Set(DEFAULT_SKIN_CAPS)
                                } else {
                                  delete next[ws.handle]
                                }
                                return next
                              })
                            }}
                          />
                          <span className="text-[10px] font-mono text-[var(--color-text-secondary)]">
                            {ws.handle}
                          </span>
                        </label>
                        {included ? (
                          <div className="flex flex-wrap gap-x-3 gap-y-1 pl-5">
                            {capChoices.map((cap) => (
                              <label
                                key={`role-${ws.handle}-${cap}`}
                                title={SKIN_CAP_LABELS[cap]}
                                className="flex items-center gap-1.5 cursor-pointer select-none no-drag"
                              >
                                <SquareCheckbox
                                  aria-label={`Role ${ws.handle} ${cap}`}
                                  checked={caps.has(cap)}
                                  onChange={(e) => {
                                    setNewRoleCapsByRoom((prev) => {
                                      const nextSet = new Set(prev[ws.handle] ?? DEFAULT_SKIN_CAPS)
                                      if (e.target.checked) nextSet.add(cap)
                                      else nextSet.delete(cap)
                                      return { ...prev, [ws.handle]: nextSet }
                                    })
                                  }}
                                />
                                <span className="text-[10px] font-mono text-[var(--color-text-secondary)]">
                                  {cap}
                                </span>
                              </label>
                            ))}
                          </div>
                        ) : null}
                      </div>
                    )
                  })}
                </div>
              ) : (
                <p className="text-[10px] text-[var(--color-text-muted)]">
                  No workspaces yet — a role with zero agents is Thread dark.
                </p>
              )}
              <button
                type="submit"
                disabled={roleBusy || !newRoleName.trim()}
                className="px-3 py-1 text-[11px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
              >
                {roleBusy ? 'Saving…' : 'Create role'}
              </button>
            </form>
            {roleError && (
              <p role="alert" className="text-[11px] text-[var(--color-status-error-soft)]">
                {roleError}
              </p>
            )}
            {loading ? (
              <p className="text-[10px] text-[var(--color-text-muted)]">Loading roles…</p>
            ) : visibleRoles.length === 0 ? (
              <p className="text-[10px] text-[var(--color-text-muted)]">
                No skin roles yet. Create one above — not owner/admin/member/viewer.
              </p>
            ) : (
              <div className="divide-y divide-[var(--color-border)]">
                {visibleRoles.map((r) => {
                  const editing = editRoleId === r.id
                  return (
                    <div key={r.id} className="py-2 space-y-2">
                      <div className="flex items-center justify-between gap-3">
                        <span className="text-xs font-mono text-[var(--color-text-primary)]">
                          {r.name}
                        </span>
                        {removeRoleConfirm === r.id ? (
                          <span className="flex items-center gap-1.5 flex-shrink-0">
                            <button
                              type="button"
                              onClick={() => void removeRole(r.id)}
                              className="text-[10px] text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
                            >
                              Confirm remove role
                            </button>
                            <button
                              type="button"
                              onClick={() => setRemoveRoleConfirm(null)}
                              className="text-[10px] text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer"
                            >
                              Cancel
                            </button>
                          </span>
                        ) : (
                          <span className="flex items-center gap-2 flex-shrink-0">
                            <button
                              type="button"
                              onClick={() => {
                                setEditRoleId(r.id)
                                setEditRoleRooms(new Set(r.roomHandles))
                                const byRoom: Record<string, Set<string>> = {}
                                for (const row of r.roomAccess) {
                                  byRoom[row.handle] = new Set(row.caps)
                                }
                                setEditRoleCapsByRoom(byRoom)
                              }}
                              className="text-[10px] text-[var(--color-accent)] hover:underline no-drag cursor-pointer"
                            >
                              Edit role
                            </button>
                            <button
                              type="button"
                              onClick={() => setRemoveRoleConfirm(r.id)}
                              className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
                            >
                              Remove role
                            </button>
                          </span>
                        )}
                      </div>
                      {editing ? (
                        <div className="space-y-2">
                          <div className="space-y-2">
                            {workspaces.map((ws) => {
                              const included =
                                editRoleRooms.has(ws.handle) || editRoleRooms.has(ws.id)
                              const caps =
                                editRoleCapsByRoom[ws.handle] ?? new Set(DEFAULT_SKIN_CAPS)
                              return (
                                <div key={`edit-role-${r.id}-${ws.id}`} className="space-y-1">
                                  <label className="flex items-center gap-1.5 cursor-pointer select-none no-drag">
                                    <SquareCheckbox
                                      aria-label={`Edit role ${r.name} agent ${ws.handle}`}
                                      checked={included}
                                      onChange={(e) => {
                                        setEditRoleRooms((prev) => {
                                          const next = new Set(prev)
                                          if (e.target.checked) next.add(ws.handle)
                                          else {
                                            next.delete(ws.handle)
                                            next.delete(ws.id)
                                          }
                                          return next
                                        })
                                        setEditRoleCapsByRoom((prev) => {
                                          const next = { ...prev }
                                          if (e.target.checked) {
                                            next[ws.handle] = new Set(DEFAULT_SKIN_CAPS)
                                          } else {
                                            delete next[ws.handle]
                                          }
                                          return next
                                        })
                                      }}
                                    />
                                    <span className="text-[10px] font-mono text-[var(--color-text-secondary)]">
                                      {ws.handle}
                                    </span>
                                  </label>
                                  {included ? (
                                    <div className="flex flex-wrap gap-x-3 gap-y-1 pl-5">
                                      {capChoices.map((cap) => (
                                        <label
                                          key={`edit-role-${r.name}-${ws.handle}-${cap}`}
                                          title={SKIN_CAP_LABELS[cap]}
                                          className="flex items-center gap-1.5 cursor-pointer select-none no-drag"
                                        >
                                          <SquareCheckbox
                                            aria-label={`Edit role ${r.name} ${ws.handle} ${cap}`}
                                            checked={caps.has(cap)}
                                            onChange={(ev) => {
                                              setEditRoleCapsByRoom((prev) => {
                                                const nextSet = new Set(
                                                  prev[ws.handle] ?? DEFAULT_SKIN_CAPS,
                                                )
                                                if (ev.target.checked) nextSet.add(cap)
                                                else nextSet.delete(cap)
                                                return { ...prev, [ws.handle]: nextSet }
                                              })
                                            }}
                                          />
                                          <span className="text-[10px] font-mono text-[var(--color-text-secondary)]">
                                            {cap}
                                          </span>
                                        </label>
                                      ))}
                                    </div>
                                  ) : null}
                                </div>
                              )
                            })}
                          </div>
                          <div className="flex gap-2">
                            <button
                              type="button"
                              className="text-[10px] text-[var(--color-accent)] cursor-pointer"
                              onClick={() =>
                                void saveRole(r.id, [...editRoleRooms], editRoleCapsByRoom)
                              }
                            >
                              Save role
                            </button>
                            <button
                              type="button"
                              className="text-[10px] text-[var(--color-text-muted)] cursor-pointer"
                              onClick={() => setEditRoleId(null)}
                            >
                              Cancel
                            </button>
                          </div>
                        </div>
                      ) : (
                        <div className="space-y-1">
                          {r.roomAccess.length === 0 ? (
                            <span className="text-[10px] text-[var(--color-text-muted)]">
                              no rooms
                            </span>
                          ) : (
                            r.roomAccess.map((row) => (
                              <div key={row.handle} className="flex flex-wrap items-center gap-1">
                                <span className="text-[9px] font-mono px-1.5 py-0.5 bg-[var(--color-bg-surface)] text-[var(--color-text-secondary)] border border-[var(--color-border)]">
                                  {row.handle}
                                </span>
                                {row.caps.map((cap) => (
                                  <span
                                    key={cap}
                                    className="text-[9px] font-mono uppercase tracking-wider px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-text-secondary)]"
                                  >
                                    {cap}
                                  </span>
                                ))}
                              </div>
                            ))
                          )}
                        </div>
                      )}
                    </div>
                  )
                })}
              </div>
            )}
          </div>
        </SettingsGroup>

            <SettingsGroup title="Users">
              {appGrants.length === 0 ? (
                <p className="text-[10px] text-[var(--color-text-muted)]">No grants on this app.</p>
              ) : (
                <div className="divide-y divide-[var(--color-border)]">
                  {appGrants.map((grant) => {
                    const person = users.find((u) => u.id != null && u.id === grant.subjectId)
                    const rooms = person
                      ? (person.defaultRoomHandles.length ? person.defaultRoomHandles : person.defaultRooms)
                      : []
                    const grantRole = grant.roleId
                      ? roles.find((r) => r.id === grant.roleId)
                      : null
                    return (
                      <div key={grant.id} className="py-2 space-y-2">
                        <div className="flex items-center justify-between gap-2">
                          <span className="text-xs text-[var(--color-text-primary)]">
                            {grantSubjectLabel(grant, users, workspaces)}
                            {grant.subjectKind === 'workspace' ? ' · workspace' : ''}
                          </span>
                          <span className="flex items-center gap-2">
                            <button
                              type="button"
                              role="switch"
                              aria-checked={grant.enabled}
                              aria-label={`Enable grant ${grant.id}`}
                              disabled={busyId === grant.id}
                              onClick={() => void setGrantEnabled(grant, !grant.enabled)}
                              className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
                                grant.enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
                              }`}
                            >
                              <span
                                className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
                                  grant.enabled ? 'translate-x-3.5' : 'translate-x-0.5'
                                }`}
                              />
                            </button>
                            <button
                              type="button"
                              aria-label={`Revoke grant ${grant.id}`}
                              className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
                              onClick={() => void revokeGrant(grant.id)}
                            >
                              Revoke
                            </button>
                          </span>
                        </div>
                        {selectedApp === 'host' && grant.subjectKind === 'principal' && person ? (
                          <div className="space-y-1">
                            {person.roleId || person.roleName ? null : (
                              <p className="text-[10px] text-[var(--color-text-muted)]">
                                default rooms: {rooms.length ? rooms.join(', ') : 'none'}
                              </p>
                            )}
                            <SettingRow label="Login role">
                              <SettingDropdown
                                ariaLabel={`${person.username} login role`}
                                value={person.roleName ?? ''}
                                placeholder="None"
                                options={loginRoleChoices}
                                onChange={(next) => void setUserRole(person.username, next.trim() ? next : null)}
                                menuAlign="right"
                              />
                            </SettingRow>
                          </div>
                        ) : selectedApp !== 'host' && grantRole ? (
                          <p className="text-[10px] text-[var(--color-text-muted)]">{grantRole.name}</p>
                        ) : null}
                      </div>
                    )
                  })}
                </div>
              )}
              <SettingRow label="Person">
                <SettingDropdown
                  ariaLabel="App user"
                  value={addPersonId}
                  placeholder="person"
                  options={peopleChoices}
                  onChange={setAddPersonId}
                  menuAlign="right"
                />
              </SettingRow>
              <SettingRow label="Role">
                <SettingDropdown
                  ariaLabel="App user role"
                  value={addRoleId}
                  placeholder="role"
                  options={appRoleChoices}
                  onChange={setAddRoleId}
                  menuAlign="right"
                />
              </SettingRow>
              <button
                type="button"
                className="px-3 py-1 text-[11px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
                disabled={busyId === 'add-user' || !addPersonId || !addRoleId}
                onClick={() => void addAppUser()}
              >
                Add user
              </button>
            </SettingsGroup>

            {selectedApp === 'host' ? (
              <>
            {selectedApp === 'host' && !bannerDismissed && darkKeys.length > 0 ? (
              <div role="status" className="border border-[var(--color-status-warning-soft)]/40 bg-[var(--color-status-warning-soft)]/10 p-3 space-y-2">
                <p className="text-[11px] text-[var(--color-text-primary)]">
                  Assign agents or these platform tokens go dark. Tokens with no rooms cannot Thread.
                </p>
                <button type="button" className="text-[10px] text-[var(--color-accent)] cursor-pointer" onClick={() => setBannerDismissed(true)}>
                  Dismiss
                </button>
              </div>
            ) : null}
<SettingsGroup title="Platform tokens">
          <div data-settings-id="skin-access.keys" className="space-y-3">
            <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
              Platform tokens are labels (vercel), not guests. The raw secret is shown only
              once when minted. Prefix <code className="text-[10px]">k2skn_</code> — not{' '}
              <code className="text-[10px]">k2sk_</code> API keys.{' '}
              <code className="text-[10px]">thread:read</code> includes overlay WS.{' '}
              <code className="text-[10px]">files:read</code> lists/reads that agent's folder
              and <code className="text-[10px]">/cli/fs/events</code>. Write-only does not
              grant list. Do not mint for this user.
            </p>
            <div className="space-y-2">
              <input
                className={INPUT_CLS}
                placeholder="name (vercel)"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                value={mintName}
                onChange={(e) => setMintName(e.target.value)}
                aria-label="Platform token name"
              />
              <div className="flex flex-wrap gap-x-4 gap-y-1">
                {capChoices.map((cap) => (
                  <label
                    key={cap}
                    title={SKIN_CAP_LABELS[cap]}
                    className="flex items-center gap-1.5 cursor-pointer select-none no-drag"
                  >
                    <SquareCheckbox
                      aria-label={`Mint cap ${cap}`}
                      checked={mintCaps.has(cap)}
                      onChange={(e) => {
                        setMintCaps((prev) => {
                          const next = new Set(prev)
                          if (e.target.checked) next.add(cap)
                          else next.delete(cap)
                          return next
                        })
                      }}
                    />
                    <span className="text-[10px] font-mono text-[var(--color-text-secondary)]">
                      {cap}
                    </span>
                  </label>
                ))}
              </div>
              {workspaces.length > 0 ? (
                <div className="flex flex-wrap gap-x-3 gap-y-1">
                  {workspaces.map((ws) => (
                    <label
                      key={`mint-${ws.id}`}
                      className="flex items-center gap-1.5 cursor-pointer select-none no-drag"
                    >
                      <SquareCheckbox
                        aria-label={`Mint agent ${ws.handle}`}
                        checked={mintRooms.has(ws.handle) || mintRooms.has(ws.id)}
                        onChange={(e) => {
                          setMintRooms((prev) => {
                            const next = new Set(prev)
                            if (e.target.checked) next.add(ws.handle)
                            else {
                              next.delete(ws.handle)
                              next.delete(ws.id)
                            }
                            return next
                          })
                        }}
                      />
                      <span className="text-[10px] font-mono text-[var(--color-text-secondary)]">
                        {ws.handle}
                      </span>
                      {ws.name && ws.name !== ws.handle ? (
                        <span className="text-[10px] text-[var(--color-text-muted)]">{ws.name}</span>
                      ) : null}
                    </label>
                  ))}
                </div>
              ) : (
                <p className="text-[10px] text-[var(--color-text-muted)]">
                  No workspaces on this box yet — add one before minting a key.
                </p>
              )}
              <button
                type="button"
                disabled={mintBusy || !mintName.trim() || mintRooms.size === 0}
                onClick={() => void mintKey()}
                className="px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50"
              >
                {mintBusy ? 'Minting…' : 'Mint key'}
              </button>
            </div>
            {mintError && (
              <p role="alert" className="text-[11px] text-[var(--color-status-error-soft)]">
                {mintError}
              </p>
            )}
            {mintedSecret && (
              <div className="border border-[var(--color-status-warning-soft)]/40 bg-[var(--color-status-warning-soft)]/10 p-3 space-y-2">
                <p className="text-[11px] font-semibold text-[var(--color-text-primary)]">
                  Store this key now — it cannot be retrieved again
                </p>
                <code className="block text-[11px] break-all select-all text-[var(--color-text-primary)]">
                  {mintedSecret}
                </code>
                <button
                  type="button"
                  className="text-[11px] text-[var(--color-accent)] cursor-pointer"
                  onClick={() => {
                    void navigator.clipboard?.writeText(mintedSecret)
                  }}
                >
                  Copy to clipboard
                </button>
              </div>
            )}
            {loading ? (
              <p className="text-[11px] text-[var(--color-text-muted)]">Loading keys…</p>
            ) : tokens.length === 0 ? (
              <p className="text-[11px] text-[var(--color-text-muted)]">
                No platform tokens yet. Mint one with a name — not for a user.
              </p>
            ) : (
              <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
                {tokens.map((k) => {
                  const busy = busyId === k.id
                  return (
                    <div
                      key={k.id}
                      className="p-3 flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between"
                    >
                      <div className="min-w-0 space-y-1">
                        <div className="flex items-center gap-2 flex-wrap">
                          <span className="text-[12px] font-mono text-[var(--color-text-primary)]">
                            {prefixLabel(k.prefix)}
                          </span>
                          {k.name && (
                            <span className="text-[11px] text-[var(--color-text-secondary)]">
                              {k.name}
                            </span>
                          )}
                        </div>
                        <div className="flex flex-wrap gap-1">
                          {k.caps.length === 0 ? (
                            <span className="text-[10px] text-[var(--color-text-muted)]">
                              no scopes
                            </span>
                          ) : (
                            k.caps.map((cap) => (
                              <span
                                key={cap}
                                className="text-[9px] font-mono uppercase tracking-wider px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-text-secondary)]"
                              >
                                {cap}
                              </span>
                            ))
                          )}
                        </div>
                        <div className="flex flex-wrap gap-1">
                          {k.roomHandles.length === 0 ? (
                            <span className="text-[10px] text-[var(--color-text-muted)]">
                              no rooms
                            </span>
                          ) : (
                            k.roomHandles.map((h) => (
                              <span
                                key={h}
                                className="text-[9px] font-mono px-1.5 py-0.5 bg-[var(--color-bg-surface)] text-[var(--color-text-secondary)] border border-[var(--color-border)]"
                              >
                                {h}
                              </span>
                            ))
                          )}
                        </div>
                        {editKeyId === k.id ? (
                          <div className="space-y-1">
                            <div className="flex flex-wrap gap-x-3 gap-y-1">
                              {workspaces.map((ws) => (
                                <label
                                  key={`edit-${k.id}-${ws.id}`}
                                  className="flex items-center gap-1.5 cursor-pointer select-none no-drag"
                                >
                                  <SquareCheckbox
                                    aria-label={`Key ${k.id} agent ${ws.handle}`}
                                    checked={editKeyRooms.has(ws.handle) || editKeyRooms.has(ws.id)}
                                    onChange={(e) => {
                                      setEditKeyRooms((prev) => {
                                        const next = new Set(prev)
                                        if (e.target.checked) next.add(ws.handle)
                                        else {
                                          next.delete(ws.handle)
                                          next.delete(ws.id)
                                        }
                                        return next
                                      })
                                    }}
                                  />
                                  <span className="text-[10px] font-mono text-[var(--color-text-secondary)]">
                                    {ws.handle}
                                  </span>
                                </label>
                              ))}
                            </div>
                            <div className="flex gap-2">
                              <button
                                type="button"
                                className="text-[10px] text-[var(--color-accent)] cursor-pointer"
                                onClick={() => void saveKeyRooms(k.id, [...editKeyRooms])}
                              >
                                Save rooms
                              </button>
                              <button
                                type="button"
                                className="text-[10px] text-[var(--color-text-muted)] cursor-pointer"
                                onClick={() => setEditKeyId(null)}
                              >
                                Cancel
                              </button>
                            </div>
                          </div>
                        ) : (
                          <button
                            type="button"
                            className="text-[10px] text-[var(--color-accent)] cursor-pointer"
                            onClick={() => {
                              setEditKeyId(k.id)
                              setEditKeyRooms(new Set(k.roomHandles))
                            }}
                          >
                            Edit rooms
                          </button>
                        )}
                      </div>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => void revokeKey(k.id)}
                        className="px-2 py-1 text-[10px] border border-[var(--color-border)] text-[var(--color-status-error-soft)] hover:border-[var(--color-status-error-soft)] cursor-pointer disabled:opacity-50 flex-shrink-0"
                      >
                        Revoke
                      </button>
                    </div>
                  )
                })}
              </div>
            )}
          </div>
        </SettingsGroup>
<SettingsGroup title="OIDC issuer (Hydra)">
          <div data-settings-id="skin-access.hydra" className="space-y-2">
            {!hydra.supported ? (
              <p className="text-[10px] text-[var(--color-status-warn)] leading-relaxed">
                {HYDRA_LINUX_BANNER}
              </p>
            ) : null}
            <div className="flex items-center justify-between py-2">
              <div className="flex-1 min-w-0 mr-3">
                <span className="text-xs text-[var(--color-text-secondary)]">
                  Turn on — this box issues standard OIDC tickets
                </span>
                <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
                  {hydra.supported
                    ? hydra.hint ??
                      'Off. Enabling skins does not start Hydra. Subject = skin principal id; no users in Hydra.'
                    : 'Off. Enabling skins does not start Hydra. Subject = skin principal id; no users in Hydra.'}
                </p>
                {hydra.publicUrl || hydra.adminUrl ? (
                  <p className="text-[10px] font-mono text-[var(--color-text-muted)] mt-1">
                    {hydra.publicUrl ? `public ${hydra.publicUrl}` : null}
                    {hydra.publicUrl && hydra.adminUrl ? ' · ' : null}
                    {hydra.adminUrl ? `admin ${hydra.adminUrl}` : null}
                    {hydra.running ? ' · running' : ' · not running'}
                  </p>
                ) : null}
              </div>
              <Toggle
                checked={hydra.enabled}
                disabled={!hydra.supported || hydraBusy || loading}
                onChange={(on) => {
                  void persistHydra(on)
                }}
                aria-label="Turn on Hydra OIDC issuer"
              />
            </div>
          </div>
        </SettingsGroup>
              </>
            ) : null}

          </div>
        )}
      </div>
    </div>
  )
}
