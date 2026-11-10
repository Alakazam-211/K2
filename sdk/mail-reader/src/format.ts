// Small formatting helpers for the views. `formatRecipients` and the folder
// order are adapted from OCmail's email-viewer.tsx and sidebar.tsx (MIT,
// Copyright (c) 2025 Matthieu MALVACHE, Copyright (c) 2026 Alakazam Labs; see
// ../NOTICE). Dates use Intl (no date library).

import { t, type MessageKey } from './i18n'
import type { InboxProvider, ReaderAddress, ReaderFolder, ReaderInbox } from './types'

/** Today: the time. This year: day and month. Older: the full date. */
export function formatDate(iso: string, now: Date = new Date()): string {
  const d = new Date(iso)
  if (!Number.isFinite(d.getTime())) return ''
  const sameDay = d.toDateString() === now.toDateString()
  if (sameDay) return d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })
  if (d.getFullYear() === now.getFullYear()) return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
  return d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
}

export function formatFullDate(iso: string): string {
  const d = new Date(iso)
  return Number.isFinite(d.getTime()) ? d.toLocaleString() : ''
}

export function displayAddress(a: ReaderAddress | null | undefined): string {
  if (!a) return t('unknown_sender')
  const name = typeof a.name === 'string' ? a.name.trim() : ''
  return name || a.address || t('unknown_sender')
}

/** "Ann, Bo" or "Ann and 3 others" (OCmail's rule, without its "me"). */
export function formatRecipients(recipients: ReaderAddress[] | undefined): string {
  if (!recipients || recipients.length === 0) return ''
  if (recipients.length > 2) {
    return t('recipient_and_others', { name: displayAddress(recipients[0]), count: recipients.length - 1 })
  }
  return recipients.map(displayAddress).join(', ')
}

export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return ''
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

const ROLE_ORDER = ['inbox', 'sent', 'drafts', 'junk', 'trash', 'archive'] as const

/** OCmail's sidebar guesses a role from the name when the server has none. */
export function folderRole(f: ReaderFolder): string | null {
  if (f.role) return f.role.toLowerCase()
  const n = f.name.toLowerCase()
  if (n === 'inbox' || n.includes('inbox')) return 'inbox'
  if (n.includes('sent')) return 'sent'
  if (n.includes('draft')) return 'drafts'
  if (n.includes('junk') || n.includes('spam')) return 'junk'
  if (n.includes('trash') || n.includes('deleted')) return 'trash'
  if (n.includes('archive')) return 'archive'
  return null
}

/** Role folders first, in a fixed order; named folders after, A–Z. */
export function folderOrder(folders: ReaderFolder[]): ReaderFolder[] {
  const rank = (f: ReaderFolder): number => {
    const r = folderRole(f)
    const i = r ? (ROLE_ORDER as readonly string[]).indexOf(r) : -1
    return i < 0 ? ROLE_ORDER.length : i
  }
  return [...folders].sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name))
}

export function folderLabel(f: ReaderFolder): string {
  const r = folderRole(f)
  if (r && (ROLE_ORDER as readonly string[]).includes(r) && f.role) return t(`folder_${r}` as MessageKey)
  return f.name
}

/** What to send as `folder=`: the role when there is one, else the name. */
export function folderParam(f: ReaderFolder): string {
  return f.role || f.name
}

export function providerLabel(p: InboxProvider): string {
  return t(`provider_${p}` as MessageKey)
}

/** Who an inbox belongs to, for grouping: the person, else the workspace. */
export function inboxOwnerLabel(inbox: ReaderInbox): string {
  if (inbox.kind === 'person' && inbox.person) return inbox.person
  return inbox.workspace || inbox.person || t('owner_unknown')
}

/** "a@b.c, d@e.f" → ["a@b.c", "d@e.f"]. */
export function splitAddresses(raw: string): string[] {
  return raw
    .split(/[,;\s]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
}
