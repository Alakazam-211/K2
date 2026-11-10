// A one-shot signal so the setup checklist's phone row can open Settings →
// K2 Companion with the pairing block in view and focused, the
// `add-server-focus.ts` pattern. A monotonic counter: every click asks
// again. `handledSeq` remembers the last request the page acted on, so a
// later visit to the page (a remount) doesn't jump to pairing by itself.

import { create } from 'zustand'

interface PhonePairingFocusState {
  requestSeq: number
  handledSeq: number
  requestPhonePairingFocus: () => void
  /** True once per request: the page calls it when it acts. */
  takeRequest: () => boolean
}

export const usePhonePairingFocusStore = create<PhonePairingFocusState>((set, get) => ({
  requestSeq: 0,
  handledSeq: 0,
  requestPhonePairingFocus: () => set({ requestSeq: get().requestSeq + 1 }),
  takeRequest: () => {
    const { requestSeq, handledSeq } = get()
    if (requestSeq === handledSeq) return false
    set({ handledSeq: requestSeq })
    return true
  },
}))

/** The consuming page's selector. */
export function usePhonePairingFocus(): number {
  return usePhonePairingFocusStore((s) => s.requestSeq)
}
