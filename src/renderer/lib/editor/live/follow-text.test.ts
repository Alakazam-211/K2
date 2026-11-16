// Follow on a live text (build-editors.md E4b; editors-notes.md §2 Follow,
// v2 f10, v3.2 f09): click an avatar, a Here row or "Follow Atlas" and the
// view goes with Atlas; "Following Atlas · Stop" stops it; any scroll or
// typing of yours stops it too. When the agent you follow finishes (its hold
// ends, or its edit finishes with no hold), following ends with "Atlas
// finished. Stopped following." and two ways on: Back to my place (where
// you were when you started following) and Follow again. Pinned here, on the
// pure reducer the editor keeps beside the shell's follow state.

import { describe, expect, it } from 'vitest'
import { finishedWords, TEXT_FOLLOW_IDLE, textFollowReducer, type TextFollow } from './follow-text'

const run = (s: TextFollow, ...as: Parameters<typeof textFollowReducer>[1][]): TextFollow => as.reduce(textFollowReducer, s)

describe('textFollowReducer', () => {
  it('follows, remembers your place, and stops', () => {
    const s = run(TEXT_FOLLOW_IDLE, { type: 'follow', who: 'atlas', home: 420 })
    expect(s).toEqual({ kind: 'following', who: 'atlas', home: 420 })
    expect(run(s, { type: 'stop' })).toEqual(TEXT_FOLLOW_IDLE)
  })

  it('stops when you scroll or type, and following someone else keeps your first place', () => {
    const s = run(TEXT_FOLLOW_IDLE, { type: 'follow', who: 'atlas', home: 420 }, { type: 'follow', who: 'sam', home: 900 })
    expect(s).toEqual({ kind: 'following', who: 'sam', home: 420 })
    expect(run(s, { type: 'moved' })).toEqual(TEXT_FOLLOW_IDLE)
  })

  it('ends with "finished" when the one you follow finishes, and only them', () => {
    const s = run(TEXT_FOLLOW_IDLE, { type: 'follow', who: 'atlas', home: 420 })
    expect(run(s, { type: 'finished', who: 'sam', name: 'Sam' })).toBe(s)
    const f = run(s, { type: 'finished', who: 'atlas', name: 'Atlas' })
    expect(f).toEqual({ kind: 'finished', who: 'atlas', name: 'Atlas', home: 420 })
    expect(finishedWords(f)).toEqual({ text: 'finished. Stopped following.', back: 'Back to my place', again: 'Follow again' })
    expect(finishedWords(s)).toBeNull()
  })

  it('Back to my place and Follow again', () => {
    const f = run(TEXT_FOLLOW_IDLE, { type: 'follow', who: 'atlas', home: 420 }, { type: 'finished', who: 'atlas', name: 'Atlas' })
    expect(run(f, { type: 'back' })).toEqual(TEXT_FOLLOW_IDLE)
    expect(run(f, { type: 'again', home: 1300 })).toEqual({ kind: 'following', who: 'atlas', home: 1300 })
    // Your own scrolling leaves the note up: Back to my place still means something.
    expect(run(f, { type: 'moved' })).toBe(f)
  })

  it('goes quiet when the shell says they left (its own note says so)', () => {
    const s = run(TEXT_FOLLOW_IDLE, { type: 'follow', who: 'atlas', home: 0 })
    expect(run(s, { type: 'gone', who: 'atlas' })).toEqual(TEXT_FOLLOW_IDLE)
    expect(run(s, { type: 'gone', who: 'sam' })).toBe(s)
  })
})
