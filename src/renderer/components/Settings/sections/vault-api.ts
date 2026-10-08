// Settings → Vault client (prd-k2-vault-v1 §15, §16.1, §22.3). Thin over
// `/cli/vault/*` on the window's server. Values only ever travel in POST
// bodies (`valueB64`); nothing here keeps one.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export interface VaultCounts {
  user: number
  system: number
  disabled: number
  grants: number
  pendingRequests: number
  rotateDue: number
}

export interface VaultStatus {
  open: boolean
  error?: { code: string; hint: string }
  keyHome?: string
  keyHomeLabel?: string
  rollbackGuard?: string
  rollback?: boolean
  tamper?: string
  /** `confirmed` | `unconfirmed` */
  recovery?: string
  recoveryConfirmedAt?: string | null
  /** A vault confirm password is set. */
  confirmPassword?: boolean
  counts?: VaultCounts
  /** The per-box honest sentence (§18). */
  honest?: string
  agentsCanRead?: string
  adopted?: { store: string; moved: number; kept: number; errors: unknown }[]
  legacyCopies?: string
  /** Present while recovery isn't confirmed: the person-only instruction. */
  recoveryHint?: string
}

export interface VaultHostDisplay {
  origin: string
  unicode: string
  mixedScript: boolean
}

export interface VaultSecret {
  id: string
  /** `user` | `system` */
  namespace: string
  scope: { kind: string; id?: string }
  name: string
  kind: string
  provider?: string
  hosts: string[]
  defaultDelivery: string
  deliveryCeiling: string
  origin: string
  createdBy: string
  humanGiven: boolean
  latestVersion: number
  /** `active` | `disabled` | `destroyed` */
  status: string
  rotateEveryDays?: number | null
  description?: string
  expiresAt?: string | null
  createdAt?: string
  updatedAt?: string
  ref?: string | null
  agentCanSee: boolean
  /** The workspace's handle, for workspace scope. */
  workspace?: string
  hostsDisplay?: VaultHostDisplay[]
}

export interface VaultAuditRow {
  seq: number
  ts: string
  secretId?: string | null
  version?: number | null
  action: string
  actor: string
  consumer?: string | null
  targetHost?: string | null
  outcome: string
  count: number
}

export interface VaultGrant {
  id: string
  consumerLabel?: string
  delivery?: string
  [k: string]: unknown
}

export interface VaultRequest {
  cardId: string
  workspaceId: string
  requestedBy: string
  name: string
  kind: string
  proposedScope: string
  proposedHosts: string[]
  proposedDelivery: string
  prompt: string
  /** `pending` | `set` | `void` | `expired` */
  status: string
  answeredBy?: string | null
  createdAt: string
  answeredAt?: string | null
}

export interface VaultPreset {
  id: string
  label: string
  hosts: string[]
  dst: string
  fmt: string
}

/** UTF-8 → base64, for `valueB64`. */
export function toBase64(text: string): string {
  const bytes = new TextEncoder().encode(text)
  let bin = ''
  for (const b of bytes) bin += String.fromCharCode(b)
  return btoa(bin)
}

/** base64 → UTF-8 text (a revealed value). */
export function fromBase64(b64: string): string {
  const bin = atob(b64)
  const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0))
  return new TextDecoder().decode(bytes)
}

const scope = () => primaryScope()

export const vaultApi = {
  status: () => daemonCliGet<VaultStatus>(scope(), 'vault/status'),
  list: async (opts: { system?: boolean } = {}): Promise<VaultSecret[]> => {
    const r = await daemonCliGet<{ secrets?: VaultSecret[] }>(scope(), 'vault/list', opts.system ? { system: 1 } : undefined)
    return Array.isArray(r?.secrets) ? r.secrets : []
  },
  show: (id: string) =>
    daemonCliGet<{ secret: VaultSecret; grants: VaultGrant[]; audit: VaultAuditRow[] }>(scope(), 'vault/show', { id }),
  requests: async (): Promise<VaultRequest[]> => {
    const r = await daemonCliGet<{ requests?: VaultRequest[] }>(scope(), 'vault/requests')
    return Array.isArray(r?.requests) ? r.requests : []
  },
  presets: async (): Promise<VaultPreset[]> => {
    const r = await daemonCliGet<{ presets?: VaultPreset[] }>(scope(), 'vault/presets')
    return Array.isArray(r?.presets) ? r.presets : []
  },
  set: (body: {
    name: string
    scope: 'workspace' | 'server'
    workspace?: string
    valueB64: string
    preset?: string
    hosts?: string[]
    delivery?: string
    kind?: string
    mode?: 'create'
  }) => daemonCliPost<{ ok: boolean; ref?: string; trimmed?: boolean }>(scope(), 'vault/set', body),
  policy: (body: {
    id: string
    addHosts?: string[]
    removeHosts?: string[]
    delivery?: string
    ceiling?: string
  }) => daemonCliPost<{ ok: boolean; secret: VaultSecret }>(scope(), 'vault/policy', body),
  grant: (body: { id: string; to: string; as?: string }) => daemonCliPost<{ ok: boolean }>(scope(), 'vault/grant', body),
  rotate: (body: { id: string; valueB64: string }) =>
    daemonCliPost<{ ok: boolean; version: number; trimmed?: boolean }>(scope(), 'vault/rotate', body),
  disable: (id: string) => daemonCliPost<{ ok: boolean }>(scope(), 'vault/disable', { id }),
  enable: (id: string) => daemonCliPost<{ ok: boolean }>(scope(), 'vault/enable', { id }),
  destroy: (id: string) => daemonCliPost<{ ok: boolean }>(scope(), 'vault/destroy', { id, yes: true }),
  confirmStart: (action: string, secretId: string) =>
    daemonCliPost<{ methods: string[]; expiresIn: number }>(scope(), 'vault/confirm/start', { action, secretId }),
  confirmFinish: (body: { action: string; secretId: string; method: string; password?: string; label?: string }) =>
    daemonCliPost<{ token: string; expiresIn: number }>(scope(), 'vault/confirm/finish', body),
  reveal: (id: string, confirmToken: string) =>
    daemonCliPost<{ valueB64: string; version: number }>(scope(), 'vault/reveal', { id, confirmToken }),
}
