// The Tickets board's width rules: pick a ticket in the list, read its
// report in the middle, discuss it on the right. The report gets the most
// room at every width, and all three parts stay on screen as long as they
// can: the list narrows first, then folds to its rail, and only then does
// the discussion become a drawer over the report.
import { describe, expect, it, vi } from 'vitest'
import {
  TICKET_DISCUSSION_MAX_WIDTH,
  TICKET_DISCUSSION_MIN_WIDTH,
  TICKET_LIST_MIN_WIDTH,
  TICKET_LIST_RAIL_WIDTH,
  TICKET_REPORT_LEAD_PX,
  ticketBoardLayout,
  ticketDiscussionWidth,
  type TicketBoardLayoutInput,
} from './ticket-board-chrome'
import { ICON_RAIL_WIDTH_PX } from './icon-rail'

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'window-abc' }),
}))

// The page's inner width at a window width: Glass sets the page in by the
// window inset (8 px a side) with an 8 px pane gap; Terminal has neither.
const GLASS = { inset: 8, gap: 8 }
const TERMINAL = { inset: 0, gap: 0 }

function open(windowWidth: number, look: { inset: number; gap: number }, over: Partial<TicketBoardLayoutInput> = {}) {
  return ticketBoardLayout({
    width: windowWidth - 2 * look.inset,
    gap: look.gap,
    listWidth: 400,
    collapsed: false,
    ticketOpen: true,
    chatOpen: true,
    ...over,
  })
}

describe('ticket board width rules', () => {
  it('the rail is the app icon rail width', () => {
    expect(TICKET_LIST_RAIL_WIDTH).toBe(ICON_RAIL_WIDTH_PX)
  })

  for (const [name, look] of [['Glass', GLASS], ['Terminal', TERMINAL]] as const) {
    it(`${name} at 1280: the full list, the report and the discussion, the report widest`, () => {
      const l = open(1280, look)
      expect(l.list).toBe('open')
      expect(l.listWidth).toBe(400)
      expect(l.discussion).toBe('side')
      expect(l.reportWidth).toBeGreaterThanOrEqual(Math.max(l.listWidth, l.discussionWidth) + TICKET_REPORT_LEAD_PX)
      expect(l.listWidth + l.discussionWidth + l.reportWidth + 2 * look.gap).toBe(1280 - 2 * look.inset)
    })

    it(`${name} at 900: the list narrows but stays open, the report still widest`, () => {
      const l = open(900, look)
      expect(l.list).toBe('open')
      expect(l.listWidth).toBeLessThan(400)
      expect(l.listWidth).toBeGreaterThanOrEqual(TICKET_LIST_MIN_WIDTH)
      expect(l.discussion).toBe('side')
      expect(l.reportWidth).toBeGreaterThan(l.listWidth)
      expect(l.reportWidth).toBeGreaterThan(l.discussionWidth)
    })

    it(`${name} at 800: the list folds to its rail by itself; the discussion stays beside the report`, () => {
      const l = open(800, look)
      expect(l.list).toBe('rail')
      expect(l.listAutoFolded).toBe(true)
      expect(l.listWidth).toBe(TICKET_LIST_RAIL_WIDTH)
      expect(l.discussion).toBe('side')
      expect(l.reportWidth).toBeGreaterThan(l.discussionWidth)
    })
  }

  it('narrower still, the discussion becomes a drawer over the report', () => {
    const l = open(640, GLASS)
    expect(l.list).toBe('rail')
    expect(l.discussion).toBe('drawer')
    expect(l.discussionWidth).toBeLessThan(l.reportWidth)
  })

  it('the report is never narrower than either side part, at any width', () => {
    for (let w = 680; w <= 2400; w += 20) {
      for (const look of [GLASS, TERMINAL]) {
        const l = open(w, look)
        if (l.discussion === 'drawer') continue
        expect(l.reportWidth, `at ${w}`).toBeGreaterThan(l.discussionWidth)
        if (l.list === 'open') expect(l.reportWidth, `at ${w}`).toBeGreaterThan(l.listWidth)
      }
    }
  })

  it('the discussion takes about a third of what the list leaves, within its bounds', () => {
    expect(ticketDiscussionWidth(600)).toBe(TICKET_DISCUSSION_MIN_WIDTH)
    expect(ticketDiscussionWidth(1200)).toBe(400)
    expect(ticketDiscussionWidth(3000)).toBe(TICKET_DISCUSSION_MAX_WIDTH)
  })

  it('a dragged discussion width stands while there is room, but never passes the report', () => {
    expect(ticketDiscussionWidth(1200, 420)).toBe(420)
    // The report keeps the rest, and it is the wider of the two.
    expect(ticketDiscussionWidth(900, 700)).toBeLessThan(900 - ticketDiscussionWidth(900, 700))
    expect(ticketDiscussionWidth(900, 700) * 2 + TICKET_REPORT_LEAD_PX).toBeLessThanOrEqual(900)
  })

  it('the person’s fold and width stand whenever there is room', () => {
    expect(open(1600, GLASS, { collapsed: true }).list).toBe('rail')
    expect(open(1600, GLASS, { collapsed: true }).listAutoFolded).toBe(false)
    expect(open(1600, GLASS, { listWidth: 520 }).listWidth).toBe(520)
  })

  it('the hidden discussion gives its room to the report', () => {
    const l = open(1280, GLASS, { chatOpen: false })
    expect(l.discussion).toBe('off')
    expect(l.reportWidth).toBe(1280 - 16 - 400 - 8)
  })

  it('with no ticket open the list keeps its width and nothing folds', () => {
    const l = open(800, GLASS, { ticketOpen: false })
    expect(l.list).toBe('open')
    expect(l.listWidth).toBe(400)
    expect(l.discussion).toBe('off')
  })

  it('before the board is measured, the person’s widths are used as they are', () => {
    const l = ticketBoardLayout({ width: 0, listWidth: 260, collapsed: false, ticketOpen: true, chatOpen: true })
    expect(l).toMatchObject({ list: 'open', listWidth: 260, discussion: 'side', listAutoFolded: false })
  })

  it('the ticket window has no list: the report and the discussion share the row', () => {
    const l = ticketBoardLayout({ width: 1000, gap: 8, hasList: false, listWidth: 400, collapsed: false, ticketOpen: true, chatOpen: true })
    expect(l.list).toBe('none')
    expect(l.listWidth).toBe(0)
    expect(l.reportWidth + l.discussionWidth + 8).toBe(1000)
  })
})
