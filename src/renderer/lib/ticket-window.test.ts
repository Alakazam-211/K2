import { describe, expect, it } from 'vitest'
import { parseTicketWindowId, ticketIdFromHash, ticketIdFromLabel } from './ticket-window'

describe('ticket window id', () => {
  const id = '3f2a9c1e-7b4d-4e0a-9d6f-0123456789ab'

  it('reads the hash or the window-ticket- label', () => {
    expect(ticketIdFromHash(`#ticket=${id}`)).toBe(id)
    expect(ticketIdFromLabel(`window-ticket-${id}`)).toBe(id)
    expect(parseTicketWindowId('', `window-ticket-${id}`)).toBe(id)
    expect(parseTicketWindowId(`#ticket=${id}`, 'main')).toBe(id)
  })

  it('ignores other windows and junk ids', () => {
    expect(parseTicketWindowId('', 'main')).toBeNull()
    expect(parseTicketWindowId('#focus=p1', 'focus-p1')).toBeNull()
    expect(parseTicketWindowId('', 'window-1234')).toBeNull()
    expect(ticketIdFromHash('#ticket=a%2Fb')).toBeNull()
    expect(ticketIdFromHash('#ticket=<x>')).toBeNull()
    expect(parseTicketWindowId('', null)).toBeNull()
  })
})
