// Presence S2 — the top-bar roster: up to 10 slightly-overlapping
// PresenceAvatar chips + a `+N` overflow chip, sitting in the TopBar
// RIGHT group immediately left of the stopwatch (twin in FocusLayout).
//
// Hidden entirely when the daemon doesn't speak presence (older host —
// `supported` false) or when the roster is just you (≤1 user: showing
// yourself alone is noise). Clicking anywhere on it opens the read-only
// presence modal; the modal lives here so both mount points (TopBar +
// FocusLayout) get it for free.
//
// 0.43.2 Z15/Z34: with a remote Home room focused (Agents/Home bar only),
// it shows THAT server's people (`room.presence`), and its modal has no
// Kick: Kick acts on the window's server, and B's usernames are never sent
// to A's `presence/kick` (MS4).

import { useState } from 'react'
import { useStore } from 'zustand'
import { createStore } from 'zustand/vanilla'
import {
  usePresenceStore,
  rosterDisplay,
  shouldShowRoster,
} from '@/stores/presence'
import type { PresenceView, PresenceViewStore } from '@/stores/server-view'
import { useTopBarScope } from '@/components/TopBar/top-bar-scope'
import TopBarPipe from '@/components/TopBar/TopBarPipe'
import PresenceAvatar, { useSelfPresenceUser } from './PresenceAvatar'
import { PresenceMoreChip } from './PresenceChip'
import PresenceModal from './PresenceModal'

/** Read when no room is followed (a hook can't be skipped). */
const NO_ROOM_PRESENCE: PresenceViewStore = createStore<PresenceView>(() => ({ roster: [], supported: false }))

export default function PresenceRoster(): React.JSX.Element | null {
  const target = useTopBarScope()
  const windowRoster = usePresenceStore((s) => s.roster)
  const windowSupported = usePresenceStore((s) => s.supported)
  const roomPresence = useStore(target.room?.presence ?? NO_ROOM_PRESENCE)
  const [modalOpen, setModalOpen] = useState(false)
  const windowSelf = useSelfPresenceUser()

  const following = target.room !== null
  const roster = following ? roomPresence.roster : windowRoster
  const supported = following ? roomPresence.supported : windowSupported

  if (!shouldShowRoster(roster, supported)) return null

  const { visible, overflow } = rosterDisplay(roster)

  return (
    <>
      <button
        onClick={() => setModalOpen(true)}
        data-testid="presence-roster"
        className="flex h-6 items-center px-1 hover:bg-[var(--color-bg-elevated)] transition-colors"
        style={{
          // @ts-expect-error -- Electron-specific CSS property
          WebkitAppRegion: 'no-drag',
        }}
        title={
          following
            ? `${roster.length} connected on ${target.label} — click for details`
            : `${roster.length} connected — click for details`
        }
      >
        {/* plan-agent-status-unify-v1 §3.5: chamfered chips, 3px apart (no
            overlap); you get a ring (on the window's own server). */}
        <span className="flex items-center" style={{ gap: 3 }}>
          {visible.map((u) => (
            <PresenceAvatar key={u.user} name={u.user} role={u.role} size={20} you={!following && u.user === windowSelf} color={u.color} />
          ))}
          {overflow > 0 && <PresenceMoreChip count={overflow} size={20} title={`${overflow} more connected`} />}
        </span>
      </button>
      {/* Rosson 2026-10-17: a divider between the people and the usage
          chip. It lives here so it shows only while the roster does. */}
      <TopBarPipe />

      {modalOpen &&
        (following ? (
          <PresenceModal onClose={() => setModalOpen(false)} roster={roster} serverLabel={target.label} />
        ) : (
          <PresenceModal onClose={() => setModalOpen(false)} />
        ))}
    </>
  )
}
