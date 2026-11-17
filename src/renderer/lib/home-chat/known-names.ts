// The last name this window saw for each person on each server.
//
// A DM whose person has left the server (disabled or removed) is read-only,
// and the server's people list no longer names them. Their DM is headed by
// the last name seen here instead: looked up by their address, else by the
// conversation's other author. Unknown: the caller says "Former member".
//
// Only a label for a person who is gone: the server's people list stays the
// truth for everyone on it. Kept in this window's storage so a reload still
// knows the name; a storage error just means no name.

import type { HomeChatPeople } from './api'

const KEY = 'k2.homeChat.knownNames.v1'
/** Names kept per server (the oldest go first). */
const MAX_PER_HOST = 500

type Names = Record<string, Record<string, string>>

let names: Names | null = null

function load(): Names {
  if (names) return names
  try {
    const raw = typeof localStorage !== 'undefined' ? localStorage.getItem(KEY) : null
    const parsed = raw ? (JSON.parse(raw) as unknown) : null
    names = parsed && typeof parsed === 'object' ? (parsed as Names) : {}
  } catch {
    names = {}
  }
  return names
}

function save(): void {
  try {
    if (typeof localStorage !== 'undefined' && names) localStorage.setItem(KEY, JSON.stringify(names))
  } catch {
    // No storage: the names last for this window only.
  }
}

/** Note everyone on `hostKey`'s people list, by address and by member id. */
export function rememberPeopleNames(hostKey: string, people: HomeChatPeople | null | undefined): void {
  if (!people) return
  const all = load()
  const host = { ...(all[hostKey] ?? {}) }
  let changed = false
  for (const p of people.people) {
    const name = p.displayName || p.username
    if (!name) continue
    for (const id of [`ext:${p.ext}`, p.memberId ? `member:${p.memberId}` : null]) {
      if (!id || host[id] === name) continue
      delete host[id]
      host[id] = name
      changed = true
    }
  }
  if (!changed) return
  const keys = Object.keys(host)
  for (const k of keys.slice(0, Math.max(0, keys.length - MAX_PER_HOST))) delete host[k]
  all[hostKey] = host
  save()
}

/** The last name seen for the person at `ext` on `hostKey`, else for the
 *  conversation's other author (`authorIds`, `me` skipped); null if none. */
export function formerPersonName(hostKey: string, ext: string, authorIds: readonly string[], me: string | null): string | null {
  const host = load()[hostKey]
  if (!host) return null
  const byExt = host[`ext:${ext}`]
  if (byExt) return byExt
  for (const a of authorIds) {
    if (a === me) continue
    const n = host[`member:${a}`]
    if (n) return n
  }
  return null
}

export function resetKnownNamesForTests(o: { keepStorage?: boolean } = {}): void {
  names = null
  if (o.keepStorage) return
  try {
    localStorage.removeItem(KEY)
  } catch {
    // none
  }
}
