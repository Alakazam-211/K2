// Tickets board view state, per WINDOW: the ticket list's width, whether
// it is folded into the avatar rail, and whether the right-hand agent chat
// rail is open. Thin-client view preference only,
// never daemon state — same storage rule as `window-chrome.ts`
// (`k2.windowChrome.<label>`, prd-per-window-chrome-v1): localStorage in
// the desktop app, sessionStorage on the hosted web client, keyed by the
// Tauri window label so two windows keep their own layout.

import { getWindowLabel } from '@/lib/window-chrome'
import { isWebClient } from '@/lib/is-web'
import { ICON_RAIL_WIDTH_PX } from '@/lib/icon-rail'

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
  /** The width the person dragged the discussion to; null = the width
   *  rules' own (about a third of what the list leaves). */
  discussionWidth: number | null
}

export const DEFAULT_TICKET_BOARD_CHROME: TicketBoardChrome = {
  listWidth: TICKET_LIST_DEFAULT_WIDTH,
  collapsed: false,
  chatOpen: true,
  discussionWidth: null,
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
      discussionWidth:
        typeof rec.discussionWidth === 'number' ? clampTicketDiscussionWidth(rec.discussionWidth) : null,
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
    discussionWidth:
      patch.discussionWidth === null
        ? null
        : typeof patch.discussionWidth === 'number'
          ? clampTicketDiscussionWidth(patch.discussionWidth)
          : current.discussionWidth,
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

// ── Width rules ─────────────────────────────────────────────────────────
//
// The board is three parts in one flow: pick a ticket in the list, read its
// report in the middle, discuss it on the right. The report is the one to
// read, so it gets the most room at every width, and all three stay on
// screen for as long as they fit:
//
//   1. The discussion takes about a third of what the list leaves (no less
//      than TICKET_DISCUSSION_MIN_WIDTH, no more than ..._MAX_WIDTH), or the
//      width the person dragged it to, never past the report.
//   2. While a ticket is open, the list gives way first: it narrows from the
//      person's width toward TICKET_LIST_MIN_WIDTH until the report leads
//      both side parts by TICKET_REPORT_LEAD_PX.
//   3. If even the narrowest list leaves the report short, the list folds to
//      its avatar rail by itself. The person's own choice is untouched, and
//      the list comes back when there's room.
//   4. Only when the rail still leaves the report short does the discussion
//      leave the row: it becomes a drawer over the report's right edge,
//      opened from a tab.
//
// With no ticket open there is no report or discussion to make room for, so
// the list keeps the person's width and the empty middle the rest.
//
// Widths are the board's inner width in CSS px. 0 means not measured yet
// (and test DOMs, which have no layout): the person's widths are used.

/** The list folded into avatars (the app's icon-rail width). */
export const TICKET_LIST_RAIL_WIDTH = ICON_RAIL_WIDTH_PX
/** The discussion's narrowest and widest automatic width. */
export const TICKET_DISCUSSION_MIN_WIDTH = 300
export const TICKET_DISCUSSION_MAX_WIDTH = 480
/** How wide a dragged discussion may go (still never past the report). */
export const TICKET_DISCUSSION_DRAG_MAX_WIDTH = 720
/** The discussion before the board is measured. */
export const TICKET_DISCUSSION_DEFAULT_WIDTH = 360
/** How far the report leads the wider of the list and the discussion. */
export const TICKET_REPORT_LEAD_PX = 24
/** With no ticket open, the empty middle keeps at least this much. */
export const TICKET_EMPTY_PANE_MIN_WIDTH = 320

export type TicketListMode = 'open' | 'rail' | 'none'
export type TicketDiscussionMode = 'side' | 'drawer' | 'off'

export interface TicketBoardLayoutInput {
  /** The board's inner width (CSS px); 0 = not measured. */
  width: number
  /** The gap between two parts (the pane gap; 0 in Terminal). */
  gap?: number
  /** False where there is no list (the ticket's own window). */
  hasList?: boolean
  /** The person's list width and fold. */
  listWidth: number
  collapsed: boolean
  /** A ticket is open (its report and discussion are drawn). */
  ticketOpen: boolean
  /** The person wants the discussion shown (and the ticket can have one). */
  chatOpen: boolean
  /** The width the person dragged the discussion to, if any. */
  discussionWidth?: number | null
}

export interface TicketBoardLayout {
  list: TicketListMode
  listWidth: number
  discussion: TicketDiscussionMode
  discussionWidth: number
  /** What's left for the report (0 when not measured). */
  reportWidth: number
  /** The list is a rail because of the width, not the person's fold. */
  listAutoFolded: boolean
}

function clampTo(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, n))
}

