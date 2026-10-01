import { describe, it, expect } from 'vitest'
import { heartbeatActorPhrase, heartbeatHistoryActorLine } from './heartbeat-actor'

describe('heartbeatActorPhrase (AH18, twin of k2_core actor_phrase)', () => {
  it('names every caller class', () => {
    expect(heartbeatActorPhrase('owner-token')).toBe('the owner')
    expect(heartbeatActorPhrase('user:rosson')).toBe('user rosson')
    expect(heartbeatActorPhrase('agent:sales')).toBe('agent sales')
    expect(heartbeatActorPhrase('app:bob')).toBe('app user bob')
    expect(heartbeatActorPhrase('app-token:kiosk')).toBe('app token kiosk')
  })
  it('a scheduler row has no actor', () => {
    expect(heartbeatActorPhrase(null)).toBeNull()
    expect(heartbeatActorPhrase(undefined)).toBeNull()
    expect(heartbeatActorPhrase('  ')).toBeNull()
  })
})

describe('heartbeatHistoryActorLine', () => {
  it('changed rows read "<verb> by <who>"', () => {
    expect(heartbeatHistoryActorLine({ decision: 'changed', reason: 'disabled', actor: 'app:bob' })).toBe(
      'disabled by app user bob',
    )
    expect(
      heartbeatHistoryActorLine({ decision: 'changed', reason: 'instructions edited', actor: 'user:rosson' }),
    ).toBe('instructions edited by user rosson')
  })
  it('fires with an actor read "fired by <who>"', () => {
    expect(heartbeatHistoryActorLine({ decision: 'fired', reason: 'ok', actor: 'agent:sales' })).toBe(
      'fired by agent sales',
    )
    expect(heartbeatHistoryActorLine({ decision: 'skipped_locked', reason: 'x', actor: 'app:bob' })).toBe(
      'skipped by app user bob',
    )
  })
  it('scheduler fires add no line', () => {
    expect(heartbeatHistoryActorLine({ decision: 'fired', reason: 'ok', actor: null })).toBeNull()
  })
})
