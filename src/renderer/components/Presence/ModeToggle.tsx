// Presence S5 — per-WINDOW viewer / claimer control (PRD §5.3).
// Compact two-icon selector (not page tabs): both modes stay visible;
// the selected one is filled. Eye = viewer, pencil = claimer.
//
// 0.46.11 (Rosson, thread-first): it lives in the terminal's own bar
// (Terminal view's session header), not the app's top bar, and shows only
// when it matters: someone else is on this session, or this window is on
// the eye (so nobody is stranded in view mode after the others leave).
// Alone on the pencil, there is no switch at all. Drop `<ModeToggle />`
// into any session bar: it reads the session from
// `ModeToggleSessionContext`, which the session chrome provides, and
// applies the rule itself.

import { createContext, useContext, useEffect } from 'react'
import { useStore } from 'zustand'
import {
  useWindowModeStore,
  initWindowModeDefault,
  type WindowMode,
} from '@/stores/window-mode'
import { usersForWorkspace } from '@/lib/presence-workspace'
import type { PresenceRosterUser as RosterUser } from '@/stores/session-events'
// Type-only: the room's rule is restated below (`roomModeOf`), so this
// control never loads the tabs store.
import type { Room } from '@/stores/room'
import { createStore } from 'zustand/vanilla'
import type { PresenceView, PresenceViewStore } from '@/stores/server-view'
import { useSelfPresenceUser } from './self-presence'

const noDrag = {
  WebkitAppRegion: 'no-drag',
} as React.CSSProperties

/** The session a bar's toggle belongs to: its workspace path (the roster's
 *  join key) and its room (whose server's roster, and a pinned room's own
 *  mode). Provided by the session chrome around its header. */
export interface ModeToggleSession {
  path: string | null
  room: Pick<Room, 'isPrimary' | 'readOnly' | 'presence' | 'scope'> | null
}

export const ModeToggleSessionContext = createContext<ModeToggleSession | null>(null)

/** The visibility rule (pure, unit-tested): show the eye/pencil when
 *  someone else is on this session, or when this window is on the eye.
 *  Alone on the pencil (or not resolved yet), hide it. */
export function shouldShowModeToggle({
  othersOnSession,
  mode,
}: {
  othersOnSession: boolean
  /** The mode in force; null while the window's default is unresolved. */
  mode: WindowMode | null
}): boolean {
  return othersOnSession || mode === 'viewer'
}

/** Is anyone but `self` on the workspace at `path`? The roster's
 *  per-workspace subscriptions, joined by the daemon's prefix rule (the
 *  same join the nav rows' avatars use). No path or no presence: no. */
export function othersOnSessionPath(
  roster: RosterUser[],
  path: string | null,
  self: string,
  supported: boolean,
): boolean {
  if (!supported || !path) return false
  return usersForWorkspace(roster, path).some((u) => u.user !== self)
}

function EyeIcon(): React.JSX.Element {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M1 12s4-7 11-7 11 7 11 7-4 7-11 7-11-7-11-7z" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  )
}

function PencilIcon(): React.JSX.Element {
  return (
    <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M17 3a2.828 2.828 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z" />
    </svg>
  )
}

function ModeOption({
  selected,
  disabled,
  title,
  onClick,
  children,
}: {
  selected: boolean
  disabled?: boolean
  title: string
  onClick: () => void
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-pressed={selected}
      title={title}
      className={`flex h-5 w-5 items-center justify-center no-drag transition-colors ${
        disabled
          ? 'text-[var(--color-text-muted)] opacity-50 cursor-not-allowed'
          : selected
            ? 'text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)]'
            : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.04]'
      }`}
      style={noDrag}
    >
      {children}
    </button>
  )
}

/** A pinned room's fixed mode, or null for the window's own room. The same
 *  rule as the room's panes (`paneRoomMode` in stores/room.ts). */
