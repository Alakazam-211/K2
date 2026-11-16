// Settings → Cores → Apps. Master-detail: This server (0.46.3: the Apps
// switch, box doctor, prepare and engine; AppsServerSection), Host, and each published
// service. Roles and app grants live on the selected detail. Platform
// tokens and Hydra are box-level and render on Host only.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { Button, Callout, Checkbox, Toggle, Input, OverflowMenu, Surface } from '@/components/ui'
import {
  SETTING_FIELD_WIDTH,
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SETTING_SERVER_REASON,
  SettingDropdown,
  SettingRow,
  SettingSwitchRow,
  SettingsGroup,
  SettingsLoadState,
} from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import { primaryScope } from '@/kessel/server-scope'
import { useServerSupports } from '@/lib/server-capabilities'
import { K2_CAPS } from '@/lib/k2-caps.generated'
import { AppsServerSection } from './AppsServerSection'
import { AppAboutCard, AppsBoxSection } from './AppsBoxSection'
import { AppsResourcesSection } from './AppsResourcesSection'
import { AppsCatalogSection } from './AppsCatalogSection'
import { AppPeopleAccess } from './AppPeopleAccess'
import { AppPhonePairing } from './AppPhonePairing'
import { AppReleases } from './AppReleases'
import { LinuxOnlyNote } from './LinuxOnlyNote'
import { ConfirmSheet } from './AccessDialogs'
import { ZONE_SUFFIX, boxHost } from '@/lib/hosts'
import {
  APPS_CONTENT_WIDTH,
  APPS_LIST_ROW,
  APPS_ROW_OFF,
  APPS_ROW_ON,
  AppMonogram,
  AvatarFrame,
  PaneHeader,
  SelectedBar,
  StatusDot,
  appStateWords,
} from './apps-ui'

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
    label: 'Keys for other sites',
    description: 'Keys that let another site or service read or post in an agent’s Thread; each is shown once. Not for people.',
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
    group: 'Keys for other sites',
  },
  {
    id: 'skin-access.hydra',
    section: 'skin-access',
    label: 'Sign-in for other sites',
    description: 'Let other sites sign people in with this server (OpenID Connect). Linux servers only; off unless you turn it on.',
    keywords: ['oidc', 'hydra', 'issuer', 'openid', 'skin', 'app'],
    group: 'Sign-in for other sites',
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
// Own group (0.45.5 app identity surface). `agent:identity` = the room
// agent's name, color and picture; `agent:handle` = its address, a separate
// grant because a handle change breaks links. Neither implies the other.
// Offered only when the window's server supports `agent-identity`.
export const SKIN_AGENT_IDENTITY_CAPS = ['agent:identity', 'agent:handle'] as const
// Own group (0.46.3 Thread downloads, Rosson 2026-10-22): save Thread files.
// Never implied by thread:read or files:read, never a default, and a
// person's grant (an agent can't set it). Offered only on servers that
// accept it (`thread-download`, 0.46.3).
export const SKIN_DOWNLOAD_CAPS = ['files:download'] as const
// GH#72 (0.45.6): `files:write` stops at the agent's own `.k2/` folder.
// This opt-in modifier lifts that for one room. Off by default, owner only
// (an agent can't set it), and offered only beside a checked `files:write`
// — never in the plain cap grid.
export const SKIN_AGENT_CONFIG_CAP = 'files:agentconfig' as const
export const SKIN_AGENT_CONFIG_LABEL: string = K2_CAPS[SKIN_AGENT_CONFIG_CAP].label

/** Toggle `cap` in a room's cap set. Dropping `files:write` also drops the
 *  `.k2/` opt-in, and dropping `voice:join` drops `voice:video` and
 *  `voice:record` (each means nothing alone and the daemon refuses it);
 *  `voice:video` and `voice:record` bring `voice:join` with them. */
export function toggleSkinCap(prev: ReadonlySet<string>, cap: string, on: boolean): Set<string> {
  const next = new Set(prev)
  if (on) {
    next.add(cap)
    if (cap === 'voice:video' || cap === 'voice:record') next.add('voice:join')
  } else {
    next.delete(cap)
    if (cap === 'files:write') next.delete(SKIN_AGENT_CONFIG_CAP)
    if (cap === 'voice:join') {
      next.delete('voice:video')
      next.delete('voice:record')
    }
  }
  return next
}

/** The "Also allow writing the agent's .k2 folder" box, shown under a
 *  checked `files:write`. Renders nothing otherwise. */
function AgentConfigOption({
  caps,
  enabled,
  ariaLabel,
  onChange,
}: {
  caps: ReadonlySet<string>
  enabled: boolean
  ariaLabel: string
  onChange: (on: boolean) => void
}): React.JSX.Element | null {
  if (!enabled || !caps.has('files:write')) return null
  return (
    <label
      title="Lets this app's guests change the agent's own instructions, inbox and settings. Leave off unless you trust them with the agent itself."
      className="flex items-center gap-1.5 cursor-pointer select-none no-drag basis-full"
    >
      <Checkbox
        aria-label={ariaLabel}
        checked={caps.has(SKIN_AGENT_CONFIG_CAP)}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="type-hint text-[var(--color-text-secondary)]">{SKIN_AGENT_CONFIG_LABEL}</span>
    </label>
  )
}

export const SKIN_CAP_CHOICES = [...DEFAULT_SKIN_CAPS, ...SKIN_FILE_CAPS, ...SKIN_TICKET_CAPS, ...SKIN_WIKI_CAPS, ...SKIN_STORE_CAPS, ...SKIN_ACTIVITY_CAPS, ...SKIN_HEARTBEAT_CAPS, ...SKIN_AGENT_IDENTITY_CAPS, ...SKIN_DOWNLOAD_CAPS] as const
export type SkinCap = (typeof SKIN_CAP_CHOICES)[number]

/** What each cap lets an app do in a room — the checkbox tooltip. From the
 *  one shared cap table (prd-zen-user-widgets-v2 UWA9, `k2-caps.generated`),
 *  the same words the Garden review dialog's table holds. */
export const SKIN_CAP_LABELS: Record<SkinCap, string> = Object.fromEntries(
  SKIN_CAP_CHOICES.map((cap) => [cap, K2_CAPS[cap].label]),
) as Record<SkinCap, string>

/** AH24: the heartbeat caps are offered only when the server ships them;
 *  the agent identity and download caps likewise (an older server refuses
 *  an unknown cap). */
export function skinCapChoices(appHeartbeats: boolean, agentIdentity: boolean, fileDownload = true): readonly SkinCap[] {
  if (appHeartbeats && agentIdentity && fileDownload) return SKIN_CAP_CHOICES
  return SKIN_CAP_CHOICES.filter(
    (c) =>
      (appHeartbeats || !(SKIN_HEARTBEAT_CAPS as readonly string[]).includes(c)) &&
      (agentIdentity || !(SKIN_AGENT_IDENTITY_CAPS as readonly string[]).includes(c)) &&
      (fileDownload || !(SKIN_DOWNLOAD_CAPS as readonly string[]).includes(c)),
  )
}

// Sew for Apps A1 (0.46.6): the chat caps an App role carries into a
// person's Sew chat pass as its ceiling. App-wide (any room line of the role
// counts), never defaults, offered on role lines only (a platform token never
// mints a chat pass), and only when the server ships them (`app-chat-caps`).
export const SKIN_CHAT_CAPS = ['chat:read', 'chat:post', 'chat:respond', 'voice:join', 'voice:video'] as const
export type SkinChatCap = (typeof SKIN_CHAT_CAPS)[number]
// Sew 0.9.0 (recording in Apps): "Record", tied to "Voice" like video. A
// ceiling only: each person in the call still consents. Offered only by a
// server that reports `app-voice-record-v1` (its Sew 0.9.0 pin is in; an
// older server refuses the unknown cap).
export const SKIN_RECORD_CAP = 'voice:record' as const
export const VOICE_RECORD_FEATURE = 'app-voice-record-v1' as const

/** Every cap a role line may show, with its label. */
export const ROLE_CAP_LABELS: Readonly<Record<string, string>> = {
  ...SKIN_CAP_LABELS,
  ...Object.fromEntries(SKIN_CHAT_CAPS.map((cap) => [cap, K2_CAPS[cap].label])),
  [SKIN_RECORD_CAP]: K2_CAPS[SKIN_RECORD_CAP].label,
}

/** A role line's choices: the room caps, then the chat caps when the
 *  server ships them, then Record when it takes `voice:record`. */
export function roleCapChoices(base: readonly SkinCap[], appChat: boolean, record = false): readonly string[] {
  if (!appChat) return base
  return record ? [...base, ...SKIN_CHAT_CAPS, SKIN_RECORD_CAP] : [...base, ...SKIN_CHAT_CAPS]
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
  /** Signs in to this server itself (a `host` grant): a server login.
   *  Unknown (undefined) when the users list couldn't read it. */
  boxLogin?: boolean
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
  /** S24-15: an older `--skin` helper still signs guests in as the box login. */
  helperNeedsRestart?: boolean
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

const CONNECT_URL_STUB = `https://skin.${boxHost('<sub>')}`
const DIRECT_LISTEN_STUB = 'Caddy :443 (or LAN port) → 127.0.0.1:daemon'

const INPUT_CLS = 'w-full type-control'

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
  if (subdomain) return `https://skin.${boxHost(subdomain)}`
  const url = asString(rec.url)
  if (url) {
    const host = hostFromUrl(url)
    if (host?.endsWith(ZONE_SUFFIX)) {
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

/** A write that failed: what, the next step, and the server's reason. */
function failed(what: string, e: unknown): { what: string; next: string; detail: string } {
  return { what, next: 'Nothing changed. Try again in a moment.', detail: errText(e) }
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
      boxLogin: asBool(rec.boxLogin),
    }]
  })
}

