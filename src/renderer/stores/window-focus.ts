// 0.37.11 A9 Phase 4c — active-viewer resize gating.
//
// Each Tauri window tracks whether IT currently has OS focus.
// `TerminalPane.sendResize` reads `isFocused` and refuses to issue
// WS `{action:"resize", cols, rows}` when the window is blurred —
// the focused window is the sole authoritative resizer for any PTY
// that multiple windows happen to be viewing.
//
// Wiring (one-time, in App.tsx):
//   - hydrate from `getCurrentWindow().isFocused()` once at mount
//   - subscribe to `tauri://focus` / `tauri://blur` events on the
//     current window and call `setFocused`
//
// TerminalPane subscribes to the store, gates `sendResize` on
// `isFocused`, and re-sends the most recent grid dimensions when
// focus transitions from `false → true` so the PTY snaps to this
// window's current pane size.
//
// Why a store (not just listen in every pane): one source of truth,
// fewer subscriptions, and we get a focus-transition signal for free
// via Zustand's `subscribe`.

import { create } from 'zustand'

interface WindowFocusState {
  /** True iff this Tauri window currently has OS focus. */
  isFocused: boolean
  setFocused: (focused: boolean) => void
}

export const useWindowFocusStore = create<WindowFocusState>((set) => ({
  // Default to true so single-window installations behave the same
  // as before — the listener will flip it to whatever Tauri reports
  // on first paint. Avoids a brief "no resizes possible" state.
  isFocused: true,
  setFocused: (focused) => set({ isFocused: focused }),
}))
