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
import PresenceAvatar from './PresenceAvatar'
import PresenceModal from './PresenceModal'

/** Read when no room is followed (a hook can't be skipped). */
const NO_ROOM_PRESENCE: PresenceViewStore = createStore<PresenceView>(() => ({ roster: [], supported: false }))

export default function PresenceRoster(): React.JSX.Element | null {
  const target = useTopBarScope()
  const windowRoster = usePresenceStore((s) => s.roster)
  const windowSupported = usePresenceStore((s) => s.supported)
  const roomPresence = useStore(target.room?.presence ?? NO_ROOM_PRESENCE)
  const [modalOpen, setModalOpen] = useState(false)

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
        {/* Slightly overlapping stack, matching top-bar visual density. */}
        <span className="flex items-center">
          {visible.map((u, i) => (
            <span
              key={u.user}
              style={{ marginLeft: i === 0 ? 0 : -4, zIndex: visible.length - i }}
              className="relative"
            >
              <PresenceAvatar name={u.user} role={u.role} size={20} />
            </span>
          ))}
          {overflow > 0 && (
            <span
              className="relative flex items-center justify-center flex-shrink-0 text-[9px] font-bold text-[var(--color-text-secondary)]"
              style={{
                marginLeft: -4,
                width: 20,
                height: 20,
                borderRadius: '50%',
                border: '2px solid var(--color-border)',
                backgroundColor: 'var(--color-bg-elevated)',
                boxSizing: 'border-box',
              }}
              title={`${overflow} more connected`}
            >
              +{overflow}
            </span>
          )}
        </span>
      </button>

      {modalOpen &&
        (following ? (
          <PresenceModal onClose={() => setModalOpen(false)} roster={roster} serverLabel={target.label} />
        ) : (
          <PresenceModal onClose={() => setModalOpen(false)} />
        ))}
    </>
  )
}
