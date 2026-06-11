// Session labels store (0.37.4 Phase B / PRD `session-label-daemon-owned.md`)
//
// The daemon owns the authoritative label for every v2 session it
// hosts. The renderer is a thin mirror: when a TerminalPane's WS
// receives a `label_initial` or `label_changed` event from the
// daemon, it calls `setSessionLabel(sessionId, label)`. UI surfaces
// (tab bar, agent panes, anywhere a session label is shown) read
// via `useSessionLabel(sessionId)`.
//
// Why a separate store instead of stuffing the label into `tabs.ts`:
// - Multiple tabs / surfaces can subscribe to the same sessionId
//   (e.g. an active-agents pane listing the same session as the
//   tab bar). A separate keyed store avoids tab-state coupling.
// - The daemon's label outlives any specific Tauri window or tab
//   instance. Storing it under sessionId — not tabId — matches
//   that ownership model.
// - PTY title events that DO drive activity tracking (the
//   spinner-vs-idle hint) keep flowing through the existing
//   `useActiveAgentsStore`. Labels and activity are orthogonal.

import { create } from 'zustand'

interface SessionLabelsState {
  /** sessionId → daemon-authoritative label. Empty string means
   *  the daemon told us "no preference" (PTY hasn't set anything
   *  yet); UI consumers fall back to whatever default makes sense
   *  for the surface. */
  labels: Record<string, string>

  /** Update a session's label (called from the WS handler on
   *  `label_initial` / `label_changed`). Idempotent — same value
   *  is a no-op. */
  setSessionLabel: (sessionId: string, label: string) => void

  /** Drop a session's entry. Called when the WS closes due to
   *  `child_exit` or the pane unmounts after session death. Keeps
   *  the store from growing unbounded across long sessions. */
  forgetSession: (sessionId: string) => void
}

export const useSessionLabelsStore = create<SessionLabelsState>((set) => ({
  labels: {},
  setSessionLabel: (sessionId, label) =>
    set((state) => {
      if (state.labels[sessionId] === label) return state
      return { labels: { ...state.labels, [sessionId]: label } }
    }),
  forgetSession: (sessionId) =>
    set((state) => {
      if (!(sessionId in state.labels)) return state
      const { [sessionId]: _drop, ...rest } = state.labels
      return { labels: rest }
    }),
}))

/** Hook for components that want to display a session's label.
 *  Returns the daemon-owned label or `undefined` if no label has
 *  been received yet (component should render a fallback like the
 *  agent display name or a placeholder). */
export function useSessionLabel(sessionId: string | null | undefined): string | undefined {
  return useSessionLabelsStore((s) => (sessionId ? s.labels[sessionId] : undefined))
}

/** Imperative access for code outside React (e.g. tab serialization
 *  on workspace switch). */
export function getSessionLabel(sessionId: string): string | undefined {
  return useSessionLabelsStore.getState().labels[sessionId]
}
