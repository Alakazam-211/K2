// Agent-to-agent secret handoff client (prd-agent-secret-handoff-v1 §10,
// daemon 0.46.1, feature `vault-handoff`). Thin over `/cli/vault/incoming`,
// `/cli/vault/incoming/decide`, `/cli/vault/sent` and
// `/cli/vault/accept-from`. Names, peers and delivery ids only: no route
// here ever carries a secret value, and nothing here asks for one.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'

/** One delivery, as `/cli/vault/incoming` and `/cli/vault/sent` list it. */
export interface HandoffView {
  deliveryId: string
  direction: 'in' | 'out'
  /** `pending` | `held` | `landed` | `declined` | `expired` | `unsent` | `unknown` | `refused` */
  status: string
  workspaceId: string
  /** The local workspace's label. */
  workspace: string
  /** The other end: `sales` (this server) or `sales::rosson.k2.dev`. */
  peer: string
  cross: boolean
  name: string
  landedName?: string | null
  kind: string
  hosts: string[]
  note: string
  reason?: string | null
  createdAt: string
  decidedAt?: string | null
  decidedBy?: string | null
  expiresAt?: string | null
  /** `k2v://…` once it landed. */
  ref?: string | null
}

export interface HandoffDecideBody {
  id: string
  action: 'accept' | 'decline'
  as?: string
  replace?: boolean
  always?: boolean
}

export interface HandoffDecideResult {
  ok: boolean
  delivery: HandoffView
  ref?: string
  alwaysAdded?: string
}

export interface AcceptFromEntry {
  workspaceId: string
  /** `ws:<workspace id>` | `team:<group id>` | `agent::host` */
  sender: string
  addedBy: string
  addedAt: string
}

function str(v: unknown, fallback = ''): string {
  return typeof v === 'string' ? v : fallback
}

function optStr(v: unknown): string | null {
  return typeof v === 'string' ? v : null
}

/** Coerce one daemon row (unknown fields dropped; never a value field). */
export function coerceHandoffView(raw: unknown): HandoffView | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  const deliveryId = str(r.deliveryId)
  if (!deliveryId) return null
  return {
    deliveryId,
    direction: r.direction === 'out' ? 'out' : 'in',
    status: str(r.status, 'pending'),
    workspaceId: str(r.workspaceId),
    workspace: str(r.workspace) || str(r.workspaceId),
    peer: str(r.peer),
    cross: r.cross === true,
    name: str(r.name),
    landedName: optStr(r.landedName),
    kind: str(r.kind),
    hosts: Array.isArray(r.hosts) ? r.hosts.filter((h): h is string => typeof h === 'string') : [],
    note: str(r.note),
    reason: optStr(r.reason),
    createdAt: str(r.createdAt),
    decidedAt: optStr(r.decidedAt),
    decidedBy: optStr(r.decidedBy),
    expiresAt: optStr(r.expiresAt),
    ref: optStr(r.ref),
  }
}

function views(raw: unknown): HandoffView[] {
  const items = (raw as { items?: unknown } | null)?.items
  if (!Array.isArray(items)) return []
  return items.map(coerceHandoffView).filter((v): v is HandoffView => v !== null)
}

function entries(raw: unknown): AcceptFromEntry[] {
  const list = (raw as { entries?: unknown } | null)?.entries
  if (!Array.isArray(list)) return []
  const out: AcceptFromEntry[] = []
  for (const e of list) {
    if (!e || typeof e !== 'object') continue
    const o = e as Record<string, unknown>
    if (typeof o.sender !== 'string') continue
    out.push({ workspaceId: str(o.workspaceId), sender: o.sender, addedBy: str(o.addedBy), addedAt: str(o.addedAt) })
  }
  return out
}

