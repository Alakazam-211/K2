// People and channels on Home (prd-home-people-channels-v1 §5): how a Home
// row names a person or a channel on a server.
//
//   person   `<external id>::<host>`: the person's K2 external id on that
//            server (`k2:owner` or `k2:connect:<uuid>`). Opens the DM.
//   channel  `<Sew channel id>::<host>`: digits. Opens the channel.
//
// Like agent rows, `host` is this device's host key (`local`, a saved
// server); the sync edge turns it into the server's id (`server-ids.ts`).
// A row's kind rides on the row (`HomeRow.kind`, absent = an agent): an
// address alone can't say, since a channel id could be an agent's handle.
// Pure: no store, no I/O.

import { parseHomeAddress } from '@/lib/home-address'

export type HomeChatRowKind = 'person' | 'channel'

/** What a person or channel row opens. */
export type HomeChatTarget =
  | { kind: 'person'; hostKey: string; ext: string; address: string }
  | { kind: 'channel'; hostKey: string; conv: string; address: string }

const PERSON_EXT_RE = /^(k2:owner|k2:connect:[a-z0-9-]{1,64})$/
const CHANNEL_ID_RE = /^[0-9]{1,20}$/

/** A box person's external id (never an agent's `k2:workspace:` or an App
 *  user's `k2:principal:`). */
export function isPersonExt(ext: string): boolean {
  return PERSON_EXT_RE.test(ext.trim().toLowerCase())
}

/** A Sew channel id (digits). */
export function isChannelId(conv: string): boolean {
  return CHANNEL_ID_RE.test(conv.trim())
}

/** `<ext>::<host>`, lowercase; null when `ext` is not a box person. */
export function personAddress(ext: string, hostKey: string): string | null {
  const e = ext.trim().toLowerCase()
  const h = hostKey.trim().toLowerCase()
  if (!isPersonExt(e) || !h) return null
  return `${e}::${h}`
}

/** `<channel id>::<host>`; null when `conv` is not a Sew id. */
export function channelAddress(conv: string, hostKey: string): string | null {
  const c = conv.trim()
  const h = hostKey.trim().toLowerCase()
  if (!isChannelId(c) || !h) return null
  return `${c}::${h}`
}

/** Does `address` fit a row of `kind`? (An agent row takes any handle.) */
export function addressFitsKind(kind: HomeChatRowKind | 'agent', address: string): boolean {
  const p = parseHomeAddress(address)
  if (!p) return false
  if (kind === 'person') return isPersonExt(p.handle)
  if (kind === 'channel') return isChannelId(p.handle)
  return true
}

/** The kind of a stored row (`kind` absent: an agent). */
export function homeRowKind(row: { kind?: string }): HomeChatRowKind | 'agent' {
  return row.kind === 'person' || row.kind === 'channel' ? row.kind : 'agent'
}

/** True for a person or channel row. */
export function isHomeChatRow(row: { kind?: string }): boolean {
  return homeRowKind(row) !== 'agent'
}

/** What a person or channel row opens; null for an agent row or a row whose
 *  address doesn't fit its kind. */
export function homeChatTarget(row: { kind?: string; address: string }): HomeChatTarget | null {
  const kind = homeRowKind(row)
  if (kind === 'agent') return null
  const p = parseHomeAddress(row.address)
  if (!p) return null
  if (kind === 'person' && isPersonExt(p.handle)) {
    return { kind, hostKey: p.host, ext: p.handle, address: row.address }
  }
  if (kind === 'channel' && isChannelId(p.handle)) {
    return { kind, hostKey: p.host, conv: p.handle, address: row.address }
  }
  return null
}
