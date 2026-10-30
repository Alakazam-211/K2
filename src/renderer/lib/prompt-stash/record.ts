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

/** One attachment's metadata. Its bytes live beside the record, by `id`,
 *  on the device that saved it (stash-sync-v1: bytes never leave it). */
export interface StashAttachment {
  id: string
  name: string
  /** MIME type ('' when unknown). */
  type: string
  /** Bytes. */
  size: number
  /** stash-sync-v1: the device that holds the file (its Homes device id).
   *  Absent on a record saved before sync: this device's. */
  device_id?: string | null
  /** stash-sync-v1: where the file is on that device, or null (the phone,
   *  the web, a file that only exists on a remote server). */
  path?: string | null
}

export interface StashQuote {
  text: string
  /** Who said it (an agent's address, "Inbox"); null when nobody. */
  from: string | null
}

/** Where a stash was written. For display only; a stash can be sent to
 *  any agent on any server. Field rules settled after the Companion 4.1.4
 *  review: an absent value is null, never "". */
export interface StashSource {
  /** The server's host key (`rosson.k2.dev`, `ip:port`): the same on every
   *  device, never this device's `local`. Null when it isn't known. */
  server_id: string | null
  server_label: string | null
  /** The login the server reports for this person (the owner's real
   *  username where known, else `owner`). */
  user: string | null
  /** The agent's address: the one it was written to, or the sender's
   *  (`doc.from`, group chats too). Null only when there's no sender. */
  agent: string | null
  agent_name: string | null
}

/** "" and non-strings are absent: null. */
export function orNull(v: unknown): string | null {
  return typeof v === 'string' && v !== '' ? v : null
}

/** `source.server_id`: "local" (a device-local id) reads as unknown. */
function serverIdOrNull(v: unknown): string | null {
  const s = orNull(v)
  return s && s.toLowerCase() === 'local' ? null : s
}

function cleanSource(src: Partial<Record<keyof StashSource, unknown>>): StashSource {
  return {
    server_id: serverIdOrNull(src.server_id),
    server_label: orNull(src.server_label),
    user: orNull(src.user),
    agent: orNull(src.agent),
    agent_name: orNull(src.agent_name),
  }
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
  /** stash-sync-v1: the device that made `updated_at` (the server sets it
   *  on push). The merge tie-break. Absent on a never-synced record. */
  updated_by?: string | null
  /** Set on delete (text and attachments cleared). */
  deleted_at: number | null
}

/** What a save hands in: the record's content, without ids or clocks.
 *  Absent source fields may be left out or "": they are stored as null. */
export interface StashDraft {
  text: string
  quotes?: Array<{ text: string; from?: string | null }>
  source: Partial<StashSource>
}

/** An attachment to save: its metadata plus the bytes. */
export interface StashFileInput {
  name: string
  type: string
  bytes: Blob
  /** The file's path on THIS device, when it has one (the desktop app on
   *  its own daemon); null otherwise. */
  path?: string | null
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
  deviceId: string | null = null,
): StashRecord {
  return {
    v: 1,
    id,
    text: draft.text,
    attachments,
    quotes: (draft.quotes ?? []).filter((q) => q.text.trim().length > 0).map((q) => ({ text: q.text, from: orNull(q.from) })),
    source: cleanSource(draft.source),
    created_at: now,
    updated_at: now,
    ...(deviceId ? { updated_by: deviceId } : {}),
    deleted_at: null,
  }
}

/** The tombstone a delete leaves: text and attachments cleared. */
export function tombstone(rec: StashRecord, now: number, deviceId: string | null = null): StashRecord {
  const t: StashRecord = { ...rec, text: '', attachments: [], quotes: [], deleted_at: now, updated_at: now }
  if (deviceId) t.updated_by = deviceId
  return t
}

/** stash-sync-v1 merge rule (the server's too): the larger `updated_at`;
 *  on a tie the larger `updated_by` (string compare); on a full tie the
 *  tombstone. Is `a` the winner over `b`? */