export const handoffApi = {
  incoming: async (workspace?: string, scope: ServerScope = primaryScope()): Promise<HandoffView[]> =>
    views(await daemonCliGet(scope, 'vault/incoming', workspace ? { workspace } : undefined)),
  sent: async (workspace?: string, scope: ServerScope = primaryScope()): Promise<HandoffView[]> =>
    views(await daemonCliGet(scope, 'vault/sent', workspace ? { workspace } : undefined)),
  decide: (body: HandoffDecideBody, scope: ServerScope = primaryScope()): Promise<HandoffDecideResult> =>
    daemonCliPost<HandoffDecideResult>(scope, 'vault/incoming/decide', body),
  acceptFrom: async (workspace?: string, scope: ServerScope = primaryScope()): Promise<AcceptFromEntry[]> =>
    entries(await daemonCliGet(scope, 'vault/accept-from', workspace ? { workspace } : undefined)),
  editAcceptFrom: async (
    body: { workspace: string; sender: string; action: 'add' | 'remove' },
    scope: ServerScope = primaryScope(),
  ): Promise<AcceptFromEntry[]> => entries(await daemonCliPost(scope, 'vault/accept-from', body)),
}

/** `sales::rosson.k2.dev` → `{ agent: 'sales', host: 'rosson.k2.dev' }`. */
export function splitPeer(peer: string): { agent: string; host: string | null } {
  const i = peer.indexOf('::')
  if (i < 0) return { agent: peer, host: null }
  return { agent: peer.slice(0, i), host: peer.slice(i + 2) || null }
}

/** "16 Oct" from an RFC 3339 time (UTC, so tests and servers agree). */
export function shortDay(ts: string | null | undefined): string {
  if (!ts) return ''
  const d = new Date(ts)
  if (Number.isNaN(d.getTime())) return ''
  const months = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']
  return `${d.getUTCDate()} ${months[d.getUTCMonth()]}`
}

/** `hd_7f3c9a…` → `hd_7f3c…` (enough to match a log line). */
export function shortDelivery(id: string): string {
  return id.length > 7 ? `${id.slice(0, 7)}…` : id
}

/** The `handoff` provenance `/cli/vault/list|show` adds to a handed-off copy. */
export interface HandoffProvenance {
  deliveryId: string
  from: string
  at?: string | null
  decidedBy?: string | null
}

/** "sent by sales on rosson.k2.dev, 16 Oct (hd_7f3c…)"; without the
 *  daemon's provenance, "sent by another agent (hd_…)". */
export function handoffOriginText(origin: string, prov?: HandoffProvenance | null): string {
  const id = origin.startsWith('handoff:') ? origin.slice('handoff:'.length) : prov?.deliveryId || ''
  if (!prov || !prov.from) return `sent by another agent (${shortDelivery(id)})`
  const { agent, host } = splitPeer(prov.from)
  const who = host ? `${agent} on ${host}` : agent
  const day = shortDay(prov.at)
  return `sent by ${who}${day ? `, ${day}` : ''} (${shortDelivery(prov.deliveryId || id)})`
}

const KIND_WORDS: Record<string, string> = {
  api_key: 'api key',
  password: 'password',
  oauth_token: 'sign-in token',
  ssh_key: 'SSH key',
  tls_key: 'TLS key',
  dsn: 'database address',
  url: 'URL',
  generic: 'secret',
}

export function handoffKindWords(kind: string): string {
  return KIND_WORDS[kind] ?? kind.replace(/_/g, ' ')
}

/** Status in words for lists and cards. */
export function handoffStatusWords(status: string): string {
  switch (status) {
    case 'pending':
      return 'waiting for a person'
    case 'held':
      return 'held for this server\u2019s owner'
    case 'landed':
      return 'in the vault'
    case 'declined':
      return 'declined'
    case 'expired':
      return 'expired'
    case 'unsent':
      return 'taken back'
    case 'unknown':
      return 'no answer from the other server'
    case 'refused':
      return 'refused'
    default:
      return status
  }
}

// Settings → Vault opens on a tab when another page asks (the Agent tab's
// "Pending: N · Review"). One-shot: the Vault page reads it once on mount.
let pendingVaultTab: string | null = null

export function requestVaultTab(tab: 'incoming' | 'sent'): void {
  pendingVaultTab = tab
}

export function takeRequestedVaultTab(): string | null {
  const t = pendingVaultTab
  pendingVaultTab = null
  return t
}
