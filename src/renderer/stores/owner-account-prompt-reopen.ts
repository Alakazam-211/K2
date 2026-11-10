// "Show it anyway" for the owner account prompt (setup checklist row
// `own_sign_in`). The daemon hides the prompt once "Not now" was chosen
// for this version; this client-side flag lets the person reopen it from
// the checklist while no account is linked. The prompt clears the flag when
// it closes, or when the daemon says there's nothing to reopen.

import { create } from 'zustand'

interface OwnerAccountPromptReopenState {
  forced: boolean
  reopen: () => void
  clear: () => void
}

export const useOwnerAccountPromptReopenStore = create<OwnerAccountPromptReopenState>((set) => ({
  forced: false,
  reopen: () => set({ forced: true }),
  clear: () => set({ forced: false }),
}))
