// Who's here: "Watching N" counts only people who are here (you included);
// away people are listed apart; the avatar stack is everyone else, with an
// overflow; and "when" words for changes.

import { describe, expect, it } from 'vitest'
import { agoText, avatarStack, watching, type LivePerson } from './people'

const P = (id: string, kind: 'person' | 'agent', extra: Partial<LivePerson> = {}): LivePerson => ({ id, name: id[0].toUpperCase() + id.slice(1), kind, colour: '#336699', away: false, ...extra })

describe('watching', () => {
  const people = [P('atlas', 'agent'), P('sam', 'person', { away: true }), P('you', 'person', { self: true }), P('brook', 'agent'), P('kai', 'person')]

  it('counts only people who are here, you included', () => {
    const w = watching(people)
    expect(w.count).toBe(4)
    expect(w.away.map((p) => p.id)).toEqual(['sam'])
  })

  it('lists you first, then people, then agents', () => {
    expect(watching(people).here.map((p) => p.id)).toEqual(['you', 'kai', 'atlas', 'brook'])
  })

  it('everyone away but you still counts you', () => {
    expect(watching([P('you', 'person', { self: true }), P('sam', 'person', { away: true })]).count).toBe(1)
  })
})

describe('the avatar stack', () => {
  it('shows everyone but you (away ones too, faded by the view), then +N', () => {
    const many = [P('you', 'person', { self: true }), ...['a1', 'a2', 'a3', 'a4', 'a5', 'a6'].map((id) => P(id, 'person'))]
    const s = avatarStack(many, 4)
    expect(s.shown.map((p) => p.id)).toEqual(['a1', 'a2', 'a3'])
    expect(s.more.map((p) => p.id)).toEqual(['a4', 'a5', 'a6'])
    const few = avatarStack(many.slice(0, 5), 4)
    expect(few.shown.length).toBe(4)
    expect(few.more).toEqual([])
  })
})

describe('when', () => {
  it('reads just now, minutes, then the clock', () => {
    const now = Date.UTC(2026, 10, 15, 14, 30)
    expect(agoText(now - 20_000, now)).toBe('just now')
    expect(agoText(now - 2 * 60_000, now)).toBe('2 min ago')
    expect(agoText(Date.UTC(2026, 10, 15, 9, 5), now, 'UTC')).toBe('09:05')
  })
})
