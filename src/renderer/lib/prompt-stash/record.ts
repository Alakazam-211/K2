// Prompt stash (spec-prompt-stash-v1, 0.46.7): the shared record shape,
// agreed with K2 Companion on 2026-10-29 ("Shared record"). Pure: no
// storage, no React. snake_case on the wire, millisecond epochs, a stable
// uuid per item and a tombstone on delete, so a later device sync (with
// Home sync) can merge lists without a migration.
//
// The idea (Cmd+S saves the draft and clears the box, Cmd+S on an empty box
// brings stashes back, restoring appends) is T3 Code's "Prompt stash"
// (MIT, `apps/web/src/promptStashStore.ts`, `useComposerStash.ts`). K2
// keeps its own shape: IndexedDB instead of localStorage, attachment bytes
// beside the record, a 50 cap that asks, and items that stay after use.

/** The most live items one device keeps. The 51st save asks first. */
export const STASH_CAP = 50
/** A tombstone is kept this long (ms), then dropped. */
export const STASH_TOMBSTONE_TTL_MS = 30 * 24 * 60 * 60 * 1000
/** The question the 51st save asks. */
export const STASH_FULL_MESSAGE = `Your stash is full (${STASH_CAP}). Remove the oldest?`

/** One attachment's metadata. Its bytes live beside the record, by `id`. */
export interface StashAttachment {
  id: string
  name: string
  /** MIME type ('' when unknown). */
  type: string
  /** Bytes. */
  size: number
}

export interface StashQuote {
  text: string
  /** Who said it (an agent's address, "Inbox"). */
  from?: string
}

/** Where a stash was written. For display only; a stash can be sent to
 *  any agent on any server. */
export interface StashSource {
  /** The server's host key (`local`, `<sub>.k2.dev`, `ip:port`). */
  server_id: string
  server_label: string
  /** The person on that server (`owner`, a Connect username). */
  user: string
  /** The agent's address it was written to (or came from). */
  agent: string
  agent_name: string
}

export interface StashRecord {
  v: 1
  id: string
  text: string
  attachments: StashAttachment[]
  quotes: StashQuote[]
  source: StashSource
  created_at: number
  updated_at: number
  /** Set on delete (text and attachments cleared). */
  deleted_at: number | null
}

/** What a save hands in: the record's content, without ids or clocks. */
export interface StashDraft {
  text: string
  quotes?: StashQuote[]
  source: StashSource
}

/** An attachment to save: its metadata plus the bytes. */
export interface StashFileInput {
  name: string
  type: string
  bytes: Blob
}

