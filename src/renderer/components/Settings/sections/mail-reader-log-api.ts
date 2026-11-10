// Settings → Cores → Email Reader Log (prd-ocmail-admin-in-k2-v1 OA2): the
// mailbox access log, read from GET /cli/mail/reader/log. The daemon decides
// what each viewer sees (Owner and Admin: every row; a Member: rows about
// their own mailboxes) and never returns message content.

import { daemonCliGet } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

export interface ReaderLogRow {
  id: number
  /** Unix seconds. */
  at: number
  actor: string
  actorRole: string
  source: string
  address: string
  kind: string
  own: boolean
  action: string
  messageRef?: string | null
  reason?: string | null
  ipHint?: string | null
  outcome: string
  status?: number | null
}

export interface ReaderLogFilters {
  inbox?: string
  actor?: string
  /** '24h' | '7d' | '30d' | '' (all). */
  since?: string
  limit?: number
}

export const READER_LOG_SINCE_CHOICES: { value: string; label: string }[] = [
  { value: '24h', label: 'Last 24 hours' },
  { value: '7d', label: 'Last 7 days' },
  { value: '30d', label: 'Last 30 days' },
  { value: '', label: 'Everything kept' },
]

export const READER_LOG_DEFAULT_LIMIT = 200

export async function fetchReaderLog(scope: ServerScope, f: ReaderLogFilters): Promise<ReaderLogRow[]> {
  const r = await daemonCliGet<{ rows?: ReaderLogRow[] }>(scope, 'mail/reader/log', {
    inbox: f.inbox?.trim() || undefined,
    actor: f.actor?.trim() || undefined,
    since: f.since || undefined,
    limit: f.limit ?? READER_LOG_DEFAULT_LIMIT,
  })
  if (!Array.isArray(r?.rows)) throw new Error('the reader log answered without rows')
  return r.rows
}

const ACTION_LABELS: Record<string, string> = {
  'reader/inboxes': 'Listed inboxes',
  'reader/folders': 'Listed folders',
  'reader/messages': 'Listed messages',
  'reader/message': 'Opened a message',
  'reader/thread': 'Opened a conversation',
  'reader/attachment': 'Downloaded an attachment',
  'reader/move': 'Moved a message',
  'reader/flag': 'Changed read or star',
  'reader/archive': 'Archived a message',
  'reader/delete': 'Moved a message to Trash',
  'reader/draft': 'Saved a draft',
  'reader/reply': 'Replied',
  'reader/send': 'Sent a message',
  // The agent tab's Inbox and k2 mail through a named workspace (OA0).
  read: 'Opened a message (agent tab)',
  messages: 'Listed messages (agent tab)',
  attachments: 'Read attachments (agent tab)',
  move: 'Moved a message (agent tab)',
  flag: 'Changed read or star (agent tab)',
  archive: 'Archived a message (agent tab)',
  delete: 'Moved a message to Trash (agent tab)',
  reply: 'Replied (agent tab)',
  send: 'Sent a message (agent tab)',
  draft: 'Saved a draft (agent tab)',
  inboxes: 'Listed inboxes (agent tab)',
}

/** "Opened a message", or the raw action for one K2 doesn't name yet. */
export function readerActionLabel(action: string): string {
  return ACTION_LABELS[action] ?? action
}

/** "pat (admin)" from `user:pat` / "Owner token". */
export function readerActorLabel(row: Pick<ReaderLogRow, 'actor' | 'actorRole'>): string {
  if (row.actor === 'owner-token') return 'Owner token'
  const name = row.actor.startsWith('user:') ? row.actor.slice(5) : row.actor
  return row.actorRole ? `${name} (${row.actorRole})` : name
}

export function readerLogTime(unix: number): string {
  return Number.isFinite(unix) && unix > 0 ? new Date(unix * 1000).toLocaleString() : '—'
}