export function clampTicketDiscussionWidth(width: number): number {
  if (!Number.isFinite(width)) return TICKET_DISCUSSION_DEFAULT_WIDTH
  return Math.round(clampTo(width, TICKET_DISCUSSION_MIN_WIDTH, TICKET_DISCUSSION_DRAG_MAX_WIDTH))
}

/** The discussion's width beside the report, given the room the list
 *  leaves them (gaps taken out). */
export function ticketDiscussionWidth(room: number, dragged?: number | null): number {
  if (typeof dragged === 'number' && Number.isFinite(dragged)) {
    const cap = Math.max(TICKET_DISCUSSION_MIN_WIDTH, Math.floor((room - TICKET_REPORT_LEAD_PX) / 2))
    return Math.round(clampTo(dragged, TICKET_DISCUSSION_MIN_WIDTH, Math.min(TICKET_DISCUSSION_DRAG_MAX_WIDTH, cap)))
  }
  return Math.round(clampTo(room / 3, TICKET_DISCUSSION_MIN_WIDTH, TICKET_DISCUSSION_MAX_WIDTH))
}

/** The board's layout at a width (the rules above). */
export function ticketBoardLayout(input: TicketBoardLayoutInput): TicketBoardLayout {
  const { width, gap = 0, hasList = true, collapsed, ticketOpen, chatOpen } = input
  const userList = clampTicketListWidth(input.listWidth)
  const dragged = input.discussionWidth ?? null
  const userMode: TicketListMode = !hasList ? 'none' : collapsed ? 'rail' : 'open'
  const listW = (mode: TicketListMode, openWidth: number): number =>
    mode === 'none' ? 0 : mode === 'rail' ? TICKET_LIST_RAIL_WIDTH : openWidth
  const wantChat = ticketOpen && chatOpen

  if (!(width > 0)) {
    return {
      list: userMode,
      listWidth: listW(userMode, userList),
      discussion: wantChat ? 'side' : 'off',
      discussionWidth: dragged === null ? TICKET_DISCUSSION_DEFAULT_WIDTH : clampTicketDiscussionWidth(dragged),
      reportWidth: 0,
      listAutoFolded: false,
    }
  }

  if (!ticketOpen) {
    const openWidth = Math.max(TICKET_LIST_MIN_WIDTH, Math.min(userList, width - gap - TICKET_EMPTY_PANE_MIN_WIDTH))
    const lw = listW(userMode, openWidth)
    return {
      list: userMode,
      listWidth: lw,
      discussion: 'off',
      discussionWidth: 0,
      reportWidth: Math.max(0, width - lw - (userMode === 'none' ? 0 : gap)),
      listAutoFolded: false,
    }
  }

  // At a list width: the discussion, the report, and whether the report
  // leads both side parts.
  const fit = (lw: number, withList: boolean): { d: number; r: number; ok: boolean } => {
    const room = width - lw - (withList ? gap : 0) - (wantChat ? gap : 0)
    const d = wantChat ? ticketDiscussionWidth(room, dragged) : 0
    const r = room - d
    return { d, r, ok: r >= Math.max(withList ? lw : 0, d) + TICKET_REPORT_LEAD_PX }
  }
  const side: TicketDiscussionMode = wantChat ? 'side' : 'off'

  if (userMode === 'open') {
    for (let lw = userList; lw >= TICKET_LIST_MIN_WIDTH; lw--) {
      const f = fit(lw, true)
      if (f.ok) return { list: 'open', listWidth: lw, discussion: side, discussionWidth: f.d, reportWidth: f.r, listAutoFolded: false }
    }
  }
  const mode: TicketListMode = userMode === 'none' ? 'none' : 'rail'
  const lw = listW(mode, 0)
  const listAutoFolded = userMode === 'open'
  const f = fit(lw, mode !== 'none')
  if (f.ok || !wantChat) {
    return { list: mode, listWidth: lw, discussion: side, discussionWidth: f.d, reportWidth: f.r, listAutoFolded }
  }
  // Too narrow for a discussion beside the report: a drawer over it.
  const report = Math.max(0, width - lw - (mode === 'none' ? 0 : gap))
  const drawer = Math.max(
    Math.min(TICKET_DISCUSSION_MIN_WIDTH, report),
    Math.min(ticketDiscussionWidth(report, dragged), report - 2 * TICKET_REPORT_LEAD_PX),
  )
  return { list: mode, listWidth: lw, discussion: 'drawer', discussionWidth: drawer, reportWidth: report, listAutoFolded }
}
