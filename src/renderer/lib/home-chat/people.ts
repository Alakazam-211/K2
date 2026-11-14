// People on Home across servers (prd-home-people-channels-v1 §6.3, D3):
// K2 logins are per server and carry no global id, so K2 never decides on
// its own that two are one person. It only offers: the picker asks "Same
// person as Alex on box-a?", a row's chip shows when a match is on another
// server, and its menu lists "Also on" those servers.
//
// A match: the same username, or the same full name. The owner's username
// is K2's own word (`owner`) on every server, so the owner matches by name.
// Pure.

import type { HomeChatPerson } from './api'

function norm(s: string | null | undefined): string {
  return (s ?? '').trim().replace(/\s+/g, ' ').toLocaleLowerCase()
}

/** Might `a` and `b` (on two servers) be one person? */
export function mightBeSamePerson(a: Pick<HomeChatPerson, 'ext' | 'username' | 'displayName'>, b: Pick<HomeChatPerson, 'ext' | 'username' | 'displayName'>): boolean {
  const ownerA = a.ext === 'k2:owner'
  const ownerB = b.ext === 'k2:owner'
  if (!ownerA && !ownerB && norm(a.username) && norm(a.username) === norm(b.username)) return true
  const na = norm(a.displayName)
  const nb = norm(b.displayName)
  // A name that is just the username says nothing more than the username.
  const realA = na && na !== norm(a.username)
  const realB = nb && nb !== norm(b.username)
  return !!(realA && realB && na === nb)
}

/** Every saved server (other than `hostKey`) whose people list has a
 *  possible match for `person`: `[hostKey, theirPerson]`. */
export function matchesElsewhere(
  person: Pick<HomeChatPerson, 'ext' | 'username' | 'displayName'>,
  hostKey: string,
  peopleByHost: Readonly<Record<string, readonly HomeChatPerson[] | undefined>>,
): Array<{ hostKey: string; person: HomeChatPerson }> {
  const out: Array<{ hostKey: string; person: HomeChatPerson }> = []
  for (const [h, list] of Object.entries(peopleByHost)) {
    if (h === hostKey || !list) continue
    for (const p of list) if (mightBeSamePerson(person, p)) out.push({ hostKey: h, person: p })
  }
  return out
}

/** Who may be picked in the add-person picker (§6.1): a person whose chat is
 *  set up. The daemon lists only people (never agents, App users, disabled
 *  logins or the caller); this filter is the client's belt on top. */
export function pickablePerson(p: HomeChatPerson): boolean {
  return p.ready && /^(k2:owner|k2:connect:[a-z0-9-]{1,64})$/.test(p.ext.toLowerCase())
}