/** The App's "Add a person" choices: everyone without a login on `app` yet
 *  (someone who has one changes their role on their row), App users first,
 *  then this server's own logins in their own group. Giving a server login a
 *  role here adds only their login on this App: their server sign-in stays
 *  as it is (the owner's `skin/grants` route never touches `host`). */
export function appPeopleChoices(
  users: SkinUser[],
  appGrants: { subjectKind: string; subjectId: string }[],
  app: string,
): { value: string; label: string; group?: string }[] {
  const has = new Set(appGrants.filter((g) => g.subjectKind === 'principal').map((g) => g.subjectId))
  const open = users.filter((u): u is SkinUser & { id: string } => !!u.id && !has.has(u.id))
  if (app === 'host') return open.map((u) => ({ value: u.id, label: u.username }))
  const appUsers = open.filter((u) => u.boxLogin !== true)
  const serverLogins = open.filter((u) => u.boxLogin === true)
  if (serverLogins.length === 0) return appUsers.map((u) => ({ value: u.id, label: u.username }))
  return [
    ...appUsers.map((u) => ({ value: u.id, label: u.username, group: 'App users' })),
    ...serverLogins.map((u) => ({ value: u.id, label: u.username, group: 'This server’s logins' })),
  ]
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
      ...(rec.helperNeedsRestart === true ? { helperNeedsRestart: true } : {}),
    }]
  })
}

