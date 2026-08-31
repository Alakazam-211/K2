// Settings → Sidecars → Skin Access (prd-skin-system-v1 U3, prd-skin-auth-v1 S8/S11/S12).
// Owner surface: front door (POST apply + live Caddy/nested status), skin-user
// roster (NOT Server Access), k2skn_ keys (secret once at mint). Hydra is
// opt-in (Linux sidecar; Mac supported=false). Enable skins ≠ start Hydra.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { Toggle } from '@/components/ui'
import { SettingsGroup } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'

export const SKIN_ACCESS_MANIFEST: SettingEntry[] = [
  {
    id: 'skin-access.front-door',
    section: 'skin-access',
    label: 'Front door',
    description: 'Use K2 Connect (skin.<sub>.k2.dev) or Direct/this box (Caddy → daemon loopback)',
    keywords: [
      'front door',
      'connect',
      'caddy',
      'direct',
      'localhost',
      'skin subdomain',
      'tunnel',
      'ui port',
      'listen',
    ],
    group: 'Front door',
  },
  {
    id: 'skin-access.users',
    section: 'skin-access',
    label: 'Skin users',
    description: 'Guest list for skins — separate from Server Access / Connect operators',
    keywords: ['skin users', 'roster', 'guest', 'principal', 'add', 'remove', 'search'],
    group: 'Skin users',
  },
  {
    id: 'skin-access.keys',
    section: 'skin-access',
    label: 'Keys',
    description: 'Mint k2skn_ capability passes with scopes / rooms; secret shown once',
    keywords: ['skin token', 'k2skn', 'scopes', 'caps', 'thread', 'overlay', 'revoke', 'mint'],
    group: 'Keys',
  },
  {
    id: 'skin-access.hydra',
    section: 'skin-access',
    label: 'OIDC issuer (Hydra)',
    description: 'Opt-in Hydra sidecar — Linux loopback 4444/4445; enabling skins does not start Hydra',
    keywords: ['oidc', 'hydra', 'issuer', 'openid'],
    group: 'OIDC issuer (Hydra)',
  },
]

export const DEFAULT_SKIN_CAPS = ['thread:read', 'thread:post'] as const

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
  username: string
  createdAt?: string | null
  defaultRooms: string[]
  defaultRoomHandles: string[]
}

