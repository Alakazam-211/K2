// @vitest-environment jsdom
// Tickets board view state is per WINDOW (localStorage keyed by the Tauri
// window label) and never daemon state.
import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  DEFAULT_TICKET_BOARD_CHROME,
  TICKET_LIST_DEFAULT_WIDTH,
  TICKET_LIST_MAX_WIDTH,
  TICKET_LIST_MIN_WIDTH,
  readTicketBoardChrome,
  ticketBoardKey,
  writeTicketBoardChrome,
} from './ticket-board-chrome'

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'window-abc' }),
}))

afterEach(() => {
  localStorage.clear()
})

describe('ticket board chrome', () => {
  it('defaults to a 400 px list, expanded, with the chat rail open', () => {
    expect(TICKET_LIST_DEFAULT_WIDTH).toBe(400)
    expect(readTicketBoardChrome('main')).toEqual(DEFAULT_TICKET_BOARD_CHROME)
    expect(DEFAULT_TICKET_BOARD_CHROME).toEqual({ listWidth: 400, collapsed: false, chatOpen: true })
  })

  it('a stored width wins over the default', () => {
    localStorage.setItem(ticketBoardKey('main'), JSON.stringify({ listWidth: 300, collapsed: false }))
    expect(readTicketBoardChrome('main')).toEqual({ listWidth: 300, collapsed: false, chatOpen: true })
  })

  it('remembers the chat rail per window', () => {
    writeTicketBoardChrome({ chatOpen: false }, 'main')
    expect(readTicketBoardChrome('main').chatOpen).toBe(false)
    expect(readTicketBoardChrome('window-2').chatOpen).toBe(true)
    writeTicketBoardChrome({ listWidth: 450 }, 'main')
    expect(readTicketBoardChrome('main')).toEqual({ listWidth: 450, collapsed: false, chatOpen: false })
  })

  it('remembers width and collapsed per window label', () => {
    writeTicketBoardChrome({ listWidth: 410 }, 'main')
    writeTicketBoardChrome({ collapsed: true }, 'window-2')
    expect(readTicketBoardChrome('main')).toEqual({ listWidth: 410, collapsed: false, chatOpen: true })
    expect(readTicketBoardChrome('window-2')).toEqual({ listWidth: 400, collapsed: true, chatOpen: true })
    expect(JSON.parse(localStorage.getItem(ticketBoardKey('main')) ?? 'null')).toEqual({
      listWidth: 410,
      collapsed: false,
      chatOpen: true,
    })
    // A patch keeps the other fields.
    writeTicketBoardChrome({ collapsed: true }, 'main')
    expect(readTicketBoardChrome('main')).toEqual({ listWidth: 410, collapsed: true, chatOpen: true })
  })

  it('uses the current window label by default', () => {
    writeTicketBoardChrome({ listWidth: 333 })
    expect(localStorage.getItem('k2.windowChrome.tickets.window-abc')).not.toBeNull()
    expect(readTicketBoardChrome()).toEqual({ listWidth: 333, collapsed: false, chatOpen: true })
  })

  it('clamps the width and survives junk in storage', () => {
    writeTicketBoardChrome({ listWidth: 5 }, 'main')
    expect(readTicketBoardChrome('main').listWidth).toBe(TICKET_LIST_MIN_WIDTH)
    writeTicketBoardChrome({ listWidth: 99_999 }, 'main')
    expect(readTicketBoardChrome('main').listWidth).toBe(TICKET_LIST_MAX_WIDTH)
    localStorage.setItem(ticketBoardKey('main'), '{nope')
    expect(readTicketBoardChrome('main')).toEqual(DEFAULT_TICKET_BOARD_CHROME)
    localStorage.setItem(ticketBoardKey('main'), JSON.stringify({ listWidth: 'wide', collapsed: 1, chatOpen: 'yes' }))
    expect(readTicketBoardChrome('main')).toEqual(DEFAULT_TICKET_BOARD_CHROME)
  })
})
