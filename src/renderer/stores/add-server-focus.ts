// add-server-focus — a tiny one-shot signal so the top-bar "Add a
// server…" action can ask the (already-open) Settings → Connections
// "Add a Server" form to reveal itself, scroll into view, and focus its
// first input.
//
// This lives in the connect domain (NOT the settings store, which is
// owned elsewhere) so the cross-component "open Settings at the right
// place, cursor ready" wiring stays self-contained: ServerSwitcher bumps
// `requestSeq`; ConnectionsSection subscribes and, on each bump, opens
// the add form + focuses + scrolls. A monotonic counter (rather than a
// boolean) means repeated clicks re-fire even without an intervening
// reset, and there's nothing to clear.

import { create } from 'zustand'

interface AddServerFocusState {
  /** Monotonic counter; each increment is one "reveal + focus" request. */
  requestSeq: number
  /** Top-bar (or anywhere) → ask the Connections add-server form to focus. */
  requestAddServerFocus: () => void
}

export const useAddServerFocusStore = create<AddServerFocusState>((set, get) => ({
  requestSeq: 0,
  requestAddServerFocus: () => set({ requestSeq: get().requestSeq + 1 }),
}))

/** Convenience selector hook for the consuming form. */
export function useAddServerFocus(): number {
  return useAddServerFocusStore((s) => s.requestSeq)
}
