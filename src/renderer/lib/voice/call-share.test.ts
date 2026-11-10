import { describe, expect, it } from 'vitest'
import { mainShare, peopleTiles, shareLayout } from './call-share'
import type { CallPerson, VideoTile } from './video-labels'

function tile(key: string, over: Partial<VideoTile> = {}): VideoTile {
  return {
    key,
    memberId: null,
    self: false,
    source: 'camera',
    name: key,
    speaking: false,
    audioMuted: false,
    state: 'live',
    paused: null,
    mirrored: false,
    large: false,
    ...over,
  }
}

const adaShare = tile('TR_S7', { memberId: '7', source: 'screen', large: true })
const linusShare = tile('TR_S9', { memberId: '9', source: 'screen', large: true })
const ownShare = tile('local:screen', { memberId: '1', self: true, source: 'screen' })

describe('which share is in the middle', () => {
  it('nobody sharing: none (the grid)', () => {
    expect(mainShare([tile('TR_7', { memberId: '7' })], null)).toBeNull()
    expect(shareLayout([tile('TR_7', { memberId: '7' })], [], null)).toBeNull()
  })

  it("the pick while it lasts, else the first of someone else's, else your own", () => {
    expect(mainShare([adaShare, linusShare, ownShare], null)?.key).toBe('TR_S7')
    expect(mainShare([adaShare, linusShare, ownShare], 'TR_S9')?.key).toBe('TR_S9')
    expect(mainShare([adaShare, ownShare], 'TR_S9')?.key).toBe('TR_S7')
    expect(mainShare([ownShare, adaShare], null)?.key).toBe('TR_S7')
    expect(mainShare([ownShare], null)?.key).toBe('local:screen')
  })

  it('a stopped share never takes the middle', () => {
    expect(mainShare([{ ...adaShare, state: 'off' }], null)).toBeNull()
  })
})

describe('the people column', () => {
  const people: CallPerson[] = [
    { user: 'You', memberId: '1', isSelf: true, speaking: false, muted: true },
    { user: 'Ada', memberId: '7', speaking: true, muted: false },
    { user: 'Grace', memberId: '8', speaking: false, muted: false },
  ]

  it('everyone, in the call order, you last: their camera where they have one, else their picture', () => {
    const tiles = [adaShare, tile('TR_8', { memberId: '8', name: 'Grace' }), tile('local:camera', { memberId: '1', self: true, name: 'You' })]
    const col = peopleTiles(tiles, people)
    expect(col.map((t) => t.key)).toEqual(['person:7', 'TR_8', 'local:camera'])
    expect(col[0]).toMatchObject({ name: 'Ada', state: 'off', speaking: true, audioMuted: false })
  })

  it('a camera for someone the call does not list yet still shows', () => {
    const col = peopleTiles([tile('TR_5', { memberId: '5' })], people)
    expect(col.map((t) => t.key)).toEqual(['person:7', 'person:8', 'person:1', 'TR_5'])
  })

  it('the other shares go to the top of the column, small', () => {
    const layout = shareLayout([adaShare, linusShare], people, null)
    expect(layout?.main.key).toBe('TR_S7')
    expect(layout?.main.large).toBe(true)
    expect(layout?.others.map((t) => [t.key, t.large])).toEqual([['TR_S9', false]])
  })
})
