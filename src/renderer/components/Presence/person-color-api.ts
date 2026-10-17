// Person colours (0.46.0) — the daemon owns them (`k2_core::person_color`):
//
//   GET  /cli/people/color   any login   {palette, person, color, chosen, colors}
//   POST /cli/people/color   any login   {color: "<palette hex>" | null, user?}
//
// No `user` = the caller's own person: for the owner token or an
// owner-linked login that is the owner's colour (the linked logins show it
// too); for anyone else their own. `null` = Automatic (the name hash).
// An older daemon has no route (404 `route not found`): the profile shows
// no swatches and every chip keeps hashing the name.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { PERSON_COLORS } from './PresenceChip'

export interface PersonColorView {
  /** The eight hues (the daemon's list; `PERSON_COLORS` order). */
  palette: string[]
  /** The caller's person key (`owner` for the owner and linked logins). */
  person: string
  /** The caller's effective colour. */
  color: string
  /** The caller's chosen colour, or null for Automatic. */
  chosen: string | null
  /** Name → effective colour for everyone. */
  colors: Record<string, string>
}

function rec(v: unknown): Record<string, unknown> {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {}
}

export function parsePersonColor(raw: unknown): PersonColorView {
  const o = rec(raw)
  const palette = Array.isArray(o.palette) ? o.palette.filter((c): c is string => typeof c === 'string') : []
  const colors: Record<string, string> = {}
  for (const [k, v] of Object.entries(rec(o.colors))) if (typeof v === 'string') colors[k] = v
  return {
    palette: palette.length > 0 ? palette : [...PERSON_COLORS],
    person: typeof o.person === 'string' ? o.person : '',
    color: typeof o.color === 'string' ? o.color : '',
    chosen: typeof o.chosen === 'string' ? o.chosen : null,
    colors,
  }
}

/** The caller's colour, or null when the daemon predates the route. */
export async function fetchPersonColor(scope: ServerScope): Promise<PersonColorView | null> {
  try {
    return parsePersonColor(await daemonCliGet<unknown>(scope, 'people/color'))
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e)
    if (msg.includes('route not found')) return null
    throw e
  }
}

/** Set the caller's own colour (`null` = Automatic). */
export async function setPersonColor(scope: ServerScope, color: string | null): Promise<PersonColorView> {
  return parsePersonColor(await daemonCliPost<unknown>(scope, 'people/color', { color }))
}
