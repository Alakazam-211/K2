// Settings → Sidecars → Skin Access (prd-skin-system-v1 U3, prd-skin-auth-v1 S8/S11/S12).
// Owner surface: front-door (Caddy path-filter), skin-user roster (NOT Server Access),
// k2skn_ keys with scopes (secret once at mint), Hydra toggle visible off.
// Applying a mode writes ~/.k2/skin/Caddyfile and supervises Caddy. Hydra stays off.

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
    keywords: ['front door', 'connect', 'caddy', 'direct', 'localhost', 'skin subdomain', 'tunnel'],
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
    description: 'Opt-in Hydra sidecar — off; enabling skins does not start Hydra',
    keywords: ['oidc', 'hydra', 'issuer', 'openid'],
    group: 'OIDC issuer (Hydra)',
  },
]

export const DEFAULT_SKIN_CAPS = ['thread:read', 'thread:post'] as const

export type FrontDoorMode = 'connect' | 'direct'

export type SkinFrontDoor = {
  mode: FrontDoorMode
  connectUrl: string
  directListen: string
  subdomain: string | null
}

export type SkinUser = {
  username: string
  createdAt?: string | null
}

export type SkinTokenRow = {
  id: string
  prefix: string
  username: string
  caps: string[]
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
  const connectUrl =
    asString(rec.connectUrl) ??
    asString(rec.connect_url) ??
    (subdomain ? `https://skin.${subdomain}.k2.dev` : CONNECT_URL_STUB)
  const directListen =
    asString(rec.directListen) ??
    asString(rec.direct_listen) ??
    asString(rec.listen) ??
    DIRECT_LISTEN_STUB
  return { mode: parseMode(raw), connectUrl, directListen, subdomain }
}

export function parseSkinUsers(raw: unknown): SkinUser[] {
  return asList(raw, ['users', 'roster']).flatMap((row) => {
    const rec = asRecord(row)
    const username = asString(rec.username) ?? asString(rec.id) ?? asString(rec.principal)
    if (!username) return []
    return [{ username, createdAt: asString(rec.createdAt) ?? asString(rec.created_at) }]
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
    return [{ id, prefix, username, caps }]
  })
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
  const [frontDoor, setFrontDoor] = useState<SkinFrontDoor>({
    mode: 'connect',
    connectUrl: CONNECT_URL_STUB,
    directListen: DIRECT_LISTEN_STUB,
    subdomain: null,
  })
  const [users, setUsers] = useState<SkinUser[]>([])
  const [tokens, setTokens] = useState<SkinTokenRow[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [userQuery, setUserQuery] = useState('')
  const [newUsername, setNewUsername] = useState('')
  const [addBusy, setAddBusy] = useState(false)
  const [addError, setAddError] = useState<string | null>(null)
  const [removeConfirm, setRemoveConfirm] = useState<string | null>(null)
  const [mintUsername, setMintUsername] = useState('')
  const [mintCaps, setMintCaps] = useState<Set<string>>(() => new Set(DEFAULT_SKIN_CAPS))
  const [mintBusy, setMintBusy] = useState(false)
  const [mintError, setMintError] = useState<string | null>(null)
  const [mintedSecret, setMintedSecret] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [doorBusy, setDoorBusy] = useState(false)

  const refresh = useCallback(async () => {
    setError(null)
    const failures: string[] = []
    try {
      const door = await daemonCliGet<unknown>('skin/front-door')
      setFrontDoor(parseFrontDoor(door))
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
    if (failures.length) setError(failures.join(' · '))
    setLoading(false)
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const visibleUsers = useMemo(() => {
    const q = userQuery.trim().toLowerCase()
    if (!q) return users
    return users.filter((u) => u.username.toLowerCase().includes(q))
  }, [users, userQuery])

  const setMode = useCallback(
    async (mode: FrontDoorMode) => {
      const prev = frontDoor.mode
      setFrontDoor((d) => ({ ...d, mode }))
      setDoorBusy(true)
      setError(null)
      try {
        await daemonCliPost('skin/front-door', { mode })
        try {
          const door = await daemonCliGet<unknown>('skin/front-door')
          setFrontDoor(parseFrontDoor(door))
        } catch {
          /* POST succeeded; keep the optimistic mode if re-read fails */
        }
      } catch (e) {
        setFrontDoor((d) => ({ ...d, mode: prev }))
        setError(`front-door: ${errText(e)}`)
      } finally {
        setDoorBusy(false)
      }
    },
    [frontDoor.mode],
  )

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
    setMintBusy(true)
    setMintError(null)
    try {
      const res = await daemonCliPost<unknown>('skin-tokens', { username, caps })
      const secret = mintSecretFrom(res)
      if (!secret) throw new Error('mint returned no secret')
      setMintedSecret(secret)
      await refresh()
    } catch (e) {
      setMintError(errText(e))
    } finally {
      setMintBusy(false)
    }
  }, [mintUsername, mintCaps, refresh])

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

        <SettingsGroup title="Front door">
          <div data-settings-id="skin-access.front-door" className="space-y-3">
            <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
              How packets arrive. Same pass / rooms either way. Saves and applies the Caddy
              path-filter (Thread only — never grid / PTY).
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
              <span>
                <span className="text-[11px] text-[var(--color-text-secondary)]">Use K2 Connect</span>
                <code className="block text-[10px] font-mono text-[var(--color-text-muted)] mt-0.5">
                  {frontDoor.connectUrl}
                </code>
              </span>
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
                <span className="block text-[10px] font-mono text-[var(--color-text-muted)] mt-0.5">
                  {frontDoor.directListen}
                </span>
                <span className="block text-[10px] text-[var(--color-text-muted)] mt-0.5">
                  Optional domain A/AAAA or CNAME (no port in DNS). Localhost skin: Caddy also
                  serves the UI (same origin).
                </span>
              </span>
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
                {visibleUsers.map((u) => (
                  <div key={u.username} className="py-2 flex items-center justify-between gap-3">
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
                ))}
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
              <button
                type="button"
                disabled={mintBusy || !mintUsername.trim()}
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
            <div className="flex items-center justify-between py-2">
              <div className="flex-1 min-w-0 mr-3">
                <span className="text-xs text-[var(--color-text-secondary)]">
                  Turn on — this box issues standard OIDC tickets
                </span>
                <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
                  Off. Enabling skins does not start Hydra. Subject = skin principal id; no users
                  in Hydra.
                </p>
              </div>
              <Toggle
                checked={false}
                disabled
                onChange={() => {
                  /* Hydra sidecar is a later slice — toggle stays off. */
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
