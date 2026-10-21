// "Light blur" in Gardens (0.46.3), the desktop twin of K2 Companion's
// Diagnostics → Light blur (`lightBlur.ts`). The Glass theme's floating
// conversation draws a progressive blur at its top bar and behind its
// message box: four live backdrop-filter layers each, so 8 per floating
// conversation (several conversations on one page multiply that). With Light
// blur on, each stack draws two layers (4px and 16px, the phone's choice):
// 4 per conversation, and much less blur radius for the GPU to cover.
//
// Off by default: today's look stays the default. A setting of the Garden on
// this computer only (Garden settings → Appearance), never sent to a server:
// `'1'` in localStorage `k2.zen.lightBlur`. Every read and write is in
// try/catch; a failure keeps the choice for this window.

import { create } from 'zustand'

export const ZEN_LIGHT_BLUR_KEY = 'k2.zen.lightBlur'
/** Today's four-layer look stays the default. */
export const ZEN_LIGHT_BLUR_DEFAULT = false

export const ZEN_LIGHT_BLUR_LABEL = 'Light blur'
export const ZEN_LIGHT_BLUR_NOTE =
  'Draws the blur behind the top bar and the message box with two layers instead of four. Lighter on slower graphics; the look is a little less smooth. This computer only.'

export function readZenLightBlurStored(): boolean {
  try {
    return localStorage.getItem(ZEN_LIGHT_BLUR_KEY) === '1'
  } catch {
    return ZEN_LIGHT_BLUR_DEFAULT
  }
}

interface ZenLightBlurState {
  on: boolean
  setOn(on: boolean): void
  /** Re-read storage (tests). */
  reload(): void
}

export const useZenLightBlurStore = create<ZenLightBlurState>((set) => ({
  on: readZenLightBlurStored(),
  setOn: (on) => {
    try {
      if (on) localStorage.setItem(ZEN_LIGHT_BLUR_KEY, '1')
      else localStorage.removeItem(ZEN_LIGHT_BLUR_KEY)
    } catch {
      /* the choice lasts this window */
    }
    set({ on })
  },
  reload: () => set({ on: readZenLightBlurStored() }),
}))
