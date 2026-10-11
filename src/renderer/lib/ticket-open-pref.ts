// "Open the Tickets page on" (Settings → Tickets → Just for you, 0.45.4;
// GitHub #75). A PERSONAL view preference: Waiting (the default) | All |
// Remember my last tab.
//
// Storage: the renderer's localStorage, keyed by server + login
// (`k2.tickets.openOn.<host>.<user>`), the same way other per-person view
// prefs are kept (`k2.workspaceSwitchFocus`). The daemon has no per-user
// preference store today (settings.json is server-wide, the Connect user
// store holds identity only), and server-wide state must not change one
// person's landing tab for everyone. Keying by host + user keeps two
// people on one computer, or one person on two servers, apart. It stays
// on this computer (it does not follow the person to another device).
//
// The Tickets board reads it when it mounts; with "Remember my last tab"
// the board also writes the tab the person last picked.

import { useConnectHostStore } from '@/stores/connect-host'
import { primaryScope } from '@/kessel/server-scope'
import type { TicketScope } from '@/components/Feedback/feedback-api'

export type TicketOpenOn = 'waiting' | 'all' | 'last'

export const DEFAULT_TICKET_OPEN_ON: TicketOpenOn = 'waiting'

export const TICKET_OPEN_ON_OPTIONS: { value: TicketOpenOn; label: string }[] = [
  { value: 'waiting', label: 'Waiting on me' },
  { value: 'all', label: 'All' },
  { value: 'last', label: 'Remember my last tab' },
]

const OPEN_ON_PREFIX = 'k2.tickets.openOn.'
const LAST_TAB_PREFIX = 'k2.tickets.lastTab.'
const SCOPES: readonly TicketScope[] = ['mine', 'waiting', 'all']

/** Who this preference belongs to: the window's server and its login
 *  (`owner` on this computer or a raw-token server; the Connect username
 *  otherwise). */
export function ticketPrefOwner(): { host: string; user: string } {
  let host = 'local'
  try {
    host = primaryScope().hostKey || 'local'
  } catch {
    host = 'local'
  }
  const active = useConnectHostStore.getState().activeHost
  const user = active === 'local' ? 'owner' : active.username?.trim() || 'owner'
  return { host, user }
}

function keyFor(prefix: string, owner = ticketPrefOwner()): string {
  return `${prefix}${owner.host}.${owner.user}`
}

function storage(): Storage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

function read(key: string): string | null {
  const s = storage()
  if (!s) return null
  try {
    return s.getItem(key)
  } catch {
    return null
  }
}

function write(key: string, value: string): void {
  const s = storage()
  if (!s) return
  try {
    s.setItem(key, value)
  } catch {
    // Private mode / quota: the board still opens on the default.
  }
}

export function readTicketOpenOn(owner = ticketPrefOwner()): TicketOpenOn {
  const v = read(keyFor(OPEN_ON_PREFIX, owner))
  return v === 'waiting' || v === 'all' || v === 'last' ? v : DEFAULT_TICKET_OPEN_ON
}

export function writeTicketOpenOn(value: TicketOpenOn, owner = ticketPrefOwner()): void {
  write(keyFor(OPEN_ON_PREFIX, owner), value)
}

export function readLastTicketTab(owner = ticketPrefOwner()): TicketScope | null {
  const v = read(keyFor(LAST_TAB_PREFIX, owner))
  return SCOPES.includes(v as TicketScope) ? (v as TicketScope) : null
}

/** Remember the tab the person picked (always written; read only with
 *  "Remember my last tab", so switching the preference later starts from
 *  where they were). */
export function writeLastTicketTab(scope: TicketScope, owner = ticketPrefOwner()): void {
  write(keyFor(LAST_TAB_PREFIX, owner), scope)
}

/** The tab the Tickets board opens on. */
export function initialTicketScope(owner = ticketPrefOwner()): TicketScope {
  const pref = readTicketOpenOn(owner)
  if (pref === 'all') return 'all'
  if (pref === 'last') return readLastTicketTab(owner) ?? 'waiting'
  return 'waiting'
}
