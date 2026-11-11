// Presence S5 — per-WINDOW viewer / claimer control (PRD §5.3).
// Compact two-icon selector (not page tabs): both modes stay visible;
// the selected one is filled. Eye = viewer, pencil = claimer.
//
// 0.46.11 (Rosson, thread-first): it lives in the terminal's own bar
// (Terminal view's session header), not the app's top bar, and shows at
// all times, alone or not, once the window's mode has resolved. A pinned
// remote room shows its fixed mode, disabled. Drop `<ModeToggle />` into
// any session bar: it reads its room from `ModeToggleSessionContext`,
// which the session chrome provides.

import { createContext, useContext, useEffect } from 'react'
import {
  useWindowModeStore,
  initWindowModeDefault,
  type WindowMode,
} from '@/stores/window-mode'
// Type-only: the room's rule is restated below (`roomModeOf`), so this
// control never loads the tabs store.
import type { Room } from '@/stores/room'

const noDrag = {
  WebkitAppRegion: 'no-drag',
} as React.CSSProperties

/** The session a bar's toggle belongs to: its room (a pinned room has its
 *  own fixed mode). Provided by the session chrome around its header. */
export interface ModeToggleSession {
  room: Pick<Room, 'isPrimary' | 'readOnly' | 'scope'> | null
}

export const ModeToggleSessionContext = createContext<ModeToggleSession | null>(null)

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

/** The eye/pencil for a session's bar, shown at all times. Reads its room
 *  from `ModeToggleSessionContext` (none: the window's own mode). Hidden
 *  only while the window's mode is unresolved, so it doesn't flash. */
export default function ModeToggle(): React.JSX.Element | null {
  const session = useContext(ModeToggleSessionContext)
  const room = session?.room ?? null
  const resolved = useWindowModeStore((s) => s.resolved)

  useEffect(() => {
    initWindowModeDefault()
  }, [])

  // A pinned remote room's mode is fixed by that room: always shown,
  // disabled, eye or pencil, to say why typing does or doesn't work.
  if (roomModeOf(room) === null && !resolved) return null
  return <ModeToggleControl room={room} />
}