export function stashBeats(a: StashRecord, b: StashRecord): boolean {
  if (a.updated_at !== b.updated_at) return a.updated_at > b.updated_at
  const da = a.updated_by ?? ''
  const db = b.updated_by ?? ''
  if (da !== db) return da > db
  return a.deleted_at !== null && b.deleted_at === null
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

/** A record read back from storage, or null when it isn't one of ours. A
 *  record of another version (`v` other than 1, a newer client's) is null
 *  too: it is hidden from the list and never rewritten (field rules). */
export function coerceStashRecord(raw: unknown): StashRecord | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  if (r.v !== 1 || typeof r.id !== 'string' || !r.id) return null
  if (typeof r.created_at !== 'number' || typeof r.updated_at !== 'number') return null
  const deleted = typeof r.deleted_at === 'number' ? r.deleted_at : null
  const src = r.source && typeof r.source === 'object' ? (r.source as Record<string, unknown>) : {}
  const str = (v: unknown): string => (typeof v === 'string' ? v : '')
  // Fields this client doesn't know are kept as they came (a newer
  // client's: stash-sync-v1 is additive); the known ones are normalized.
  const attachments = Array.isArray(r.attachments)
    ? r.attachments.flatMap((a): StashAttachment[] => {
        if (!a || typeof a !== 'object') return []
        const o = a as Record<string, unknown>
        if (typeof o.id !== 'string' || !o.id) return []
        const att: StashAttachment = { ...o, id: o.id, name: str(o.name) || 'file', type: str(o.type), size: typeof o.size === 'number' ? o.size : 0 }
        if ('device_id' in o) att.device_id = orNull(o.device_id)
        if ('path' in o) att.path = orNull(o.path)
        return [att]
      })
    : []
  const quotes = Array.isArray(r.quotes)
    ? r.quotes.flatMap((q): StashQuote[] => {
        if (!q || typeof q !== 'object') return []
        const o = q as Record<string, unknown>
        if (typeof o.text !== 'string') return []
        return [{ text: o.text, from: orNull(o.from) }]
      })
    : []
  const out: StashRecord = {
    ...r,
    v: 1,
    id: r.id,
    text: str(r.text),
    attachments,
    quotes,
    source: { ...src, ...cleanSource(src) },
    created_at: r.created_at,
    updated_at: r.updated_at,
    deleted_at: deleted,
  }
  if ('updated_by' in r) out.updated_by = orNull(r.updated_by)
  return out
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

// ── An agent's offer (`stash.offer` card) ──────────────────────────────

/** Offer limits (field rules): longer is cut, with a note on the card. */
export const STASH_OFFER_TITLE_MAX = 200
export const STASH_OFFER_TEXT_MAX = 20_000

/** `s` cut to at most `max` characters (code points, so an emoji is never
 *  split), the last one an ellipsis. */
function cutChars(s: string, max: number): { out: string; cut: boolean } {
  const chars = Array.from(s)
  if (chars.length <= max) return { out: s, cut: false }
  return { out: `${chars.slice(0, max - 1).join('')}…`, cut: true }
}

export interface StashOfferView {
  title: string | null
  text: string
  /** Something was cut: the card says so. */
  truncated: boolean
  /** What a tap saves: exactly what the card shows (title, blank line,
   *  text), no hidden content. */
  saved: string
}

export function stashOfferView(offer: { text: string; title?: string | null }): StashOfferView {
  // The title is one line: newlines and runs of whitespace become one
  // space before the cap (K2 Companion's rule).
  const oneLine = (offer.title ?? '').replace(/\s+/g, ' ').trim()
  const t = oneLine ? cutChars(oneLine, STASH_OFFER_TITLE_MAX) : null
  const x = cutChars(offer.text, STASH_OFFER_TEXT_MAX)
  const title = t ? t.out : null
  return {
    title,
    text: x.out,
    truncated: x.cut || (t?.cut ?? false),
    saved: title ? `${title}\n\n${x.out}` : x.out,
  }
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
