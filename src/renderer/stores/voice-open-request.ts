// Voice & Chat only gate screen (prd-voice-chat-only-members-v1 §8.3):
// "Open voice rooms" switches the window to a server it can open and asks
// the headset to open its list once it mounts. The gate can't import the
// voice store (it lives in the lazily loaded App chunk), so it leaves the
// ask here and the headset takes it.

import { create } from 'zustand'

interface VoiceOpenRequest {
  pending: boolean
  request: () => void
  /** True when there was an ask; it is cleared. */
  take: () => boolean
}

export const useVoiceOpenRequest = create<VoiceOpenRequest>((set, get) => ({
  pending: false,
  request: () => set({ pending: true }),
  take: () => {
    if (!get().pending) return false
    set({ pending: false })
    return true
  },
}))