/** Room ids in a role-less App grant's scope (a JSON array). Anything else → []. */
export function grantScopeRooms(scope: string | null): string[] {
  if (!scope) return []
  try {
    const parsed: unknown = JSON.parse(scope)
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === 'string') : []
  } catch {
    return []
  }
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
  const agentIdentity = useServerSupports('agent-identity')
  const agentConfigOptIn = useServerSupports('agent-config-write')
  const fileDownload = useServerSupports('thread-download')
  const phonePairing = useServerSupports('app-phone-pairing')
  const appChat = useServerSupports('app-chat-caps')
  const recordCap = useMemo(() => {
    try {
      return primaryScope().serverSupports(VOICE_RECORD_FEATURE)
    } catch {
      return false
    }
  }, [])
  const capChoices = useMemo(
    () => skinCapChoices(appHeartbeats, agentIdentity, fileDownload),
    [appHeartbeats, agentIdentity, fileDownload],
  )
  const roleChoices = useMemo(() => roleCapChoices(capChoices, appChat, recordCap), [capChoices, appChat, recordCap])
  const [users, setUsers] = useState<SkinUser[]>([])
  const [roles, setRoles] = useState<SkinRole[]>([])
  const [tokens, setTokens] = useState<SkinTokenRow[]>([])
  const [workspaces, setWorkspaces] = useState<SkinWorkspace[]>([])
  const [loading, setLoading] = useState(true)
  /** What failed (plain words), the next step and the server's reason. */
  const [error, setError] = useState<{ what: string; next: string; detail: string } | null>(null)
  const [ask, setAsk] = useState<{ title: string; message: string; confirmLabel: string; run: () => Promise<void> } | null>(null)
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
  const [publishedApps, setPublishedApps] = useState<PublishedApp[]>([])
  const [grants, setGrants] = useState<SkinGrantRow[]>([])
  const [selectedApp, setSelectedApp] = useState<string | null>(null)
  // 0.46.3: "This server" (Apps switch, box doctor, prepare, engine) is the
  // first entry and opens by default; Host / an app hide it.
  const [showServer, setShowServer] = useState(true)
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
      failures.push(`sign-in for other sites: ${errText(e)}`)
      setHydra(DEFAULT_HYDRA)
    }
    if (failures.length) {
      setError({
        what: 'K2 couldn’t read everything on this page',
        next: 'What did load is shown. Try again in a moment.',
        detail: failures.join(' · '),
      })
    }
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
      if (!secret) throw new Error('the server sent no key back')
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
        await refresh()
      } catch (e) {
        setError(failed('Couldn’t remove the role', e))
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
        setError(failed('Couldn’t change their role', e))
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
        setError(failed('Couldn’t save the key’s agents', e))
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
        setError(failed(action === 'start' ? `Couldn’t start ${app.name}` : `Couldn’t stop ${app.name}`, e))
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
        setError(failed(enabled ? 'Couldn’t turn their access on' : 'Couldn’t turn their access off', e))
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
        setError(failed('Couldn’t remove their access', e))
      } finally {
        setBusyId(null)
      }
    },
    [refresh],
  )

  const persistHydra = useCallback(
    async (enabled: boolean) => {
      if (!hydra.supported) return
      setHydraBusy(true)
      setError(null)
      const prev = hydra
      setHydra((h) => ({ ...h, enabled }))
      try {
        const posted = await daemonCliPost<unknown>(primaryScope(), 'skin/hydra', { enabled, apply: true })
        setHydra(parseHydra(posted))
      } catch (e) {
        setHydra(prev)
        setError(failed(enabled ? 'Couldn’t turn on sign-in for other sites' : 'Couldn’t turn off sign-in for other sites', e))
      } finally {
        setHydraBusy(false)
      }
    },
    [hydra],
  )

  const revokeKey = useCallback(
    async (id: string) => {
      setBusyId(id)
      setMintError(null)
      try {
        await daemonCliPost(primaryScope(), 'skin-tokens/revoke', { id })
        await refresh()
      } catch (e) {
        setError(failed('Couldn’t revoke the key', e))
      } finally {
        setBusyId(null)
      }
    },
    [refresh],
  )

  // S24-11: everyone with the box login gets the same access on this App.
  const copyHostLogins = useCallback(async () => {
    if (!selectedApp || selectedApp === 'host') return
    setBusyId('copy-host')
    setError(null)
    try {
      await daemonCliPost(primaryScope(), 'skin/grants/copy-host', { app: selectedApp })
      await refresh()
    } catch (e) {
      setError(failed('Couldn’t add everyone with the server sign-in', e))
    } finally {
      setBusyId(null)
    }
  }, [selectedApp, refresh])

  const addAppUser = useCallback(async () => {
    if (!selectedApp || !addPersonId || !addRoleId) return
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
      setError(failed('Couldn’t add them', e))
    } finally {
      setBusyId(null)
    }
  }, [selectedApp, addPersonId, addRoleId, refresh])

  const peopleChoices = appPeopleChoices(users, appGrants, selectedApp ?? 'host')
  const appRoleChoices = visibleRoles.map((r) => ({ value: r.id, label: r.name }))
  const loginRoleChoices = [
    { value: '', label: 'None' },
    ...visibleRoles.map((r) => ({ value: r.name, label: r.name })),
  ]
  const workspaceLabel = selectedPublished
    ? (workspaces.find((w) => w.id === selectedPublished.projectId)?.name ?? selectedPublished.projectId)
    : ''
  const handleName = (handle: string): string => workspaces.find((w) => w.handle === handle)?.name ?? handle

  const select = (next: 'server' | string): void => {
    setError(null)
    if (next === 'server') {
      setShowServer(true)
      return
    }
    setShowServer(false)
    setSelectedApp(next)
  }

  const railRow = (
    key: string,
    on: boolean,
    ariaLabel: string,
    lead: React.ReactNode,
    name: React.ReactNode,
    detail: React.ReactNode,
    onClick: () => void,
  ): React.JSX.Element => (
    <button
      key={key}
      type="button"
      data-k2-focus=""
      data-apps-rail={key}
      aria-label={ariaLabel}
      aria-current={on ? 'page' : undefined}
      className={`${APPS_LIST_ROW} ${on ? APPS_ROW_ON : APPS_ROW_OFF}`}
      onClick={onClick}
    >
      {on ? <SelectedBar /> : null}
      {lead}
      <span className="min-w-0 flex-1">
        <span className="block type-name text-[var(--color-text-primary)] truncate">{name}</span>
        <span className="flex items-start gap-1.5 type-hint text-[var(--color-text-secondary)] min-w-0 [&>[data-status-dot]]:mt-1">{detail}</span>
      </span>
    </button>
  )

  const running = selectedPublished ? selectedPublished.status === 'running' || selectedPublished.status === 'starting' : false
  const published = selectedPublished ? appStateWords(selectedPublished.status) : null

  return (
    <div className="flex h-full min-h-0">
      <aside aria-label="Apps" className="w-72 flex-shrink-0 min-h-0 flex flex-col border-r border-[var(--color-border)]" data-apps-rail-root="">
        <div className="flex-shrink-0 px-4 pt-6 pb-4">
          <h2 className="type-title text-[var(--color-text-primary)]">Apps</h2>
          <p className={SETTING_ROW_HINT}>Run your agents&rsquo; Apps on this server and choose who can sign in to each one.</p>
        </div>
        <div className="flex-1 min-h-0 overflow-y-auto px-4 pb-4 space-y-5 [scrollbar-gutter:stable]">
          <Surface chamfer material="tile" className="overflow-hidden">
            <div className="divide-y divide-[var(--color-border)]">
              {railRow(
                'server',
                showServer,
                'This server',
                <RailGlyph kind="server" />,
                'This server',
                'Switch, checks and disk use',
                () => select('server'),
              )}
              {railRow(
                'host',
                !showServer && selectedApp === 'host',
                'Host',
                <RailGlyph kind="host" />,
                'Server sign-in',
                'The server’s own sign-in',
                () => select('host'),
              )}
              {railRow(
                ADD_APP_PANE,
                !showServer && selectedApp === ADD_APP_PANE,
                'Add an App',
                <RailGlyph kind="add" />,
                'Add an App',
                'Hearth and Meet, in one step',
                () => select(ADD_APP_PANE),
              )}
            </div>
          </Surface>
          <section>
            <h3 className="type-heading text-[var(--color-text-primary)] mb-2">Published Apps</h3>
            {loading ? (
              <p className={SETTING_ROW_HINT}>Loading…</p>
            ) : publishedApps.length === 0 ? (
              <p className={SETTING_ROW_HINT} data-testid="apps-rail-empty">
                No Apps are published yet. An agent publishes one from its workspace.
              </p>
            ) : (
              <Surface chamfer material="tile" className="overflow-hidden">
                <div className="divide-y divide-[var(--color-border)]">
                  {publishedApps.map((app) => {
                    const st = appStateWords(app.status)
                    return railRow(
                      app.id,
                      !showServer && selectedApp === app.id,
                      `Open ${app.name}`,
                      <AppMonogram name={app.name} />,
                      app.name,
                      <>
                        <StatusDot tone={st.tone} />
                        <span className="min-w-0">
                          {PUBLISH_STATUS.has(app.status) ? st.words : 'Not started'}
                          {app.expose === 'local' ? ' · only on this server' : ''}
                        </span>
                      </>,
                      () => select(app.id),
                    )
                  })}
                </div>
              </Surface>
            )}
          </section>
        </div>
      </aside>
      <div className="flex-1 min-w-0 min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]" data-apps-pane="">
        <div className={`w-full ${APPS_CONTENT_WIDTH} space-y-6 pb-6`}>
          {error ? (
            <SettingsLoadState
              kind="error"
              title={error.what}
              detail={error.detail}
              testId="apps-page-error"
              action={
                <Button variant="default" onClick={() => void refresh()}>
                  Try again
                </Button>
              }
            >
              {error.next}
            </SettingsLoadState>
          ) : null}
          {showServer ? (
            <>
              <AppsServerSection middle={<AppsBoxSection />} />
              <AppsResourcesSection />
            </>
          ) : selectedApp === ADD_APP_PANE ? (
            <AppsCatalogSection />
          ) : selectedApp == null ? null : (
            <>
              {selectedPublished && published ? (
                <PaneHeader
                  title={selectedPublished.name}
                  testId="apps-detail-header"
                  hint={
                    <span className="inline-flex flex-wrap items-center gap-x-1.5">
                      <StatusDot tone={published.tone} />
                      <span>{PUBLISH_STATUS.has(selectedPublished.status) ? published.words : 'Not started'}</span>
                      <span>· kept up by {workspaceLabel}</span>
                      <span>
                        ·{' '}
                        {selectedPublished.expose === 'local'
                          ? 'only on this server'
                          : selectedPublished.url
                            ? selectedPublished.url.replace(/^https?:\/\//, '').replace(/\/$/, '')
                            : 'no public address'}
                      </span>
                    </span>
                  }
                  actions={
                    running ? (
                      <Button
                        aria-label={`Stop ${selectedPublished.name}`}
                        disabled={publishBusy === selectedPublished.id}
                        onClick={() => void publishAction('stop', selectedPublished)}
                      >
                        {publishBusy === selectedPublished.id ? 'Stopping…' : 'Stop'}
                      </Button>
                    ) : (
                      <Button
                        variant="accent"
                        aria-label={`Start ${selectedPublished.name}`}
                        disabled={publishBusy === selectedPublished.id}
                        onClick={() => void publishAction('start', selectedPublished)}
                      >
                        {publishBusy === selectedPublished.id ? 'Starting…' : 'Start'}
                      </Button>
                    )
                  }
                />
              ) : (
                <PaneHeader
                  title="Server sign-in"
                  testId="apps-host-header"
                  hint="The sign-in for the whole server: its own sign-in page and any service that names no App. Anyone who has it can also open every App, so give it out sparingly."
                />
              )}
              {selectedPublished ? <AppAboutCard key={`about-${selectedPublished.id}`} appId={selectedPublished.name} /> : null}
              {selectedPublished?.helperNeedsRestart ? (
                <Callout tone="warn" role="status" className="type-hint">
                  Waiting for a restart: this App&rsquo;s sign-in helper is older than per-App sign-in, so guests still sign in
                  with the server&rsquo;s own sign-in. Stop it and start it again to finish.
                </Callout>
              ) : null}

              <SettingsGroup
                card
                title="Roles"
                settingId="skin-access.roles"
                hint="What a guest may do with each agent. These aren’t the server’s Owner, Admin or Member roles, and they never open a terminal."
              >
                {visibleRoles.length === 0 && !loading ? (
                  <div className="px-4 py-3">
                    <p className={SETTING_ROW_HINT} data-testid="apps-roles-empty">
                      No roles yet. Make one below; it starts with Thread only for each agent you tick.
                    </p>
                  </div>
                ) : null}
                {visibleRoles.map((r) => {
                  const editing = editRoleId === r.id
                  return (
                    <div key={r.id} className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`apps-role-${r.id}`}>
                      <div className="flex items-start justify-between gap-6">
                        <div className="min-w-0 flex-1">
                          <div className="type-name text-[var(--color-text-primary)]">{r.name}</div>
                          {r.roomAccess.length === 0 ? (
                            <p className={SETTING_ROW_HINT}>No agents yet: guests with it see no Thread.</p>
                          ) : (
                            r.roomAccess.map((row) => (
                              <p key={row.handle} className={SETTING_ROW_HINT}>
                                <span className="text-[var(--color-text-primary)]">{handleName(row.handle)}</span>
                                {': '}
                                {row.caps.length ? row.caps.map((c) => shortCap(c)).join(', ') : 'nothing yet'}
                              </p>
                            ))
                          )}
                        </div>
                        {editing ? null : (
                          <div className="flex items-center gap-2 flex-shrink-0">
                            <Button
                              aria-label={`Edit role ${r.name}`}
                              onClick={() => {
                                setEditRoleId(r.id)
                                setEditRoleRooms(new Set(r.roomHandles))
                                const byRoom: Record<string, Set<string>> = {}
                                for (const row of r.roomAccess) {
                                  byRoom[row.handle] = new Set(row.caps)
                                }
                                setEditRoleCapsByRoom(byRoom)
                              }}
                            >
                              Edit
                            </Button>
                            <OverflowMenu
                              label={`More actions for ${r.name}`}
                              items={[
                                {
                                  id: 'remove',
                                  label: 'Remove role…',
                                  danger: true,
                                  onSelect: () =>
                                    setAsk({
                                      title: `Remove the ${r.name} role?`,
                                      message: 'Everyone who has it loses what it allows, at their next sign-in. You can make it again later.',
                                      confirmLabel: 'Remove role',
                                      run: () => removeRole(r.id),
                                    }),
                                },
                              ]}
                            />
                          </div>
                        )}
                      </div>
                      {editing ? (
                        <div className="mt-3 space-y-3">
                          <AgentCapsPicker
                            workspaces={workspaces}
                            rooms={editRoleRooms}
                            capsByRoom={editRoleCapsByRoom}
                            choices={roleChoices}
                            agentConfigOptIn={agentConfigOptIn}
                            labelFor={(ws) => `Edit role ${r.name} agent ${ws}`}
                            capLabelFor={(ws, cap) => `Edit role ${r.name} ${ws} ${cap}`}
                            configLabelFor={(ws) => `Edit role ${r.name} ${ws} agent .k2 folder`}
                            onRooms={setEditRoleRooms}
                            onCaps={setEditRoleCapsByRoom}
                          />
                          <div className="flex gap-2">
                            <Button
                              variant="accent"
                              disabled={roleBusy}
                              onClick={() => void saveRole(r.id, [...editRoleRooms], editRoleCapsByRoom)}
                            >
                              {roleBusy ? 'Saving…' : 'Save role'}
                            </Button>
                            <Button variant="ghost" onClick={() => setEditRoleId(null)}>
                              Cancel
                            </Button>
                          </div>
                        </div>
                      ) : null}
                    </div>
                  )
                })}
                <form
                  className="px-4 py-3 space-y-3"
                  onSubmit={(e) => {
                    e.preventDefault()
                    void createRole()
                  }}
                >
                  <div className="flex items-start justify-between gap-6">
                    <div className="min-w-0 flex-1">
                      <div className={SETTING_ROW_LABEL}>New role</div>
                      <p className={SETTING_ROW_HINT}>Name it, then tick the agents it reaches. Each agent starts with Thread only.</p>
                    </div>
                    <Input
                      className={`${SETTING_FIELD_WIDTH} type-control flex-shrink-0`}
                      placeholder="Role name, like reception"
                      autoCapitalize="none"
                      autoCorrect="off"
                      spellCheck={false}
                      value={newRoleName}
                      onChange={(e) => setNewRoleName(e.target.value)}
                      aria-label="New skin role name"
                    />
                  </div>
                  {workspaces.length > 0 ? (
                    <AgentCapsPicker
                      workspaces={workspaces}
                      rooms={newRoleRooms}
                      capsByRoom={newRoleCapsByRoom}
                      choices={roleChoices}
                      agentConfigOptIn={agentConfigOptIn}
                      labelFor={(ws) => `Role agent ${ws}`}
                      capLabelFor={(ws, cap) => `Role ${ws} ${cap}`}
                      configLabelFor={(ws) => `Role ${ws} agent .k2 folder`}
                      onRooms={setNewRoleRooms}
                      onCaps={setNewRoleCapsByRoom}
                    />
                  ) : (
                    <p className={SETTING_ROW_HINT}>No agents on this server yet, so a new role reaches no Thread.</p>
                  )}
                  {roleError ? (
                    <p role="alert" className="type-hint text-[var(--color-status-error-soft)]">
                      Couldn&rsquo;t save the role. <span className="text-[var(--color-text-muted)]">The server said: {trimReason(roleError)}.</span>
                    </p>
                  ) : null}
                  <div className="flex justify-end">
                    <Button type="submit" variant="accent" disabled={roleBusy || !newRoleName.trim()}>
                      {roleBusy && editRoleId === null ? 'Saving…' : 'Create role'}
                    </Button>
                  </div>
                </form>
              </SettingsGroup>

              <SettingsGroup
                card
                title="Users"
                hint={
                  selectedApp === 'host'
                    ? 'People who can use the server’s own sign-in, and the role each one gets.'
                    : 'Only the people here can sign in to this App. A new App starts with nobody.'
                }
              >
                {appGrants.length === 0 ? (
                  <div className="px-4 py-3">
                    <p className={SETTING_ROW_HINT} data-testid="apps-users-empty">
                      Nobody can sign in yet. Add a person below.
                    </p>
                  </div>
                ) : null}
                {appGrants.map((grant) => {
                  const person = users.find((u) => u.id != null && u.id === grant.subjectId)
                  const rooms = person
                    ? (person.defaultRoomHandles.length ? person.defaultRoomHandles : person.defaultRooms)
                    : []
                  const grantRole = grant.roleId ? roles.find((r) => r.id === grant.roleId) : null
                  const who = `${grantSubjectLabel(grant, users, workspaces)}${grant.subjectKind === 'workspace' ? ' · workspace' : ''}`
                  return (
                    <div key={grant.id} className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`apps-grant-${grant.id}`}>
                      <div className="flex items-start justify-between gap-6">
                        <div className="min-w-0 flex-1">
                          <div className="type-name text-[var(--color-text-primary)]">{who}</div>
                          {selectedApp === 'host' && grant.subjectKind === 'principal' && person ? (
                            person.roleId || person.roleName ? null : (
                              <p className={SETTING_ROW_HINT}>
                                {rooms.length
                                  ? `No role yet, so they reach only ${rooms.map(handleName).join(', ')}’s Thread.`
                                  : 'No role yet, so they reach no agent’s Thread.'}
                              </p>
                            )
                          ) : selectedApp !== 'host' && grantRole ? (
                            <p className={SETTING_ROW_HINT}>{grantRole.name}</p>
                          ) : selectedApp !== 'host' && grant.subjectKind === 'principal' ? (
                            <p className={SETTING_ROW_HINT}>
                              {(() => {
                                const ids = grantScopeRooms(grant.scope)
                                if (!ids.length) return 'No role yet, so they reach no agent’s Thread.'
                                const names = ids.map((id) => workspaces.find((w) => w.id === id)?.name ?? id)
                                return `No role yet, so they reach only ${names.join(', ')}’s Thread.`
                              })()}
                            </p>
                          ) : null}
                          {!grant.enabled ? <p className={SETTING_ROW_HINT}>Off: can&rsquo;t sign in until you turn it back on.</p> : null}
                        </div>
                        <div className="flex items-center gap-2 flex-shrink-0">
                          {selectedApp === 'host' && grant.subjectKind === 'principal' && person ? (
                            <SettingDropdown
                              ariaLabel={`${person.username} login role`}
                              value={person.roleName ?? ''}
                              placeholder="None"
                              options={loginRoleChoices}
                              onChange={(next) => void setUserRole(person.username, next.trim() ? next : null)}
                              menuAlign="right"
                            />
                          ) : null}
                          <Toggle
                            size="sm"
                            checked={grant.enabled}
                            onChange={() => void setGrantEnabled(grant, !grant.enabled)}
                            aria-label={`Enable grant ${grant.id}`}
                            disabled={busyId === grant.id}
                          />
                          <OverflowMenu
                            label={`More actions for ${who}`}
                            triggerProps={{ 'data-testid': `apps-grant-menu-${grant.id}` }}
                            items={[
                              {
                                id: 'revoke',
                                label: 'Remove access…',
                                danger: true,
                                onSelect: () =>
                                  setAsk({
                                    title: `Remove ${grantSubjectLabel(grant, users, workspaces)}?`,
                                    message:
                                      selectedApp === 'host'
                                        ? 'They can’t use the server’s own sign-in any more. You can add them again later.'
                                        : 'They can’t sign in to this App any more. You can add them again later.',
                                    confirmLabel: 'Remove access',
                                    run: () => revokeGrant(grant.id),
                                  }),
                              },
                            ]}
                          />
                        </div>
                      </div>
                    </div>
                  )
                })}
                <SettingRow
                  label="Add a person"
                  hint={
                    selectedApp === 'host'
                      ? 'Pick a person and the role they get here.'
                      : 'Pick a person and the role they get here. This server’s logins are listed too: they get a login on this App, and their server sign-in stays as it is.'
                  }
                  stack
                >
                  <div className={SETTING_FIELD_WIDTH}>
                    <SettingDropdown
                      ariaLabel="App user"
                      value={addPersonId}
                      placeholder="Person"
                      options={peopleChoices}
                      onChange={setAddPersonId}
                      menuAlign="left"
                      fullWidth
                    />
                  </div>
                  <div className={SETTING_FIELD_WIDTH}>
                    <SettingDropdown
                      ariaLabel="App user role"
                      value={addRoleId}
                      placeholder={appRoleChoices.length ? 'Role' : 'Make a role first'}
                      options={appRoleChoices}
                      onChange={setAddRoleId}
                      menuAlign="left"
                      fullWidth
                    />
                  </div>
                  <Button variant="accent" disabled={busyId === 'add-user' || !addPersonId || !addRoleId} onClick={() => void addAppUser()}>
                    {busyId === 'add-user' ? 'Adding…' : 'Add user'}
                  </Button>
                </SettingRow>
                {selectedApp && selectedApp !== 'host' ? (
                  <SettingRow
                    label="Everyone with the server sign-in"
                    hint="Gives each of them the same access to this App that they have on the server."
                  >
                    <Button disabled={busyId === 'copy-host'} onClick={() => void copyHostLogins()}>
                      {busyId === 'copy-host' ? 'Adding…' : 'Add them all'}
                    </Button>
                  </SettingRow>
                ) : null}
              </SettingsGroup>

              {selectedApp ? (
                <AppPeopleAccess
                  key={selectedApp}
                  app={selectedApp}
                  hasRoles={visibleRoles.length > 0}
                  onRolesChanged={() => void refresh()}
                />
              ) : null}

              {selectedApp && selectedApp !== 'host' && phonePairing ? (
                <AppPhonePairing key={`phones-${selectedApp}`} app={selectedApp} />
              ) : null}

              {selectedApp && selectedApp !== 'host' ? <AppReleases key={`releases-${selectedApp}`} app={selectedApp} /> : null}

              {selectedApp === 'host' ? (
                <>
                  <SettingsGroup
                    card
                    title="Keys for other sites"
                    settingId="skin-access.keys"
                    hint="A key lets another site or service, like your website, read or post in an agent’s Thread. Not for people: give a person a role instead. Each key is shown once, when you create it."
                  >
                    {!bannerDismissed && darkKeys.length > 0 ? (
                      <div className="px-4 py-3">
                        <Callout tone="warn" role="status" className="type-hint flex items-center justify-between gap-4">
                          <span>Give these keys an agent: a key with no agents can&rsquo;t reach any Thread.</span>
                          <Button onClick={() => setBannerDismissed(true)}>Dismiss</Button>
                        </Callout>
                      </div>
                    ) : null}
                    {tokens.map((k) => {
                      const busy = busyId === k.id
                      return (
                        <div key={k.id} className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`apps-token-${k.id}`}>
                          <div className="flex items-start justify-between gap-6">
                            <div className="min-w-0 flex-1">
                              <div className="flex items-baseline gap-2 flex-wrap">
                                <span className="type-name text-[var(--color-text-primary)]">{k.name || 'Unnamed key'}</span>
                                <span className="type-hint text-[var(--color-text-secondary)]" title={prefixLabel(k.prefix)}>{keyEnding(k.prefix)}</span>
                              </div>
                              <p className={SETTING_ROW_HINT}>
                                {k.caps.length ? k.caps.map((c) => shortCap(c)).join(', ') : 'No permissions'}
                                {' · '}
                                {k.roomHandles.length ? k.roomHandles.map(handleName).join(', ') : 'no agents'}
                              </p>
                            </div>
                            {editKeyId === k.id ? null : (
                              <div className="flex items-center gap-2 flex-shrink-0">
                                <Button
                                  aria-label={`Edit agents for ${k.name || keyEnding(k.prefix)}`}
                                  onClick={() => {
                                    setEditKeyId(k.id)
                                    setEditKeyRooms(new Set(k.roomHandles))
                                  }}
                                >
                                  Edit agents
                                </Button>
                                <Button
                                  variant="danger-quiet"
                                  disabled={busy}
                                  onClick={() =>
                                    setAsk({
                                      title: `Revoke ${k.name || keyEnding(k.prefix)}?`,
                                      message: 'The site using it stops working at once. A revoked key can’t come back: create a new one instead.',
                                      confirmLabel: 'Revoke key',
                                      run: () => revokeKey(k.id),
                                    })
                                  }
                                >
                                  Revoke
                                </Button>
                              </div>
                            )}
                          </div>
                          {editKeyId === k.id ? (
                            <div className="mt-3 space-y-3">
                              <div className="grid grid-cols-2 gap-x-4 gap-y-1.5">
                                {workspaces.map((ws) => (
                                  <label key={`edit-${k.id}-${ws.id}`} className="flex items-center gap-2 cursor-pointer select-none no-drag min-w-0">
                                    <Checkbox
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
                                    <span className="type-row text-[var(--color-text-primary)] truncate">{ws.name}</span>
                                  </label>
                                ))}
                              </div>
                              <div className="flex gap-2">
                                <Button variant="accent" disabled={busy} onClick={() => void saveKeyRooms(k.id, [...editKeyRooms])}>
                                  Save agents
                                </Button>
                                <Button variant="ghost" onClick={() => setEditKeyId(null)}>
                                  Cancel
                                </Button>
                              </div>
                            </div>
                          ) : null}
                        </div>
                      )
                    })}
                    <div className="px-4 py-3 space-y-3">
                      <div className="flex items-start justify-between gap-6">
                        <div className="min-w-0 flex-1">
                          <div className={SETTING_ROW_LABEL}>New key</div>
                          <p className={SETTING_ROW_HINT}>Name it after the site that uses it, pick what it may do and which agents it reaches.</p>
                        </div>
                        <Input
                          className={`${SETTING_FIELD_WIDTH} type-control flex-shrink-0`}
                          placeholder="Site name, like website"
                          autoCapitalize="none"
                          autoCorrect="off"
                          spellCheck={false}
                          value={mintName}
                          onChange={(e) => setMintName(e.target.value)}
                          aria-label="Key name"
                        />
                      </div>
                      <div className="grid grid-cols-2 gap-x-4 gap-y-1.5">
                        {capChoices.map((cap) => (
                          <label key={cap} title={SKIN_CAP_LABELS[cap]} className="flex items-start gap-2 cursor-pointer select-none no-drag min-w-0">
                            <Checkbox
                              aria-label={`Mint cap ${cap}`}
                              checked={mintCaps.has(cap)}
                              onChange={(e) => {
                                setMintCaps((prev) => toggleSkinCap(prev, cap, e.target.checked))
                              }}
                            />
                            <span className="type-hint text-[var(--color-text-secondary)]">{shortCap(cap)}</span>
                          </label>
                        ))}
                        <AgentConfigOption
                          caps={mintCaps}
                          enabled={agentConfigOptIn}
                          ariaLabel="Mint cap agent .k2 folder"
                          onChange={(on) => {
                            setMintCaps((prev) => toggleSkinCap(prev, SKIN_AGENT_CONFIG_CAP, on))
                          }}
                        />
                      </div>
                      {workspaces.length > 0 ? (
                        <div>
                          <div className="type-hint text-[var(--color-text-secondary)] mb-1.5">Agents it reaches</div>
                          <div className="grid grid-cols-2 gap-x-4 gap-y-1.5">
                            {workspaces.map((ws) => (
                              <label key={`mint-${ws.id}`} className="flex items-center gap-2 cursor-pointer select-none no-drag min-w-0">
                                <Checkbox
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
                                <span className="type-row text-[var(--color-text-primary)] truncate">{ws.name}</span>
                              </label>
                            ))}
                          </div>
                        </div>
                      ) : (
                        <p className={SETTING_ROW_HINT}>No agents on this server yet: add a workspace before creating a key.</p>
                      )}
                      {mintError ? (
                        <p role="alert" className="type-hint text-[var(--color-status-error-soft)]">
                          Couldn&rsquo;t create the key. <span className="text-[var(--color-text-muted)]">{trimReason(mintError)}.</span>
                        </p>
                      ) : null}
                      {mintedSecret ? (
                        <Callout tone="warn" className="type-hint space-y-2" data-testid="apps-token-secret">
                          <p className="font-medium">Copy this key now. K2 can&rsquo;t show it again.</p>
                          <code className="block break-all select-all text-[var(--color-text-primary)]">{mintedSecret}</code>
                          <Button
                            onClick={() => {
                              void navigator.clipboard?.writeText(mintedSecret)
                            }}
                          >
                            Copy key
                          </Button>
                        </Callout>
                      ) : null}
                      <div className="flex justify-end">
                        <Button variant="accent" disabled={mintBusy || !mintName.trim() || mintRooms.size === 0} onClick={() => void mintKey()}>
                          {mintBusy ? 'Creating…' : 'Create key'}
                        </Button>
                      </div>
                    </div>
                  </SettingsGroup>

                  <SettingsGroup
                    card
                    title="Sign-in for other sites"
                    settingId="skin-access.hydra"
                    hint="Let other sites sign people in with this server. Turning on Apps doesn’t turn this on."
                  >
                    {!hydra.supported ? (
                      <div className="px-4 py-3">
                        <LinuxOnlyNote testId="apps-hydra-linux">
                          This runs on Linux servers. On this Mac it can&rsquo;t be turned on.
                        </LinuxOnlyNote>
                      </div>
                    ) : null}
                    <SettingSwitchRow
                      control={
                        <Toggle
                          checked={hydra.enabled}
                          disabled={!hydra.supported || hydraBusy || loading}
                          onChange={(on) => {
                            void persistHydra(on)
                          }}
                          aria-label="Turn on Hydra OIDC issuer"
                        />
                      }
                    >
                      <div className={SETTING_ROW_LABEL}>Sign-in service</div>
                      <p className={SETTING_ROW_HINT}>
                        {hydra.enabled
                          ? hydra.running
                            ? 'On and answering.'
                            : 'On, but not answering yet.'
                          : 'Off. Each person signs in as their own guest account; nothing is copied to the service.'}
                      </p>
                      <p className={SETTING_SERVER_REASON}>Uses OpenID Connect.</p>
                      {hydra.supported && (hydra.publicUrl || hydra.adminUrl) ? (
                        <p className={SETTING_SERVER_REASON}>
                          {hydra.publicUrl ? `Public address ${hydra.publicUrl}` : null}
                          {hydra.publicUrl && hydra.adminUrl ? ' · ' : null}
                          {hydra.adminUrl ? `admin ${hydra.adminUrl}` : null}
                        </p>
                      ) : null}
                    </SettingSwitchRow>
                  </SettingsGroup>
                </>
              ) : null}
            </>
          )}
        </div>
      </div>
      {ask ? (
        <ConfirmSheet
          title={ask.title}
          message={ask.message}
          confirmLabel={ask.confirmLabel}
          destructive
          testId="apps-confirm"
          onCancel={() => setAsk(null)}
          onConfirm={() => {
            const run = ask.run
            setAsk(null)
            void run()
          }}
        />
      ) : null}
    </div>
  )
}

