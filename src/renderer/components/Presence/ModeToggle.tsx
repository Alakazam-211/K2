// Presence S5 — per-WINDOW viewer / claimer control (PRD §5.3).
// Compact two-icon selector (not page tabs): both modes stay visible;
// the selected one is filled. Eye = viewer, pencil = claimer.

import { useEffect } from 'react'
import {
  useWindowModeStore,
  initWindowModeDefault,
} from '@/stores/window-mode'
import { useTopBarScope } from '@/components/TopBar/top-bar-scope'

const noDrag = {
  WebkitAppRegion: 'no-drag',
} as React.CSSProperties

function EyeIcon(): React.JSX.Element {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M1 12s4-7 11-7 11 7 11 7-4 7-11 7-11-7-11-7z" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  )
}

function PencilIcon(): React.JSX.Element {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
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
      className={`flex h-6 w-6 items-center justify-center no-drag transition-colors ${
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

export default function ModeToggle(): React.JSX.Element {
  const windowMode = useWindowModeStore((s) => s.mode)
  const capable = useWindowModeStore((s) => s.capable)
  const setMode = useWindowModeStore((s) => s.setMode)
  const target = useTopBarScope()

  useEffect(() => {
    initWindowModeDefault()
  }, [])

  // 0.43.2 Z15: a focused remote room's mode comes from the room (view-only
  // → viewer; usable → claimer), not from this window's eye/pencil, which
  // never applied to a room. Shown, disabled. Same rule as the room's panes
  // (`paneRoomMode` in stores/room.ts; not imported here, so the top bar
  // does not load the tabs store).
  if (target.room) {
    const roomMode = target.room.readOnly ? 'viewer' : 'claimer'
    const setBy = `Set by ${target.label}`
    return (
      <div
        role="group"
        aria-label="Room input mode"
        data-mode-toggle={roomMode}
        data-mode-source="room"
        title={setBy}
        className="flex items-center no-drag border border-[var(--color-border)] rounded-none"
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

  const mode = windowMode
  const isViewer = mode === 'viewer'
  const claimerBlocked = !capable

  return (
    <div
      role="group"
      aria-label="Window input mode"
      data-mode-toggle={mode}
      className="flex items-center no-drag border border-[var(--color-border)] rounded-none"
      style={noDrag}
    >
      <ModeOption
        selected={isViewer}
        title="Viewer — watch only"
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
            : 'Claimer — type and resize'
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
