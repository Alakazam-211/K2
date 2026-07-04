// Presence S5 — the per-WINDOW viewer/claimer toggle (PRD §5.3).
// Rendered as the LAST control in the top bar's RIGHT group (TopBar +
// its FocusLayout twin). Own file by design, matching the sibling
// presence controls (PresenceKickButton et al.).
//
// Eye = viewer (watch only), pencil = claimer (drive the terminal).
// Clicking flips this window's mode in the window-mode store; every
// TerminalPane mirrors the store to its grid-WS via `set_mode`. The
// daemon enforces — this control is the honest UI for it:
//
//   - ALWAYS visible, even for a solo owner (deliberate: an owner may
//     want viewer mode for demos / hands-off watching);
//   - when the daemon reports `capable:false` (ungranted viewer-role
//     user), the claimer flip is DISABLED with an ask-the-owner tooltip
//     — the mode ACKs keep `capable` current, so a live grant enables
//     it without a reload.

import { useEffect } from 'react'
import {
  useWindowModeStore,
  initWindowModeDefault,
} from '@/stores/window-mode'

export default function ModeToggle(): React.JSX.Element {
  const mode = useWindowModeStore((s) => s.mode)
  const capable = useWindowModeStore((s) => s.capable)
  const setMode = useWindowModeStore((s) => s.setMode)

  // Derive the window default (owner → claimer, else viewer) once.
  useEffect(() => {
    initWindowModeDefault()
  }, [])

  const isViewer = mode === 'viewer'
  // The only blocked transition is viewer → claimer while the daemon
  // says we're not capable. Flipping back to viewer is always allowed.
  const blocked = isViewer && !capable

  const title = blocked
    ? 'View-only — ask the owner for edit access'
    : isViewer
      ? 'Viewer mode — click to switch to claimer (type + resize)'
      : 'Claimer mode — click to switch to viewer (watch only)'

  return (
    <button
      onClick={() => {
        if (blocked) return
        setMode(isViewer ? 'claimer' : 'viewer')
      }}
      disabled={blocked}
      aria-label={title}
      data-mode-toggle={mode}
      className={`flex h-6 w-6 items-center justify-center transition-colors ${
        blocked
          ? 'text-[var(--color-text-muted)] opacity-50 cursor-not-allowed'
          : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]'
      }`}
      style={{
        // @ts-expect-error -- Electron-specific CSS property
        WebkitAppRegion: 'no-drag'
      }}
      title={title}
    >
      {isViewer ? (
        // Eye — watching.
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <path d="M1 12s4-7 11-7 11 7 11 7-4 7-11 7-11-7-11-7z" />
          <circle cx="12" cy="12" r="3" />
        </svg>
      ) : (
        // Pencil — driving.
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <path d="M17 3a2.828 2.828 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z" />
        </svg>
      )}
    </button>
  )
}
