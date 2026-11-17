import { describe, expect, it } from 'vitest'
import { NO_BLUE, PRESENCE_TOKEN, presenceSlots } from './presence-slots'

describe('presenceSlots', () => {
  it('others take slots 1, 3, 4 and the blue 2 last; each keeps theirs; a leaver frees theirs', () => {
    const a = presenceSlots(new Map(), ['atlas', 'nova'])
    expect([...a]).toEqual([['atlas', 1], ['nova', 3]])
    const b = presenceSlots(a, ['nova', 'kai', 'atlas'])
    expect([...b].sort()).toEqual([['atlas', 1], ['kai', 4], ['nova', 3]])
    expect(presenceSlots(b, ['kai', 'sam']).get('sam')).toBe(1)
    expect(presenceSlots(new Map(), ['a', 'b', 'c', 'd']).get('d')).toBe(2)
  })

  it('NO_BLUE never hands out the viewer’s blue: past three, the slots repeat', () => {
    const s = presenceSlots(new Map(), ['a', 'b', 'c', 'd', 'e'], NO_BLUE)
    expect([...s.values()]).toEqual([1, 3, 4, 1, 3])
  })

  it('each slot is its token', () => {
    expect(PRESENCE_TOKEN[3]).toBe('var(--color-presence-3)')
  })
})
