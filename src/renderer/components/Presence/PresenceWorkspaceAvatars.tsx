// Presence on a nav row (plan-agent-status-unify-v1 §3.5, S8): who else is
// on this agent right now — never you (§2.5 #10).
//
// The chips sit on line 1 on the SAME column grid as line 2's status
// cluster: 14px chamfered chips, 4px gaps (`--clu-gap`), 2px before the
// last (`--num-gap`), right-aligned to the row's 12px inset. So the last
// chip sits over the number column, the next over the status, then the
// heartbeat and working columns. No overlap. Up to MAX_WORKSPACE_AVATARS
// chips, then a `+N` chip. The join from roster row to nav row is the
// store's `usersForWorkspace` (the daemon's prefix rule).

import { useMemo } from 'react'
import { presenceDisplayName, useSelfPresenceUser } from './PresenceAvatar'
import { PresenceChip, PresenceMoreChip } from './PresenceChip'
import {
  usePresenceStore,
  usersForWorkspace,
  MAX_WORKSPACE_AVATARS,
  type RosterUser,
} from '@/stores/presence'

/** A row chip's side (the status cluster's slot). */
export const ROW_CHIP_PX = 14

export default function PresenceWorkspaceAvatars({
  path,
}: {
  /** The nav row's resolved workspace path (worktreePath ?? project.path). */
  path: string
}): React.JSX.Element | null {
  const roster = usePresenceStore((s) => s.roster)
  const supported = usePresenceStore((s) => s.supported)
  const self = useSelfPresenceUser()

  const users = useMemo(() => usersForWorkspace(roster, path).filter((u) => u.user !== self), [roster, path, self])

  if (!supported) return null
  return <PresenceAvatarCluster users={users} />
}

/** The chip grid itself, for a list that does not come from the connected
 *  roster (a Home row on another server reads that server's summary, which
 *  already leaves you out). Renders nothing for an empty list. */
export function PresenceAvatarCluster({
  users,
}: {
  users: { user: string; role: RosterUser['role'] }[]
}): React.JSX.Element | null {
  if (users.length === 0) return null

  // A `+N` chip takes the last column when there are more than fit.
  const overflow = users.length > MAX_WORKSPACE_AVATARS + 1 ? users.length - MAX_WORKSPACE_AVATARS : 0
  const visible = overflow > 0 ? users.slice(0, MAX_WORKSPACE_AVATARS) : users
  const title = users.map((u) => presenceDisplayName(u.user)).join(', ')
  const count = visible.length + (overflow > 0 ? 1 : 0)
  const lastGap = { marginLeft: 'calc(var(--num-gap) - var(--clu-gap))' }

  return (
    <span className="flex flex-none items-center" style={{ gap: 'var(--clu-gap)' }} title={title} data-presence-grid="">
      {visible.map((u, i) => (
        <span key={u.user} className="flex flex-none" style={i === count - 1 ? lastGap : undefined}>
          <PresenceChip name={u.user} role={u.role} size={ROW_CHIP_PX} />
        </span>
      ))}
      {overflow > 0 && (
        <span className="flex flex-none" style={lastGap}>
          <PresenceMoreChip count={overflow} size={ROW_CHIP_PX} title={`${overflow} more here`} />
        </span>
      )}
    </span>
  )
}
