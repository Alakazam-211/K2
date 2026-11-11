// Settings → Vault: the words and the small rules the page and its dialogs
// share (how a delivery reads, which choices a kind gets, where a secret
// came from, how the list groups). No values ever pass through here.

import { handoffOriginText, shortDay } from './handoff-api'
import type { VaultSecret } from './vault-api'

/** How long a revealed value stays on screen. */
export const REVEAL_SHOW_MS = 30_000

const DELIVERY_LABEL: Record<string, string> = {
  fetch: 'K2 adds it to calls',
  proxy: 'K2 adds it to calls',
  agent: 'SSH signs through K2; nobody sees the key',
  file: 'A file the agent can read',
  env: 'An environment variable the agent can read',
  api: 'The agent can print it',
  none: 'Not given to agents',
}

export function deliveryLabel(d: string): string {
  return DELIVERY_LABEL[d] ?? d
}

/** Can an agent read the value with this delivery? (file, env, api; the
 * daemon's `Delivery::readable`). fetch, proxy and agent never show it. */
export function deliveryReadable(d: string): boolean {
  return d === 'file' || d === 'env' || d === 'api'
}

/** The delivery choices for a kind: `agent` only for SSH keys, and never
 * `fetch` for them (an SSH key is never put in a request). */
export function deliveryChoices(kind: string): string[] {
  return kind === 'ssh_key' ? ['agent', 'file', 'env'] : ['fetch', 'file', 'env']
}

/** The ceiling choices for a kind. */
export function ceilingChoices(kind: string): string[] {
  return [...deliveryChoices(kind), 'api']
}

/** `choices`, plus `current` when it isn't one of them, so a select always
 * shows the value it will send. */
export function withCurrent(choices: string[], current: string): string[] {
  return choices.includes(current) ? choices : [...choices, current]
}

/** Delivery modes as K2 menu options, labelled the way the vault says them. */
export function deliveryOptions(choices: string[]): { value: string; label: string }[] {
  return choices.map((d) => ({ value: d, label: deliveryLabel(d) }))
}

const KIND_LABEL: Record<string, string> = {
  api_key: 'API key',
  password: 'Password',
  oauth_token: 'Sign-in token',
  ssh_key: 'SSH key',
  tls_key: 'TLS key',
  dsn: 'Database address',
  url: 'URL',
  generic: 'Secret',
}

export function kindLabel(k: string): string {
  return KIND_LABEL[k] ?? k.replace(/_/g, ' ')
}

/** The kinds a person can add here. */
export const ADD_KINDS: { value: string; label: string }[] = [
  { value: 'api_key', label: 'API key' },
  { value: 'password', label: 'Password' },
  { value: 'oauth_token', label: 'Sign-in token' },
  { value: 'ssh_key', label: 'SSH key' },
  { value: 'generic', label: 'Something else' },
]

/** Kinds whose value is several lines (a PEM or OpenSSH key): a one-line
 * field would drop the line breaks and store a broken key. */
export function isMultilineKind(kind: string): boolean {
  return kind === 'ssh_key' || kind === 'tls_key'
}

/** Where a secret came from, in words. */
export function originLabel(s: Pick<VaultSecret, 'origin' | 'createdBy'> & Pick<Partial<VaultSecret>, 'handoff'>): string {
  if (s.origin.startsWith('thread:')) return 'Thread card'
  if (s.origin.startsWith('handoff:')) return handoffOriginText(s.origin, s.handoff)
  if (s.origin.startsWith('adopt:')) return 'moved in from older K2 storage'
  if (s.createdBy.startsWith('agent:')) return `agent ${s.createdBy.slice('agent:'.length)}`
  if (s.createdBy.startsWith('guest:')) return `guest ${s.createdBy.slice('guest:'.length)}`
  return 'a person'
}

/** Where a secret came from, as the start of a row's line. */
export function sourceWords(s: VaultSecret): string {
  if (s.origin.startsWith('thread:')) return 'Given in a Thread card'
  if (s.origin.startsWith('handoff:')) {
    const t = originLabel(s)
    return t.charAt(0).toUpperCase() + t.slice(1)
  }
  if (s.origin.startsWith('adopt:')) return 'Moved in from older K2 storage'
  return `Added by ${originLabel(s)}`
}

/** "10 Nov 08:30" from an RFC 3339 time (UTC, so tests and servers agree). */
export function whenWords(ts: string | null | undefined): string {
  if (!ts) return ''
  const day = shortDay(ts)
  const time = ts.slice(11, 16)
  return day ? `${day}${/^\d\d:\d\d$/.test(time) ? ` ${time}` : ''}` : ts.slice(0, 16).replace('T', ' ')
}

/** A history line's action, in words. */
const AUDIT_ACTION: Record<string, string> = {
  set: 'Saved',
  create: 'Saved',
  rotate: 'Rotated',
  use: 'Used',
  fetch: 'Used',
  get: 'Read by an agent',
  reveal: 'Shown to a person',
  grant: 'Shared',
  revoke: 'Sharing removed',
  policy: 'Settings changed',
  disable: 'Turned off',
  enable: 'Turned back on',
  destroy: 'Deleted',
}

export function auditActionWords(a: string): string {
  return AUDIT_ACTION[a] ?? a.charAt(0).toUpperCase() + a.slice(1).replace(/_/g, ' ')
}

/** Who did it, in words: `agent:atlas` → "agent atlas", `owner` → "the owner". */
export function actorWords(a: string): string {
  if (a === 'owner') return 'the owner'
  const i = a.indexOf(':')
  if (i < 0) return a
  const who = a.slice(i + 1)
  const kind = a.slice(0, i)
  if (kind === 'connect') return who
  if (kind === 'system') return 'K2'
  return `${kind} ${who}`
}

// ── Grouping ─────────────────────────────────────────────────────────

export interface SecretGroup {
  key: string
  title: string
  hint: string
  rows: VaultSecret[]
}

/** Server, then each workspace (A–Z), then chats, then anything else. */
export function groupSecrets(secrets: readonly VaultSecret[]): SecretGroup[] {
  const server: VaultSecret[] = []
  const chats: VaultSecret[] = []
  const other: VaultSecret[] = []
  const byWs = new Map<string, VaultSecret[]>()
  for (const s of secrets) {
    if (s.namespace !== 'user') continue
    const kind = s.scope?.kind
    if (kind === 'server') server.push(s)
    else if (kind === 'session') chats.push(s)
    else if (kind === 'workspace') {
      const label = s.workspace || s.scope.id || 'workspace'
      const list = byWs.get(label) ?? []
      list.push(s)
      byWs.set(label, list)
    } else other.push(s)
  }
  const byName = (a: VaultSecret, b: VaultSecret) => a.name.localeCompare(b.name)
  const out: SecretGroup[] = []
  if (server.length)
    out.push({ key: 'server', title: 'All agents on this server', hint: 'Any agent here can use these, at the sites you picked.', rows: server.sort(byName) })
  for (const ws of [...byWs.keys()].sort((a, b) => a.localeCompare(b))) {
    out.push({ key: `ws:${ws}`, title: ws, hint: `Only the ${ws} workspace’s agents can use these.`, rows: byWs.get(ws)!.sort(byName) })
  }
  if (chats.length) out.push({ key: 'session', title: 'Chats', hint: 'Each one is kept for the chat it was given in.', rows: chats.sort(byName) })
  if (other.length) out.push({ key: 'other', title: 'Other', hint: 'Kept for one job or service.', rows: other.sort(byName) })
  return out
}