function roomModeOf(room: Pick<Room, 'isPrimary' | 'readOnly'> | null): WindowMode | null {
  if (!room || room.isPrimary) return null
  return room.readOnly ? 'viewer' : 'claimer'
}

const GROUP_CLASS = 'self-center flex flex-shrink-0 items-center no-drag border border-[var(--color-border)] rounded-none'

/** The eye/pencil pair itself, with no visibility rule: the window's mode,
 *  or (`room` set) a pinned room's fixed mode, shown disabled. */
export function ModeToggleControl({
  room = null,
}: {
  room?: Pick<Room, 'isPrimary' | 'readOnly' | 'scope'> | null
}): React.JSX.Element {
  const windowMode = useWindowModeStore((s) => s.mode)
  const capable = useWindowModeStore((s) => s.capable)
  const setMode = useWindowModeStore((s) => s.setMode)

  // 0.43.2 Z15: a remote room's mode comes from the room (view-only →
  // viewer; usable → claimer), not from this window's eye/pencil, which
  // never applied to a room. Shown, disabled.
  const roomMode = roomModeOf(room)
  if (room && roomMode) {
    const setBy = `Set by ${room.scope.label}`
    return (
      <div
        role="group"
        aria-label="Room input mode"
        data-mode-toggle={roomMode}
        data-mode-source="room"
        title={setBy}
        className={GROUP_CLASS}
        style={noDrag}
      >
        <ModeOption selected={roomMode === 'viewer'} disabled title={setBy} onClick={() => {}}>
          <EyeIcon />
        </ModeOption>
        <ModeOption selected={roomMode === 'claimer'} disabled title={setBy} onClick={() => {}}>
          <PencilIcon />
        </ModeOption>
      </div>
    )
  }

  const isViewer = windowMode === 'viewer'
  const claimerBlocked = !capable

  return (
    <div
      role="group"
      aria-label="Window input mode"
      data-mode-toggle={windowMode}
      className={GROUP_CLASS}
      style={noDrag}
    >
      <ModeOption
        selected={isViewer}
        title="View only — watch, don't type"
        onClick={() => setMode('viewer')}
      >
        <EyeIcon />
      </ModeOption>
      <ModeOption
        selected={!isViewer}
        disabled={claimerBlocked}
        title={
          claimerBlocked
            ? 'View-only — ask the owner for edit access'
            : 'Edit — type and resize'
        }
        onClick={() => {
          if (claimerBlocked) return
          setMode('claimer')
        }}
      >
        <PencilIcon />
      </ModeOption>
    </div>
  )
}

const NO_SESSION_PRESENCE: PresenceViewStore = createStore<PresenceView>(() => ({ roster: [], supported: false }))

/** The eye/pencil for a session's bar, under the visibility rule. Reads
 *  its session from `ModeToggleSessionContext`; with none, it shows only
 *  while the window is on the eye. */
export default function ModeToggle(): React.JSX.Element | null {
  const session = useContext(ModeToggleSessionContext)
  const room = session?.room ?? null
  const windowMode = useWindowModeStore((s) => (s.resolved ? s.mode : null))
  const self = useSelfPresenceUser()
  // The session's server's roster: a pinned room reads its own server's.
  // No session: nobody to count.
  const presence = room?.presence ?? NO_SESSION_PRESENCE
  const roster = useStore(presence, (s) => s.roster)
  const supported = useStore(presence, (s) => s.supported)

  useEffect(() => {
    initWindowModeDefault()
  }, [])

  const roomMode = roomModeOf(room)
  // A remote room's mode is fixed by that room: it shows only when it is
  // the eye, to say why typing does nothing.
  const shown =
    roomMode !== null
      ? roomMode === 'viewer'
      : shouldShowModeToggle({
          othersOnSession: othersOnSessionPath(roster, session?.path ?? null, self, supported),
          mode: windowMode,
        })
  if (!shown) return null
  return <ModeToggleControl room={room} />
}
