// `GET/POST /cli/people/name` (0.46.0): a person's optional first and
// last name. Any login reads them and sets its own; an Admin+ sets
// someone else's (Owner tier for the owner person), the same floors as
// `/cli/people/color`. On POST an absent key is left alone; null or ""
// clears it.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { asNameMap, parsePersonNames, type PersonNames } from '@/lib/person-name'

export interface PersonNameView {
  /** The person key (`owner` for the owner and owner-linked logins). */
  person: string
  firstName: string | null
  lastName: string | null
  displayName: string | null
  names: Record<string, PersonNames>
}

export function parsePersonName(raw: unknown): PersonNameView {
  const o = raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
  const n = parsePersonNames(o)
  return {
    person: typeof o.person === 'string' ? o.person : '',
    firstName: n.firstName ?? null,
    lastName: n.lastName ?? null,
    displayName: n.displayName ?? null,
    names: asNameMap(o.names),
  }
}

/** The names of `person` (default: the caller), or null when the daemon
 *  predates the route. */
export async function fetchPersonName(scope: ServerScope, person?: string): Promise<PersonNameView | null> {
  try {
    return parsePersonName(await daemonCliGet<unknown>(scope, 'people/name', person ? { person } : undefined))
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e)
    if (msg.includes('route not found')) return null
    throw e
  }
}

export interface PersonNameUpdate {
  /** Omitted: the caller. */
  person?: string
  firstName?: string | null
  lastName?: string | null
}

export async function setPersonName(scope: ServerScope, update: PersonNameUpdate): Promise<PersonNameView> {
  return parsePersonName(await daemonCliPost<unknown>(scope, 'people/name', { ...update }))
}
