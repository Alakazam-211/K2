// "Me" for the Tickets board's Mine / Waiting on me filters: the current
// login on this window's server, from `GET /cli/auth/whoami`. The daemon
// sends the names as `ticketMe` (owner + display name + owner-linked
// Connect accounts), the same list its `waiting-count?me=1` badge counts;
// an older daemon without it falls back to owner + display name / the
// username (`myAssigneeNames`). Resolved once per board mount.

import { useEffect, useMemo, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { useSettingsStore } from '@/stores/settings'
import { myAssigneeNames } from './feedback-api'

export interface TicketWhoami {
  owner?: boolean
  username?: string | null
  /** The daemon's "me" assignee names (lower-cased); absent on older daemons. */
  ticketMe?: string[]
}

export function fetchTicketWhoami(): Promise<TicketWhoami> {
  return daemonCliGet<TicketWhoami>(primaryScope(), 'auth/whoami')
}

/** `null` while whoami is loading. A failed whoami resolves to the owner
 *  on this computer's daemon and to nobody on a remote one (the filters
 *  then show nothing for Mine; All still shows everything). */
export function useTicketMe(): string[] | null {
  const ownerDisplayName = useSettingsStore((s) => s.ownerDisplayName)
  const [whoami, setWhoami] = useState<TicketWhoami | null | undefined>(undefined)

  useEffect(() => {
    let cancelled = false
    fetchTicketWhoami().then(
      (w) => {
        if (!cancelled) setWhoami(w)
      },
      (e: unknown) => {
        console.warn('[tickets] whoami failed; Mine falls back', e)
        if (!cancelled) setWhoami(primaryScope().isRemote ? null : { owner: true })
      },
    )
    return () => {
      cancelled = true
    }
  }, [])

  return useMemo(() => {
    if (whoami === undefined) return null
    return myAssigneeNames(whoami, ownerDisplayName ?? '')
  }, [whoami, ownerDisplayName])
}
