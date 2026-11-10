// Thread grouping for the message list. Adapted from OCmail's
// lib/thread-utils.ts (MIT, Copyright (c) 2025 Matthieu MALVACHE, Copyright
// (c) 2026 Alakazam Labs; see ../NOTICE). OCmail keys rows by JMAP account +
// id because ids are account-local; K2's message ids already carry their
// inbox, so a row's id is its key. A message with no thread id is its own
// thread.

import type { ReaderEmail, ThreadGroup } from './types'

function time(iso: string): number {
  const n = new Date(iso).getTime()
  return Number.isFinite(n) ? n : 0
}

function threadKey(email: ReaderEmail): string {
  return `${email.inbox}\u0000${email.threadId || `solo:${email.id}`}`
}

/** Unique sender names across a thread, newest first (OCmail's rule). */
export function getThreadParticipants(emails: ReaderEmail[], maxNames = 4): string[] {
  const seen = new Set<string>()
  const names: string[] = []
  for (const email of emails) {
    const sender = email.from
    if (sender && sender.address) {
      const key = sender.address.toLowerCase()
      if (!seen.has(key)) {
        seen.add(key)
        names.push(sender.name || sender.address.split('@')[0])
      }
    }
    if (names.length >= maxNames) break
  }
  return names
}

function toGroup(threadId: string, threadEmails: ReaderEmail[]): ThreadGroup {
  const emails = [...threadEmails].sort((a, b) => time(b.date) - time(a.date))
  return {
    threadId,
    emails,
    latestEmail: emails[0],
    participantNames: getThreadParticipants(emails),
    hasUnread: emails.some((e) => e.unread),
    hasAttachment: emails.some((e) => e.hasAttachment),
    emailCount: emails.length,
  }
}

/** Groups messages by thread; a single message is a group of one. */
export function groupEmailsByThread(emails: ReaderEmail[]): ThreadGroup[] {
  if (!emails || emails.length === 0) return []
  const map = new Map<string, ReaderEmail[]>()
  for (const email of emails) {
    const key = threadKey(email)
    const list = map.get(key)
    if (list) list.push(email)
    else map.set(key, [email])
  }
  return [...map.values()].map((list) => toGroup(list[0].threadId || list[0].id, list))
}

/** Newest conversation first. */
export function sortThreadGroups(groups: ThreadGroup[]): ThreadGroup[] {
  return [...groups].sort((a, b) => time(b.latestEmail.date) - time(a.latestEmail.date))
}

/** Adds fetched thread messages to a group (no duplicates). */
export function mergeThreadEmails(group: ThreadGroup, fetched: ReaderEmail[]): ThreadGroup {
  const byId = new Map<string, ReaderEmail>()
  for (const e of group.emails) byId.set(e.id, e)
  for (const e of fetched) if (!byId.has(e.id)) byId.set(e.id, e)
  return toGroup(group.threadId, [...byId.values()])
}