export function newStashId(): string {
  const c = (globalThis as { crypto?: Crypto }).crypto
  if (c && typeof c.randomUUID === 'function') return c.randomUUID()
  // RFC 4122 v4 from getRandomValues (older webviews).
  const b = new Uint8Array(16)
  if (c && typeof c.getRandomValues === 'function') c.getRandomValues(b)
  else for (let i = 0; i < 16; i++) b[i] = Math.floor(Math.random() * 256)
  b[6] = (b[6] & 0x0f) | 0x40
  b[8] = (b[8] & 0x3f) | 0x80
  const h = Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('')
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`
}

export function buildStashRecord(
  draft: StashDraft,
  attachments: StashAttachment[],
  now: number,
  id: string = newStashId(),
): StashRecord {
  return {
    v: 1,
    id,
    text: draft.text,
    attachments,
    quotes: (draft.quotes ?? []).filter((q) => q.text.trim().length > 0).map((q) => (q.from ? { text: q.text, from: q.from } : { text: q.text })),
    source: { ...draft.source },
    created_at: now,
    updated_at: now,
    deleted_at: null,
  }
}

/** The tombstone a delete leaves: text and attachments cleared. */
export function tombstone(rec: StashRecord, now: number): StashRecord {
  return { ...rec, text: '', attachments: [], quotes: [], deleted_at: now, updated_at: now }
}

export function isLive(rec: StashRecord): boolean {
  return rec.deleted_at === null
}

/** Live items, newest first (ties: the later id last-written wins by updated_at). */
export function liveNewestFirst(all: readonly StashRecord[]): StashRecord[] {
  return all.filter(isLive).sort((a, b) => b.created_at - a.created_at || b.updated_at - a.updated_at)
}

/** The live item a full stash would remove: the oldest. */
export function oldestLive(all: readonly StashRecord[]): StashRecord | null {
  const live = liveNewestFirst(all)
  return live.length > 0 ? live[live.length - 1] : null
}

/** Tombstones older than the TTL: drop them. */
export function expiredTombstones(all: readonly StashRecord[], now: number): StashRecord[] {
  return all.filter((r) => r.deleted_at !== null && now - r.deleted_at > STASH_TOMBSTONE_TTL_MS)
}

/** A record read back from storage, or null when it isn't one of ours. */
export function coerceStashRecord(raw: unknown): StashRecord | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  if (r.v !== 1 || typeof r.id !== 'string' || !r.id) return null
  if (typeof r.created_at !== 'number' || typeof r.updated_at !== 'number') return null
  const deleted = typeof r.deleted_at === 'number' ? r.deleted_at : null
  const src = r.source && typeof r.source === 'object' ? (r.source as Record<string, unknown>) : {}
  const str = (v: unknown): string => (typeof v === 'string' ? v : '')
  const attachments = Array.isArray(r.attachments)
    ? r.attachments.flatMap((a): StashAttachment[] => {
        if (!a || typeof a !== 'object') return []
        const o = a as Record<string, unknown>
        if (typeof o.id !== 'string' || !o.id) return []
        return [{ id: o.id, name: str(o.name) || 'file', type: str(o.type), size: typeof o.size === 'number' ? o.size : 0 }]
      })
    : []
  const quotes = Array.isArray(r.quotes)
    ? r.quotes.flatMap((q): StashQuote[] => {
        if (!q || typeof q !== 'object') return []
        const o = q as Record<string, unknown>
        if (typeof o.text !== 'string') return []
        return [typeof o.from === 'string' && o.from ? { text: o.text, from: o.from } : { text: o.text }]
      })
    : []
  return {
    v: 1,
    id: r.id,
    text: str(r.text),
    attachments,
    quotes,
    source: {
      server_id: str(src.server_id),
      server_label: str(src.server_label),
      user: str(src.user),
      agent: str(src.agent),
      agent_name: str(src.agent_name),
    },
    created_at: r.created_at,
    updated_at: r.updated_at,
    deleted_at: deleted,
  }
}

// ── Putting a stash back in a composer ─────────────────────────────────

/** A stash's words as they go into a composer: its quotes as Markdown
 *  quote blocks (the composer has no quote chips), then the text. */
export function stashBody(rec: Pick<StashRecord, 'text' | 'quotes'>): string {
  const blocks = rec.quotes.map((q) => {
    const lines = q.text.replace(/\s+$/, '').split('\n').map((l) => `> ${l}`.trimEnd())
    if (q.from) lines.unshift(`> ${q.from}:`)
    return lines.join('\n')
  })
  const text = rec.text.replace(/\s+$/, '')
  if (text) blocks.push(text)
  return blocks.join('\n\n')
}

/** The draft after a stash is chosen: below any text already there,
 *  after a blank line. Never replaces what the person has typed. */
export function insertStashBelow(draft: string, body: string): string {
  if (!body) return draft
  const head = draft.replace(/\s+$/, '')
  if (!head) return body
  return `${head}\n\n${body}`
}

// ── The drawer's words ─────────────────────────────────────────────────

/** One line for the drawer: the text's first words, else the first quote,
 *  else the attachments' names. */
export function stashPreview(rec: Pick<StashRecord, 'text' | 'quotes' | 'attachments'>, max = 90): string {
  const flat = (s: string): string => s.replace(/\s+/g, ' ').trim()
  const line = flat(rec.text) || flat(rec.quotes[0]?.text ?? '') || rec.attachments.map((a) => a.name).join(', ')
  return line.length > max ? `${line.slice(0, max - 1)}…` : line
}

/** "sales on rosson.k2.dev": where it was written. */
export function stashSourceLabel(src: StashSource): string {
  const agent = src.agent_name || src.agent
  const server = src.server_label || src.server_id
  if (agent && server) return `${agent} on ${server}`
  return agent || server || ''
}

/** "now", "5m", "3h", "2d", "4w": the item's age. */
export function stashAge(createdAt: number, now: number): string {
  const s = Math.max(0, Math.floor((now - createdAt) / 1000))
  if (s < 60) return 'now'
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h`
  const d = Math.floor(h / 24)
  if (d < 7) return `${d}d`
  return `${Math.floor(d / 7)}w`
}