export type SkinTokenRow = {
  id: string
  prefix: string
  username: string
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
const CADDY_INSTALL_HINT = 'brew install caddy / distro package'

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
    const username = asString(rec.username) ?? asString(rec.id) ?? asString(rec.principal)
    if (!username) return []
    return [{
      username,
      createdAt: asString(rec.createdAt) ?? asString(rec.created_at),
      defaultRooms: parseStringList(rec.defaultRooms ?? rec.default_rooms),
      defaultRoomHandles: parseStringList(rec.defaultRoomHandles ?? rec.default_room_handles),
    }]
  })
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
    const username =
      asString(rec.username) ?? asString(rec.user) ?? asString(rec.principal) ?? ''
    const caps = parseCaps(rec.caps ?? rec.scopes ?? rec.capabilities)
    const prefix =
      asString(rec.prefix) ??
      (id.startsWith('k2skn_') ? id : `k2skn_…${id.slice(-4)}`)
    return [{
      id,
      prefix,
      username,
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

function FrontDoorStatus({ door }: { door: SkinFrontDoor }): React.JSX.Element {
  const caddy = door.caddy
  const nested = door.mode === 'connect' ? door.nested : undefined
  return (
    <div className="space-y-1.5">
      <div>
        <div className="text-[10px] text-[var(--color-text-muted)]">Connect URL</div>
        <code className="block text-[10px] font-mono text-[var(--color-text-secondary)] mt-0.5">
          {door.connectUrl}
        </code>
      </div>
      <div>
        <div className="text-[10px] text-[var(--color-text-muted)]">Listen</div>
        <span className="block text-[10px] font-mono text-[var(--color-text-secondary)] mt-0.5">
          {door.listen}
        </span>
      </div>
      {caddy?.missing ? (
        <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
          Caddy is not installed — {CADDY_INSTALL_HINT}.
        </p>
      ) : caddy ? (
        <div>
          <div className="text-[10px] text-[var(--color-text-muted)]">Caddy</div>
          <span className="block text-[10px] text-[var(--color-text-secondary)] mt-0.5">
            {caddy.running
              ? `running${caddy.pid != null ? ` (pid ${caddy.pid})` : ''}`
              : 'not running'}
          </span>
        </div>
      ) : null}
      {nested ? (
        <div>
          <div className="text-[10px] text-[var(--color-text-muted)]">Nested</div>
          <span className="block text-[10px] text-[var(--color-text-secondary)] mt-0.5">
            {nested.registered ? 'registered' : 'not registered'}
            {nested.host ? (
              <>
                {' · '}
                <code className="font-mono">{nested.host}</code>
              </>
            ) : null}
          </span>
        </div>
      ) : null}
    </div>
  )
}

export function SkinAccessSection(): React.JSX.Element {
  const [frontDoor, setFrontDoor] = useState<SkinFrontDoor>(DEFAULT_FRONT_DOOR)
  const [uiPortText, setUiPortText] = useState('')
  const [users, setUsers] = useState<SkinUser[]>([])
  const [tokens, setTokens] = useState<SkinTokenRow[]>([])
  const [workspaces, setWorkspaces] = useState<SkinWorkspace[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [userQuery, setUserQuery] = useState('')
  const [newUsername, setNewUsername] = useState('')
  const [addBusy, setAddBusy] = useState(false)
  const [addError, setAddError] = useState<string | null>(null)
  const [removeConfirm, setRemoveConfirm] = useState<string | null>(null)
  const [mintUsername, setMintUsername] = useState('')
  const [mintCaps, setMintCaps] = useState<Set<string>>(() => new Set(DEFAULT_SKIN_CAPS))
  const [mintRooms, setMintRooms] = useState<Set<string>>(() => new Set())
  const [mintBusy, setMintBusy] = useState(false)
  const [mintError, setMintError] = useState<string | null>(null)
  const [mintedSecret, setMintedSecret] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [doorBusy, setDoorBusy] = useState(false)
  const [hydra, setHydra] = useState<SkinHydra>(DEFAULT_HYDRA)
  const [hydraBusy, setHydraBusy] = useState(false)
  const [applyToAll, setApplyToAll] = useState<Record<string, boolean>>({})
  const [bannerDismissed, setBannerDismissed] = useState(false)
  const [editKeyId, setEditKeyId] = useState<string | null>(null)
  const [editKeyRooms, setEditKeyRooms] = useState<Set<string>>(() => new Set())

  const refresh = useCallback(async () => {
    setError(null)
    const failures: string[] = []
    try {
      const door = await daemonCliGet<unknown>('skin/front-door')
      const parsed = parseFrontDoor(door)
      setFrontDoor(parsed)
      if (parsed.error) failures.push(parsed.error)
    } catch (e) {
      failures.push(`front-door: ${errText(e)}`)
    }
    try {
      const roster = await daemonCliGet<unknown>('skin/users')
      setUsers(parseSkinUsers(roster))
    } catch (e) {
      failures.push(`users: ${errText(e)}`)
      setUsers([])
    }
    try {
      const keys = await daemonCliGet<unknown>('skin-tokens')
      setTokens(parseSkinTokens(keys))
    } catch (e) {
      failures.push(`keys: ${errText(e)}`)
      setTokens([])
    }
    try {
      const projects = await daemonCliGet<unknown>('projects/list')
      setWorkspaces(parseWorkspaces(projects))
    } catch (e) {
      failures.push(`workspaces: ${errText(e)}`)
      setWorkspaces([])
    }
    try {
      const h = await daemonCliGet<unknown>('skin/hydra')
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

  useEffect(() => {
    setUiPortText(frontDoor.uiPort == null ? '' : String(frontDoor.uiPort))
  }, [frontDoor.uiPort])

  useEffect(() => {
    const u = users.find((x) => x.username === mintUsername.trim().toLowerCase())
    if (!u) return
    const fromUser = u.defaultRoomHandles.length ? u.defaultRoomHandles : u.defaultRooms
    setMintRooms(new Set(fromUser))
  }, [mintUsername, users])

  const visibleUsers = useMemo(() => {
    const q = userQuery.trim().toLowerCase()
    if (!q) return users
    return users.filter((u) => u.username.toLowerCase().includes(q))
  }, [users, userQuery])

  const persistDoor = useCallback(
    async (body: { mode: FrontDoorMode; uiPort?: number | null; apply: true }) => {
      setDoorBusy(true)
      setError(null)
      try {
        const posted = await daemonCliPost<unknown>('skin/front-door', body)
        const postErr = parseFrontDoor(posted).error
        try {
          const door = await daemonCliGet<unknown>('skin/front-door')
          const parsed = parseFrontDoor(door)
          setFrontDoor(parsed)
          setError(parsed.error ?? postErr)
        } catch {
          if (postErr) setError(postErr)
        }
      } catch (e) {
        setError(`front-door: ${errText(e)}`)
        throw e
      } finally {
        setDoorBusy(false)
      }
    },
    [],
  )

  const setMode = useCallback(
    async (mode: FrontDoorMode) => {
      if (mode === frontDoor.mode) return
      const prev = frontDoor.mode
      setFrontDoor((d) => ({ ...d, mode }))
      try {
        await persistDoor({ mode, apply: true })
      } catch {
        setFrontDoor((d) => ({ ...d, mode: prev }))
      }
    },
    [frontDoor.mode, persistDoor],
  )

  const commitUiPort = useCallback(async () => {
    const trimmed = uiPortText.trim()
    let next: number | null = null
    if (trimmed) {
      const n = Number(trimmed)
      if (!Number.isInteger(n) || n < 1 || n > 65535) {
        setError('front-door: UI port must be 1–65535')
        return
      }
      next = n
    }
    if (next === (frontDoor.uiPort ?? null)) return
    try {
      await persistDoor({ mode: frontDoor.mode, uiPort: next, apply: true })
    } catch {
      /* persistDoor already set the alert */
    }
  }, [uiPortText, frontDoor.mode, frontDoor.uiPort, persistDoor])

  const addUser = useCallback(async () => {
    const username = newUsername.trim().toLowerCase()
    if (!username) return
    setAddBusy(true)
    setAddError(null)
    try {
      await daemonCliPost('skin/users', { username })
      setNewUsername('')
      await refresh()
    } catch (e) {
      setAddError(errText(e))
    } finally {
      setAddBusy(false)
    }
  }, [newUsername, refresh])

  const removeUser = useCallback(
    async (username: string) => {
      setAddError(null)
      try {
        await daemonCliPost('skin/users/remove', { username })
        setRemoveConfirm(null)
        await refresh()
      } catch (e) {
        setAddError(errText(e))
      }
    },
    [refresh],
  )

  const mintKey = useCallback(async () => {
    const username = mintUsername.trim().toLowerCase()
    if (!username) {
      setMintError('Username is required')
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
      const res = await daemonCliPost<unknown>('skin-tokens', { username, caps, rooms })
      const secret = mintSecretFrom(res)
      if (!secret) throw new Error('mint returned no secret')
      setMintedSecret(secret)
      await refresh()
    } catch (e) {
      setMintError(errText(e))
    } finally {
      setMintBusy(false)
    }
  }, [mintUsername, mintCaps, mintRooms, refresh])

  const saveUserRooms = useCallback(
    async (username: string, handles: string[], applyTokens: boolean) => {
      setAddError(null)
      try {
        await daemonCliPost('skin/users/rooms', {
          username,
          rooms: handles,
          applyTokens,
        })
        await refresh()
      } catch (e) {
        setAddError(errText(e))
      }
    },
    [refresh],
  )

  const saveKeyRooms = useCallback(
    async (id: string, handles: string[]) => {
      setMintError(null)
      setBusyId(id)
      try {
        await daemonCliPost('skin-tokens/rooms', { id, rooms: handles })
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
        const posted = await daemonCliPost<unknown>('skin/hydra', { enabled, apply: true })
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
        await daemonCliPost('skin-tokens/revoke', { id })
        await refresh()
      } catch (e) {
        setMintError(errText(e))
      } finally {
        setBusyId(null)
      }
    },
    [refresh],
  )

  return (
    <div className="h-full min-h-0 overflow-y-auto p-6">
      <div className="max-w-2xl space-y-8">
        <div>
          <h2 className="text-base font-medium text-[var(--color-text-primary)]">Skin Access</h2>
          <p className="text-[11px] text-[var(--color-text-muted)] mt-1 max-w-2xl">
            Skin users and capability passes — not Server Access (Connect operators). Overlay
            Thread rooms only; grid / PTY never. Hydra stays off until you ask.
          </p>
        </div>

        {error && (
          <p
            role="alert"
            className="text-[11px] text-[var(--color-status-error-soft)] max-w-2xl"
          >
            {error}
          </p>
        )}

        {!bannerDismissed && darkKeys.length > 0 ? (
          <div
            role="status"
            className="border border-[var(--color-status-warning-soft)]/40 bg-[var(--color-status-warning-soft)]/10 p-3 space-y-2"
          >
            <p className="text-[11px] text-[var(--color-text-primary)]">
              Assign agents or these guests go dark. Live keys with no rooms cannot Thread.
            </p>
            <button
              type="button"
              className="text-[10px] text-[var(--color-accent)] cursor-pointer"
              onClick={() => setBannerDismissed(true)}
            >
              Dismiss
            </button>
          </div>
        ) : null}

        <SettingsGroup title="Front door">
          <div data-settings-id="skin-access.front-door" className="space-y-3">
            <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
              How packets arrive. Same pass / rooms either way.
            </p>
            <label className="flex items-start gap-2 cursor-pointer select-none no-drag">
              <input
                type="radio"
                name="skin-front-door"
                value="connect"
                checked={frontDoor.mode === 'connect'}
                disabled={doorBusy}
                onChange={() => void setMode('connect')}
                className="mt-0.5"
              />
              <span className="text-[11px] text-[var(--color-text-secondary)]">Use K2 Connect</span>
            </label>
            <label className="flex items-start gap-2 cursor-pointer select-none no-drag">
              <input
                type="radio"
                name="skin-front-door"
                value="direct"
                checked={frontDoor.mode === 'direct'}
                disabled={doorBusy}
                onChange={() => void setMode('direct')}
                className="mt-0.5"
              />
              <span>
                <span className="text-[11px] text-[var(--color-text-secondary)]">
                  Direct / this box
                </span>
                <span className="block text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
                  Caddy on this box → daemon loopback. DNS A/AAAA has no port (use :443 in
                  production). Do not bind k2-daemon to the LAN. Pretty names on the tunnel are
                  CNAME to <code className="text-[10px]">{'skin.<sub>.k2.dev'}</code>, not this
                  radio.
                </span>
              </span>
            </label>
            <FrontDoorStatus door={frontDoor} />
            <label className="flex items-center gap-2">
              <span className="text-[10px] text-[var(--color-text-muted)]">
                UI port (optional, same-origin /)
              </span>
              <input
                type="number"
                min={1}
                max={65535}
                step={1}
                inputMode="numeric"
                placeholder=""
                aria-label="UI port (optional, same-origin /)"
                disabled={doorBusy}
                value={uiPortText}
                onChange={(e) => setUiPortText(e.target.value)}
                onBlur={() => void commitUiPort()}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault()
                    ;(e.target as HTMLInputElement).blur()
                  }
                }}
                className="w-20 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
              />
            </label>
          </div>
        </SettingsGroup>

        <SettingsGroup title="Skin users">
          <div data-settings-id="skin-access.users" className="space-y-3">
            <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
              Guest list for skins. Not the Server Access / Connect operator roster. Login lives
              in the skin; K2 stores the principal id.
            </p>
            <form
              className="flex flex-wrap gap-1.5 items-center"
              onSubmit={(e) => {
                e.preventDefault()
                void addUser()
              }}
            >
              <input
                className={`${INPUT_CLS} flex-1 min-w-[8rem]`}
                placeholder="username"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                value={newUsername}
                onChange={(e) => setNewUsername(e.target.value)}
                aria-label="New skin username"
              />
              <button
                type="submit"
                disabled={addBusy || !newUsername.trim()}
                className="flex-shrink-0 px-3 py-1 text-[11px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
              >
                {addBusy ? 'Adding…' : 'Add user'}
              </button>
            </form>
            {addError && (
              <div
                role="alert"
                className="text-[10px] text-[var(--color-status-error-soft)] px-2 py-1 border border-[color-mix(in_srgb,var(--color-status-error-soft)_20%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error-soft)_5%,transparent)]"
              >
                {addError}
              </div>
            )}
            <input
              type="search"
              value={userQuery}
              onChange={(e) => setUserQuery(e.target.value)}
              placeholder="Search users"
              aria-label="Search skin users"
              className={INPUT_CLS}
            />
            {loading ? (
              <p className="text-[10px] text-[var(--color-text-muted)] py-1">Loading…</p>
            ) : users.length === 0 ? (
              <div className="text-[10px] text-[var(--color-text-muted)] py-1">
                No skin users yet — add one above.
              </div>
            ) : visibleUsers.length === 0 ? (
              <div className="text-[10px] text-[var(--color-text-muted)] py-1">
                No users match.
              </div>
            ) : (
              <div className="divide-y divide-[var(--color-border)]">
                {visibleUsers.map((u) => {
                  const selected = new Set(
                    u.defaultRoomHandles.length ? u.defaultRoomHandles : u.defaultRooms,
                  )
                  return (
                  <div key={u.username} className="py-2 space-y-2">
                    <div className="flex items-center justify-between gap-3">
                    <span className="text-xs text-[var(--color-text-primary)] font-mono truncate">
                      {u.username}
                    </span>
                    {removeConfirm === u.username ? (
                      <span className="flex items-center gap-1.5 flex-shrink-0">
                        <button
                          type="button"
                          onClick={() => void removeUser(u.username)}
                          className="text-[10px] text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
                        >
                          Confirm remove
                        </button>
                        <button
                          type="button"
                          onClick={() => setRemoveConfirm(null)}
                          className="text-[10px] text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer"
                        >
                          Cancel
                        </button>
                      </span>
                    ) : (
                      <button
                        type="button"
                        onClick={() => setRemoveConfirm(u.username)}
                        className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
                      >
                        Remove
                      </button>
                    )}
                    </div>
                    {workspaces.length > 0 ? (
                      <div className="flex flex-wrap gap-x-3 gap-y-1">
                        {workspaces.map((ws) => (
                          <label
                            key={`${u.username}-${ws.id}`}
                            className="flex items-center gap-1.5 cursor-pointer select-none no-drag"
                          >
                            <input
                              type="checkbox"
                              aria-label={`${u.username} agent ${ws.handle}`}
                              checked={selected.has(ws.handle) || selected.has(ws.id)}
                              onChange={(e) => {
                                const next = new Set(selected)
                                if (e.target.checked) next.add(ws.handle)
                                else {
                                  next.delete(ws.handle)
                                  next.delete(ws.id)
                                }
                                void saveUserRooms(
                                  u.username,
                                  [...next].filter((h) => workspaces.some((w) => w.handle === h || w.id === h)),
                                  applyToAll[u.username] === true,
                                )
                              }}
                            />
                            <span className="text-[10px] font-mono text-[var(--color-text-secondary)]">
                              {ws.handle}
                            </span>
                            {ws.name && ws.name !== ws.handle ? (
                              <span className="text-[10px] text-[var(--color-text-muted)]">
                                {ws.name}
                              </span>
                            ) : null}
                          </label>
                        ))}
                      </div>
                    ) : null}
                    <label className="flex items-center gap-1.5 cursor-pointer select-none no-drag">
                      <input
                        type="checkbox"
                        checked={applyToAll[u.username] === true}
                        onChange={(e) =>
                          setApplyToAll((prev) => ({ ...prev, [u.username]: e.target.checked }))
                        }
                      />
                      <span className="text-[10px] text-[var(--color-text-muted)]">
                        Apply to all keys
                      </span>
                    </label>
                  </div>
                  )
                })}
              </div>
            )}
          </div>
        </SettingsGroup>

        <SettingsGroup title="Keys">
          <div data-settings-id="skin-access.keys" className="space-y-3">
            <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
              Each live key shows scopes / rooms. The raw secret is shown only once when minted.
              Prefix <code className="text-[10px]">k2skn_</code> — not{' '}
              <code className="text-[10px]">k2sk_</code> API keys.{' '}
              <code className="text-[10px]">thread:read</code> includes overlay WS.
            </p>
            <div className="space-y-2">
              <input
                className={INPUT_CLS}
                placeholder="username"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                value={mintUsername}
                onChange={(e) => setMintUsername(e.target.value)}
                aria-label="Mint key username"
              />
              <div className="flex flex-wrap gap-x-4 gap-y-1">
                {DEFAULT_SKIN_CAPS.map((cap) => (
                  <label
                    key={cap}
                    className="flex items-center gap-1.5 cursor-pointer select-none no-drag"
                  >
                    <input
                      type="checkbox"
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
                      <input
                        type="checkbox"
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
                disabled={mintBusy || !mintUsername.trim() || mintRooms.size === 0}
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
                No skin keys yet. Mint one for a skin user.
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
                          {k.username && (
                            <span className="text-[11px] text-[var(--color-text-secondary)]">
                              {k.username}
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
                                  <input
                                    type="checkbox"
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
      </div>
    </div>
  )
}
