// Person names (0.46.0, Rosson 2026-10-17): an optional first and last
// name for every person. The daemon owns them (`/cli/people/name`; the
// owner's in `ownerFirstName` / `ownerLastName`, a Connect login's on its
// `connect-users.json` row) and serves `firstName`, `lastName` and a
// computed `displayName` ("First Last", else the username) on whoami, the
// users list, the presence roster (rows + a `names` map) and Home rows.
//
// Names are display and context only. Identity stays the username
// everywhere it matters (sender, `[from …]`, assignees, routing).
//
// Display: "First Last" when set, else the username ("Owner" for the
// wire `owner`). Chip initials use the first and last initials. Tooltips
// read "First Last (username) · role".

/** The daemon's length cap (chars). */
export const PERSON_NAME_MAX = 64

export interface PersonNames {
  firstName?: string | null
  lastName?: string | null
  /** The daemon's computed name ("First Last", else the username). */
  displayName?: string | null
}

function clean(v: string | null | undefined): string {
  return typeof v === 'string' ? v.trim() : ''
}

/** "First Last" (either alone), or null when neither is set. */
export function realName(names: PersonNames | null | undefined): string | null {
  const full = [clean(names?.firstName), clean(names?.lastName)].filter(Boolean).join(' ')
  return full === '' ? null : full
}

/** The wire `"owner"` row is shown as "Owner". */
export function shownUsername(user: string): string {
  return user === 'owner' ? 'Owner' : user
}

/** What to call a person: their real name, else the daemon's
 *  displayName, else the username. */
export function personDisplayName(user: string, names?: PersonNames | null): string {
  return realName(names) ?? (clean(names?.displayName) || shownUsername(user))
}

/** Initials from the first and last name (`Rosson Long` → `RL`, first
 *  only → `R`), or null when no real name is set. */
export function nameInitials(names: PersonNames | null | undefined): string | null {
  const f = clean(names?.firstName)
  const l = clean(names?.lastName)
  if (!f && !l) return null
  return ((f ? Array.from(f)[0] : '') + (l ? Array.from(l)[0] : '')).toUpperCase()
}

/** "First Last (username) · role", or "username · role" with no name. */
export function personTooltip(user: string, role: string, names?: PersonNames | null, suffix = ''): string {
  const real = realName(names)
  const who = real ? `${real} (${shownUsername(user)})` : shownUsername(user)
  return `${who}${suffix} · ${role}`
}

function str(v: unknown): string | null {
  return typeof v === 'string' && v.trim() !== '' ? v : null
}

/** One wire names entry (`{firstName, lastName, displayName}`). */
export function parsePersonNames(raw: unknown): PersonNames {
  const o = raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
  return { firstName: str(o.firstName), lastName: str(o.lastName), displayName: str(o.displayName) }
}

/** A roster `names` payload as login → names (anything else → empty). */
export function asNameMap(raw: unknown): Record<string, PersonNames> {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {}
  const out: Record<string, PersonNames> = {}
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (v && typeof v === 'object') out[k] = parsePersonNames(v)
  }
  return out
}