/** Short plain names for the caps whose shared label runs long; the full
 *  label stays as the checkbox's tooltip. */
const SHORT_CAP: Record<string, string> = {
  'files:read': 'Read files',
  'files:write': 'Write files',
  'files:download': 'Save Thread files',
  'tickets:read': 'Read tickets',
  'tickets:post': 'Open and answer tickets',
  'store:read': 'Read workspace data',
  'store:write': 'Change table rows',
  'heartbeats:read': 'See heartbeats',
  'heartbeats:write': 'Change heartbeats',
  'agent:identity': 'Change the agent’s name',
  'agent:handle': 'Change the agent’s address',
}

/** A plain-words name for a cap: the short name, else the shared label cut at its aside. */
function shortCap(cap: string): string {
  if (SHORT_CAP[cap]) return SHORT_CAP[cap]
  const label = ROLE_CAP_LABELS[cap] ?? SKIN_CAP_LABELS[cap as SkinCap]
  if (!label) return cap
  return label.replace(/\s*\(.*\)\s*$/, '')
}

/** "Key ending in ab12": the last four characters a person can match. */
export function keyEnding(prefix: string): string {
  const tail = prefix.replace(/[^a-z0-9]/gi, '').slice(-4)
  return tail ? `Key ending in ${tail}` : 'Key'
}

