import { describe, expect, it } from 'vitest'
import {
  displayThreadAddress,
  isThreadAddressMiss,
  publishThreadAddress,
  subscribeThreadAddress,
  threadAddressChangeMatches,
  threadErrorText,
} from './thread-address-bus'

describe('thread address bus (prd-thread-survives-tab-rename S4)', () => {
  it('Q5: an old row stamped with an earlier address shows the current one', () => {
    const past = ['k2/3', 'k2/reviewer']
    expect(displayThreadAddress('k2/3', 'k2/critic', past)).toBe('k2/critic')
    expect(displayThreadAddress('k2/reviewer', 'k2/critic', past)).toBe('k2/critic')
    expect(displayThreadAddress('k2', 'k2/critic', past)).toBe('k2')
    expect(displayThreadAddress('rosson', 'k2/critic', past)).toBe('rosson')
    // An older daemon sends no past addresses: storage is shown as is.
    expect(displayThreadAddress('k2/3', 'k2/critic', [])).toBe('k2/3')
    expect(displayThreadAddress('k2/3', '', past)).toBe('k2/3')
  })

  it('a 404 for an address is a miss; its hint is the text a person sees', () => {
    const e = new Error(JSON.stringify({ ok: false, error: { code: 'not_found', hint: "unknown overlay addr 'k2/3'" } }))
    expect(isThreadAddressMiss(e)).toBe(true)
    expect(threadErrorText(e)).toBe("unknown overlay addr 'k2/3'")
    expect(isThreadAddressMiss(new Error('forbidden'))).toBe(false)
    expect(threadErrorText(new Error('plain'))).toBe('plain')
  })

  it('a change matches the view by pane, conversation, or the address it holds', () => {
    const change = { address: 'k2/reviewer', previous: 'k2/3', paneGroupId: 'pg-1', conversationId: 'c-1' }
    expect(threadAddressChangeMatches(change, { paneGroupId: 'pg-1' })).toBe(true)
    expect(threadAddressChangeMatches(change, { conversationId: 'c-1' })).toBe(true)
    expect(threadAddressChangeMatches(change, { addr: 'k2/3' })).toBe(true)
    expect(threadAddressChangeMatches(change, { addr: 'k2/4', paneGroupId: 'pg-2', conversationId: 'c-2' })).toBe(false)
  })

  it('every listener hears a change once; unsubscribe stops it', () => {
    const seen: string[] = []
    const off = subscribeThreadAddress((c) => seen.push(c.address))
    publishThreadAddress({ address: 'k2/reviewer', previous: 'k2/3' })
    off()
    publishThreadAddress({ address: 'k2/critic', previous: 'k2/reviewer' })
    expect(seen).toEqual(['k2/reviewer'])
  })
})
