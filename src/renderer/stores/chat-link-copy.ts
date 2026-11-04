// "Show a copy button beside links on hover" (Settings → General, next to
// the font rows). Rosson 2026-11-04: the hover copy icon beside links in chat
// markdown (removed 10-31) comes back behind a switch, off by default.
//
// A client setting, not a server one (Rosson 2026-11-04): kept in this
// install's localStorage (`k2.chatLinkCopyButton`), like
// `k2.workspaceSwitchFocus`. No daemon key. A blocked or broken store reads
// as off.

import { create } from 'zustand'

export const LS_CHAT_LINK_COPY_BUTTON = 'k2.chatLinkCopyButton'

/** Only the stored string `'1'` means on; anything else (unset, garbage) is off. */
export function readChatLinkCopyButton(): boolean {
  try {
    if (typeof localStorage === 'undefined') return false
    return localStorage.getItem(LS_CHAT_LINK_COPY_BUTTON) === '1'
  } catch {
    return false
  }
}

export function writeChatLinkCopyButton(on: boolean): void {
  try {
    if (typeof localStorage === 'undefined') return
    if (on) localStorage.setItem(LS_CHAT_LINK_COPY_BUTTON, '1')
    else localStorage.removeItem(LS_CHAT_LINK_COPY_BUTTON)
  } catch {
    // Private mode / blocked storage: the in-memory value still holds this session.
  }
}

interface ChatLinkCopyState {
  /** Show the copy icon beside a link on hover. Off by default. */
  enabled: boolean
  setEnabled: (on: boolean) => void
}

export const useChatLinkCopyStore = create<ChatLinkCopyState>((set) => ({
  enabled: readChatLinkCopyButton(),
  setEnabled: (on) => {
    writeChatLinkCopyButton(on)
    set({ enabled: on })
  },
}))
