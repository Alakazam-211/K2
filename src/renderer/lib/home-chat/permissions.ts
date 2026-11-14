// What a person may do in a Home channel (prd-home-people-channels-v1 §13).
// The daemon checks every one of these (HP-19); the client only hides what
// the caller's role can't do, so nobody is offered a button that refuses.
//
// | Action            | Owner / Admin        | Member, Voice & Chat only |
// | Add people        | as a member          | as a member               |
// | Remove others     | yes (invite-only)    | no                        |
// | Leave             | yes                  | yes                       |
// | Rename / delete   | any channel they see | their own                 |
//
// Voice & Chat only logins chat like Members (they reach `/cli/home-chat/*`
// through the reviewed allowlist); agents never call these routes.

import type { HomeChatPeople } from './api'

/** Is the caller an Owner or Admin on that server? The owner's own login
 *  is `k2:owner`; the people list carries roles only for an Owner or Admin. */
export function callerIsAdmin(people: HomeChatPeople | null | undefined): boolean {
  if (!people) return false
  if (people.me.ext === 'k2:owner') return true
  return people.people.some((p) => p.role !== null)
}

export interface ChannelFacts {
  callerExt: string
  admin: boolean
  creator: string
  /** The caller is in the channel. */
  member: boolean
  open: boolean
  archived: boolean
}

export interface ChannelPerms {
  addPeople: boolean
  removeOthers: boolean
  leave: boolean
  rename: boolean
  remove: boolean
}

export function channelPerms(f: ChannelFacts): ChannelPerms {
  const live = !f.archived
  const own = f.creator !== '' && f.creator === f.callerExt
  return {
    addPeople: live && f.member,
    // An open channel is open to everyone: no one is removed from it.
    removeOthers: live && f.admin && !f.open,
    leave: f.member || f.open,
    rename: live && (own || f.admin),
    remove: live && (own || f.admin),
  }
}
