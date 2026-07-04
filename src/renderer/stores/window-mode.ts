// Presence S5 — per-WINDOW viewer/claimer mode (PRD §5.3).
//
// One mode per Tauri window (not per pane, not per user): the top-right
// ModeToggle flips it, and every TerminalPane in the window mirrors it
// to its grid-WS with `{action:"set_mode", mode}`. The daemon is the
// AUTHORITY — it gates input/resize/claims per frame against mode AND
// capability; everything here is advisory UX:
//
//   - client-side suppression (viewer mode sends no input/resize/claim,
//     keeping the wire quiet and the UI honest), and
//   - the `capable` mirror of the daemon's mode-ACK frames
//     (`{"event":"mode","payload":{mode,capable}}`), which the toggle
//     uses to disable the claimer option for ungranted viewer-role users.
//
// DEFAULT (derived ONCE from whoami, the PresenceKickButton cached
// approach): owner → claimer (local windows keep today's behavior),
// anything else → viewer. Until whoami resolves the store stays
// UNRESOLVED and nothing is suppressed or sent — the daemon's own
// defaults (owner connections claimer, everyone else viewer) already
// enforce the right thing, so a transient whoami failure can never
// lock the local owner out of their own terminals.

import { create } from 'zustand'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { onActiveHostChange } from '@/stores/connect-host'

export type WindowMode = 'viewer' | 'claimer'

/** Pure default rule (unit-tested): owner → claimer, any other resolved
 *  role → viewer. `null` (whoami failed / pre-role daemon) → no default;
 *  the store stays unresolved and the daemon defaults rule. */
export function deriveDefaultMode(role: string | null): WindowMode | null {
  if (role === null) return null
  return role === 'owner' ? 'claimer' : 'viewer'
}

interface WindowModeState {
  /** This window's requested mode. Meaningful only once `resolved`. */
  mode: WindowMode
  /** True once the default derived from whoami OR the user toggled
   *  manually. While false, panes neither suppress nor send set_mode. */
  resolved: boolean
  /** Daemon-reported claimer capability, mirrored from the latest
   *  grid-WS mode ACK. Optimistic `true` until a frame says otherwise. */
  capable: boolean
  /** Manual toggle (ModeToggle) — always wins over a later default. */
  setMode: (mode: WindowMode) => void
  /** Mirror a daemon mode-ACK's `capable` flag. */
  setCapable: (capable: boolean) => void
}

export const useWindowModeStore = create<WindowModeState>((set) => ({
  mode: 'viewer',
  resolved: false,
  capable: true,
  setMode: (mode) => set({ mode, resolved: true }),
  setCapable: (capable) => set({ capable }),
}))

/** True when panes should client-side-suppress input/resize/claims:
 *  the mode is RESOLVED viewer. Unresolved never suppresses (see the
 *  header — the daemon defaults already enforce correctness). */
export function isViewerModeActive(): boolean {
  const s = useWindowModeStore.getState()
  return s.resolved && s.mode === 'viewer'
}

// ── Default resolution (single-flight) ────────────────────────────────
let defaultStarted = false

/** Resolve the window's default mode from whoami, once. Manual toggles
 *  that landed first are never clobbered (`resolved` check on apply).
 *  Idempotent — every mount point may call it. */
export function initWindowModeDefault(): void {
  if (defaultStarted) return
  defaultStarted = true
  void fetchViewerRole().then((role) => {
    const derived = deriveDefaultMode(role)
    if (derived === null) {
      // No identity resolved — allow a later mount to retry.
      defaultStarted = false
      return
    }
    const s = useWindowModeStore.getState()
    if (!s.resolved) {
      useWindowModeStore.setState({ mode: derived, resolved: true })
    }
  })
}

function reset(): void {
  defaultStarted = false
  useWindowModeStore.setState({ mode: 'viewer', resolved: false, capable: true })
}

/** Test-only: reset module + store state between vitest cases. */
export function __resetWindowModeForTests(): void {
  reset()
}

// Per-daemon identity — a host switch re-derives the default against
// the NEW host (the PresenceKickButton whoami cache resets on the same
// seam, so the re-fetch hits the new daemon).
onActiveHostChange(() => {
  reset()
  initWindowModeDefault()
})