function trimReason(text: string): string {
  return text.trim().replace(/[.\s]+$/, '')
}

/** The rail key of the Add an App pane. Not an App id: those are lowercase
 *  letters, digits and dashes, so a colon never collides with one. */
const ADD_APP_PANE = ':add-app'

function RailGlyph({ kind }: { kind: 'server' | 'host' | 'add' }): React.JSX.Element {
  return (
    <AvatarFrame>
      {kind === 'add' ? (
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5} className="w-3.5 h-3.5">
          <path d="M8 3.5v9M3.5 8h9" strokeLinecap="round" />
        </svg>
      ) : kind === 'server' ? (
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5} className="w-3.5 h-3.5">
          <rect x="2.5" y="3" width="11" height="4" />
          <rect x="2.5" y="9" width="11" height="4" />
          <path d="M5 5h.01M5 11h.01" strokeLinecap="round" />
        </svg>
      ) : (
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5} className="w-3.5 h-3.5">
          <circle cx="8" cy="6" r="2.5" />
          <path d="M3.5 13c.6-2.3 2.4-3.5 4.5-3.5s3.9 1.2 4.5 3.5" strokeLinecap="round" />
        </svg>
      )}
    </AvatarFrame>
  )
}

/** Tick the agents a role reaches, and what it may do with each. */
function AgentCapsPicker({
  workspaces,
  rooms,
  capsByRoom,
  choices,
  agentConfigOptIn,
  labelFor,
  capLabelFor,
  configLabelFor,
  onRooms,
  onCaps,
}: {
  workspaces: SkinWorkspace[]
  rooms: ReadonlySet<string>
  capsByRoom: Record<string, Set<string>>
  choices: readonly string[]
  agentConfigOptIn: boolean
  labelFor: (handle: string) => string
  capLabelFor: (handle: string, cap: string) => string
  configLabelFor: (handle: string) => string
  onRooms: React.Dispatch<React.SetStateAction<Set<string>>>
  onCaps: React.Dispatch<React.SetStateAction<Record<string, Set<string>>>>
}): React.JSX.Element {
  return (
    <div className="space-y-2">
      <div className="grid grid-cols-2 gap-x-4 gap-y-1.5">
        {workspaces.map((ws) => {
          const included = rooms.has(ws.handle) || rooms.has(ws.id)
          return (
            <label key={`agent-${ws.id}`} className="flex items-center gap-2 cursor-pointer select-none no-drag min-w-0">
              <Checkbox
                aria-label={labelFor(ws.handle)}
                checked={included}
                onChange={(e) => {
                  onRooms((prev) => {
                    const next = new Set(prev)
                    if (e.target.checked) next.add(ws.handle)
                    else {
                      next.delete(ws.handle)
                      next.delete(ws.id)
                    }
                    return next
                  })
                  onCaps((prev) => {
                    const next = { ...prev }
                    if (e.target.checked) next[ws.handle] = new Set(DEFAULT_SKIN_CAPS)
                    else delete next[ws.handle]
                    return next
                  })
                }}
              />
              <span className="type-row text-[var(--color-text-primary)] truncate">{ws.name}</span>
            </label>
          )
        })}
      </div>
      {workspaces
        .filter((ws) => rooms.has(ws.handle) || rooms.has(ws.id))
        .map((ws) => {
          const caps = capsByRoom[ws.handle] ?? new Set(DEFAULT_SKIN_CAPS)
          return (
            <Surface key={`caps-${ws.id}`} chamfer material="tile" className="px-3 py-2 space-y-1.5">
              <div className="type-row text-[var(--color-text-primary)]">What it may do with {ws.name}</div>
              <div className="grid grid-cols-2 gap-x-4 gap-y-1.5">
                {choices.map((cap) => (
                  <label key={`${ws.handle}-${cap}`} title={ROLE_CAP_LABELS[cap]} className="flex items-start gap-2 cursor-pointer select-none no-drag min-w-0">
                    <Checkbox
                      aria-label={capLabelFor(ws.handle, cap)}
                      checked={caps.has(cap)}
                      onChange={(e) => {
                        onCaps((prev) => ({
                          ...prev,
                          [ws.handle]: toggleSkinCap(prev[ws.handle] ?? new Set(DEFAULT_SKIN_CAPS), cap, e.target.checked),
                        }))
                      }}
                    />
                    <span className="type-hint text-[var(--color-text-secondary)]">{shortCap(cap)}</span>
                  </label>
                ))}
                <AgentConfigOption
                  caps={caps}
                  enabled={agentConfigOptIn}
                  ariaLabel={configLabelFor(ws.handle)}
                  onChange={(on) => {
                    onCaps((prev) => ({
                      ...prev,
                      [ws.handle]: toggleSkinCap(prev[ws.handle] ?? new Set(DEFAULT_SKIN_CAPS), SKIN_AGENT_CONFIG_CAP, on),
                    }))
                  }}
                />
              </div>
            </Surface>
          )
        })}
    </div>
  )
}
