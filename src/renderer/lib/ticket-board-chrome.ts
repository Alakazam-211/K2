// Tickets board view state, per WINDOW: the ticket list's width, whether
// it is folded into the avatar rail, and whether the right-hand agent chat
// rail is open. Thin-client view preference only,
// never daemon state — same storage rule as `window-chrome.ts`
// (`k2.windowChrome.<label>`, prd-per-window-chrome-v1): localStorage in
// the desktop app, sessionStorage on the hosted web client, keyed by the
// Tauri window label so two windows keep their own layout.

import { getWindowLabel } from '@/lib/window-chrome'
import { isWebClient } from '@/lib/is-web'

export const TICKET_BOARD_KEY_PREFIX = 'k2.windowChrome.tickets.'

/** Default list width (Rosson: 300 was too narrow; 100 px wider). A width
 *  the user dragged and saved for this window still wins. */
export const TICKET_LIST_DEFAULT_WIDTH = 400
export const TICKET_LIST_MIN_WIDTH = 220
export const TICKET_LIST_MAX_WIDTH = 560

export type TicketBoardChrome = {
  listWidth: number
  collapsed: boolean
  /** The "Chat with agent" rail. Open by default (Rosson). */
  chatOpen: boolean
}

export const DEFAULT_TICKET_BOARD_CHROME: TicketBoardChrome = {
  listWidth: TICKET_LIST_DEFAULT_WIDTH,
  collapsed: false,
  chatOpen: true,
}

export function ticketBoardKey(label: string): string {
  return `${TICKET_BOARD_KEY_PREFIX}${label}`
}

export function clampTicketListWidth(width: number): number {
  if (!Number.isFinite(width)) return TICKET_LIST_DEFAULT_WIDTH
  return Math.round(Math.min(TICKET_LIST_MAX_WIDTH, Math.max(TICKET_LIST_MIN_WIDTH, width)))
}

function storage(): Storage | null {
  try {
    if (isWebClient()) return typeof sessionStorage === 'undefined' ? null : sessionStorage
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

export function readTicketBoardChrome(label = getWindowLabel()): TicketBoardChrome {
  const s = storage()
  if (!s) return { ...DEFAULT_TICKET_BOARD_CHROME }
  let raw: string | null = null
  try {
    raw = s.getItem(ticketBoardKey(label))
  } catch {
    return { ...DEFAULT_TICKET_BOARD_CHROME }
  }
  if (raw == null) return { ...DEFAULT_TICKET_BOARD_CHROME }
  try {
    const parsed = JSON.parse(raw) as unknown
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return { ...DEFAULT_TICKET_BOARD_CHROME }
    }
    const rec = parsed as Record<string, unknown>
    return {
      listWidth:
        typeof rec.listWidth === 'number'
          ? clampTicketListWidth(rec.listWidth)
          : DEFAULT_TICKET_BOARD_CHROME.listWidth,
      collapsed:
        typeof rec.collapsed === 'boolean' ? rec.collapsed : DEFAULT_TICKET_BOARD_CHROME.collapsed,
      chatOpen:
        typeof rec.chatOpen === 'boolean' ? rec.chatOpen : DEFAULT_TICKET_BOARD_CHROME.chatOpen,
    }
  } catch {
    return { ...DEFAULT_TICKET_BOARD_CHROME }
  }
}

export function writeTicketBoardChrome(
  patch: Partial<TicketBoardChrome>,
  label = getWindowLabel(),
): TicketBoardChrome {
  const current = readTicketBoardChrome(label)
  const next: TicketBoardChrome = {
    listWidth:
      typeof patch.listWidth === 'number' ? clampTicketListWidth(patch.listWidth) : current.listWidth,
    collapsed: typeof patch.collapsed === 'boolean' ? patch.collapsed : current.collapsed,
    chatOpen: typeof patch.chatOpen === 'boolean' ? patch.chatOpen : current.chatOpen,
  }
  const s = storage()
  if (s) {
    try {
      s.setItem(ticketBoardKey(label), JSON.stringify(next))
    } catch {
      // Private mode / quota — the caller's state still updates.
    }
  }
  return next
}
