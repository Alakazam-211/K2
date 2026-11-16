// Follow, as one state machine: idle ("Follow Atlas") → following
// ("Following Brook · Stop") → looked away ("Back to Brook") → left
// ("Sam left the file", 4 s, you keep the view). Gone once the agent is done.

import { describe, expect, it } from 'vitest'
import { FOLLOW_IDLE, LEFT_NOTE_MS, followReducer, followView, type FollowState } from './follow'
import type { LivePerson } from './people'

const P = (id: string, name: string, kind: 'person' | 'agent' = 'person', extra: Partial<LivePerson> = {}): LivePerson => ({ id, name, kind, colour: '#336699', away: false, ...extra })
const people = [P('you', 'You', 'person', { self: true }), P('atlas', 'Atlas', 'agent'), P('brook', 'Brook', 'agent'), P('sam', 'Sam')]
const ctx = { people, agent: 'atlas', agentDone: false }

describe('the follow control’s words', () => {
  it('idle: “Follow Atlas”, filled, aimed at the agent at work', () => {
    expect(followView(FOLLOW_IDLE, ctx)).toMatchObject({ control: 'follow', label: 'Follow Atlas', target: 'atlas' })
  })

  it('following: one control that always names whom, tinted with their colour', () => {
    const s = followReducer(FOLLOW_IDLE, { type: 'follow', who: 'brook' })
    expect(s).toEqual({ kind: 'following', who: 'brook' })
    const v = followView(s, ctx)
    expect(v).toMatchObject({ control: 'following', label: 'Following Brook', stop: 'Stop', target: 'brook', colour: '#336699' })
  })

  it('looked away: “Back to Brook”', () => {
    const s = followReducer({ kind: 'following', who: 'brook' }, { type: 'look-away' })
    expect(s).toEqual({ kind: 'away', who: 'brook' })
    expect(followView(s, ctx)).toMatchObject({ control: 'back', label: 'Back to Brook', target: 'brook' })
    expect(followReducer(s, { type: 'back' })).toEqual({ kind: 'following', who: 'brook' })
  })

  it('looking away while idle changes nothing', () => {
    expect(followReducer(FOLLOW_IDLE, { type: 'look-away' })).toBe(FOLLOW_IDLE)
  })

  it('stop goes back to idle from following and from away', () => {
    expect(followReducer({ kind: 'following', who: 'sam' }, { type: 'stop' })).toEqual(FOLLOW_IDLE)
    expect(followReducer({ kind: 'away', who: 'sam' }, { type: 'stop' })).toEqual(FOLLOW_IDLE)
  })

  it('the person you follow leaving: “Sam left the file” for 4 s, then idle', () => {
    const s = followReducer({ kind: 'following', who: 'sam' }, { type: 'left', who: 'sam', name: 'Sam', now: 1000 })
    expect(s).toEqual({ kind: 'left', who: 'sam', name: 'Sam', until: 1000 + LEFT_NOTE_MS })
    expect(LEFT_NOTE_MS).toBe(4000)
    const v = followView(s, ctx)
    expect(v.note).toBe('Sam left the file')
    expect(v).toMatchObject({ control: 'follow', label: 'Follow Atlas' })
    expect(followReducer(s, { type: 'tick', now: 4999 })).toBe(s)
    expect(followReducer(s, { type: 'tick', now: 5000 })).toEqual(FOLLOW_IDLE)
  })

  it('someone else leaving doesn’t touch it', () => {
    const s: FollowState = { kind: 'following', who: 'brook' }
    expect(followReducer(s, { type: 'left', who: 'sam', name: 'Sam', now: 1 })).toBe(s)
  })

  it('a followed person who isn’t in the list any more reads as left', () => {
    const v = followView({ kind: 'following', who: 'kai' }, ctx)
    expect(v.control).toBe('follow')
  })

  it('is gone once the agent is done, and with no agent at all', () => {
    expect(followView(FOLLOW_IDLE, { ...ctx, agentDone: true }).control).toBe('none')
    expect(followView(FOLLOW_IDLE, { ...ctx, agent: null }).control).toBe('none')
    // Following a person still shows while the agent is done.
    expect(followView({ kind: 'following', who: 'sam' }, { ...ctx, agentDone: true }).control).toBe('following')
  })

  it('never follows yourself', () => {
    expect(followReducer(FOLLOW_IDLE, { type: 'follow', who: 'you' }, people)).toBe(FOLLOW_IDLE)
  })
})
