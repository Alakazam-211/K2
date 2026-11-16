// "Say something to X": typing, sending, sent, a reply, a failure that keeps
// the words, Send again, a second failure, and off while offline.

import { describe, expect, it } from 'vitest'
import { SAY_IDLE, sayReducer, sayView, type SayState } from './say'

const ctx = { toName: 'Atlas', offline: false, paused: false }

describe('say', () => {
  it('starts empty, with Send quiet until there is text', () => {
    const v = sayView(SAY_IDLE, ctx)
    expect(v.placeholder).toBe('Say something to Atlas…')
    expect(v.sendPrimary).toBe(false)
    expect(v.canSend).toBe(false)
    const typed = sayReducer(SAY_IDLE, { type: 'draft', text: 'Keep the cut on the breath?' })
    expect(sayView(typed, ctx)).toMatchObject({ sendPrimary: true, canSend: true })
  })

  it('sends, then shows the sent line and the reply', () => {
    let s = sayReducer(SAY_IDLE, { type: 'draft', text: 'Keep the cut?' })
    s = sayReducer(s, { type: 'send' })
    expect(s.status).toBe('sending')
    expect(sayView(s, ctx).canSend).toBe(false)
    s = sayReducer(s, { type: 'sent' })
    expect(s.draft).toBe('')
    expect(sayView(s, ctx).line).toEqual({ tone: 'plain', who: 'You → Atlas', text: 'Keep the cut?' })
    s = sayReducer(s, { type: 'reply', text: 'On it.' })
    expect(sayView(s, ctx).line).toEqual({ tone: 'plain', who: 'Atlas → You', text: 'On it.' })
  })

  it('a failure keeps the words and offers Send again; the main Send steps aside', () => {
    let s: SayState = sayReducer(SAY_IDLE, { type: 'draft', text: 'Is B-roll 03 better?' })
    s = sayReducer(sayReducer(s, { type: 'send' }), { type: 'failed' })
    expect(s.draft).toBe('Is B-roll 03 better?')
    const v = sayView(s, ctx)
    expect(v.line).toMatchObject({ tone: 'bad', text: 'Couldn’t send: the connection dropped. Your message is kept here.', action: 'Send again' })
    expect(v.canSend).toBe(false)
    s = sayReducer(sayReducer(s, { type: 'send-again' }), { type: 'failed' })
    expect(sayView(s, ctx).line).toMatchObject({ tone: 'bad', text: 'Still couldn’t send (second try). Your message is kept here.', action: 'Send again' })
    s = sayReducer(sayReducer(s, { type: 'send-again' }), { type: 'sent' })
    expect(sayView(s, ctx).line).toEqual({ tone: 'plain', who: 'You → Atlas', text: 'Is B-roll 03 better?', note: 'Sent.' })
  })

  it('is off while offline, and says why', () => {
    const typed = sayReducer(SAY_IDLE, { type: 'draft', text: 'hello' })
    const v = sayView(typed, { ...ctx, offline: true })
    expect(v).toMatchObject({ placeholder: 'Back online to send', disabled: true, canSend: false })
  })

  it('still sends while edits are paused', () => {
    expect(sayView(SAY_IDLE, { ...ctx, paused: true }).placeholder).toBe('Edits paused; you can still message Atlas…')
  })

  it('sending nothing is not a send', () => {
    expect(sayReducer(SAY_IDLE, { type: 'send' })).toBe(SAY_IDLE)
    expect(sayReducer(sayReducer(SAY_IDLE, { type: 'draft', text: '   ' }), { type: 'send' }).status).toBe('idle')
  })

  it('the last line names who said it, even after you follow someone else', () => {
    let s: SayState = sayReducer(SAY_IDLE, { type: 'reply', text: 'On it.', name: 'Atlas' })
    expect(sayView(s, { ...ctx, toName: 'Sam' }).line).toEqual({ tone: 'plain', who: 'Atlas → You', text: 'On it.' })
    s = sayReducer(sayReducer(sayReducer(s, { type: 'draft', text: 'Here?' }), { type: 'send' }), { type: 'sent', name: 'Atlas' })
    expect(sayView(s, { ...ctx, toName: 'Sam' }).line).toEqual({ tone: 'plain', who: 'You → Atlas', text: 'Here?' })
  })
})
