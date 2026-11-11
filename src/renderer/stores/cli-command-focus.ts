// A one-shot signal so the setup checklist's k2 command row can open
// Settings → General with the k2 command row in view and focused, the
// `phone-pairing-focus.ts` pattern. A monotonic counter: every click asks
// again. `handledSeq` remembers the last request the page acted on, so a
// later visit to the page (a remount) doesn't jump to the row by itself.

import { create } from 'zustand'

interface CliCommandFocusState {
  requestSeq: number
  handledSeq: number
  requestCliCommandFocus: () => void
  /** True once per request: the page calls it when it acts. */
  takeRequest: () => boolean
}

export const useCliCommandFocusStore = create<CliCommandFocusState>((set, get) => ({
  requestSeq: 0,
  handledSeq: 0,
  requestCliCommandFocus: () => set({ requestSeq: get().requestSeq + 1 }),
  takeRequest: () => {
    const { requestSeq, handledSeq } = get()
    if (requestSeq === handledSeq) return false
    set({ handledSeq: requestSeq })
    return true
  },
}))

/** The consuming page's selector. */
export function useCliCommandFocus(): number {
  return useCliCommandFocusStore((s) => s.requestSeq)
}
