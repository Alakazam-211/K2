// Presence S6 — workspace-nav mini avatar cluster (PRD §5.4).
//
// Sits where the git ± diff counts used to live on the sidebar's project
// and worktree rows: up to MAX_WORKSPACE_AVATARS overlapping 14px
// `PresenceAvatar`s + a `+N` chip for the rest. The join from roster row
// to nav row is the store's `usersForWorkspace` selector — the daemon's
// `event_matches_workspace` prefix rule applied symmetrically (see
// `stores/presence.ts`). Renders nothing when the daemon doesn't speak
// presence or nobody is here — the row simply has no cluster.

import { useMemo } from 'react'
import PresenceAvatar, { presenceDisplayName } from './PresenceAvatar'
import {
  usePresenceStore,
  usersForWorkspace,
  rosterDisplay,
  MAX_WORKSPACE_AVATARS,
} from '@/stores/presence'

export default function PresenceWorkspaceAvatars({
  path,
}: {
  /** The nav row's resolved workspace path (worktreePath ?? project.path). */
  path: string
}): React.JSX.Element | null {
  const roster = usePresenceStore((s) => s.roster)
  const supported = usePresenceStore((s) => s.supported)

  const users = useMemo(() => usersForWorkspace(roster, path), [roster, path])

  if (!supported || users.length === 0) return null

  const { visible, overflow } = rosterDisplay(users, MAX_WORKSPACE_AVATARS)
  const title = users.map((u) => presenceDisplayName(u.user)).join(', ')

  return (
    <span className="flex items-center flex-shrink-0" title={title}>
      {visible.map((u, i) => (
        <span
          key={u.user}
          className="flex flex-shrink-0"
          // Overlap the stack −3px; the daemon's owner-first ordering is
          // preserved, so later (higher z) chips overlap earlier ones.
          style={{ marginLeft: i === 0 ? 0 : -3, zIndex: i + 1, position: 'relative' }}
        >
          <PresenceAvatar name={u.user} role={u.role} size={14} />
        </span>
      ))}
      {overflow > 0 && (
        <span className="ml-0.5 text-[9px] tabular-nums font-medium text-[var(--color-text-muted)] flex-shrink-0">
          +{overflow}
        </span>
      )}
    </span>
  )
}
