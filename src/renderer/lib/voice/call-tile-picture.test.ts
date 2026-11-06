// The floating call tile's picture: the active-speaker hold (1.5 s, as Sew's
// active-speaker sink holds), and whose video it shows.

import { describe, expect, it } from 'vitest'
import { LOCAL_CAMERA_TRACK, LOCAL_SCREEN_TRACK, LOCAL_VIDEO_OFF } from './video-engine'
import { fakeTrack } from './fake-video-engine'
import { videoTiles, type CallPerson } from './video-labels'
import { holdLeft, nextSpeaker, NO_SPEAKER, SPEAKER_HOLD_MS, speakingIds, tilePicture, videoLive } from './call-tile-picture'

describe('the active-speaker hold', () => {
  it('holds a speaker 1.5 s after they stop before someone else takes over', () => {
    expect(SPEAKER_HOLD_MS).toBe(1500)
    let h = nextSpeaker(NO_SPEAKER, ['7'], 0)
    expect(h.id).toBe('7')
    // 7 talks for 10 s, then stops as 8 starts: 7 keeps the picture.
    h = nextSpeaker(h, ['8'], 10_000)
    expect(h.id).toBe('7')
    expect(holdLeft(h, 10_000)).toBe(1500)
    h = nextSpeaker(h, ['8'], 11_499)
    expect(h.id).toBe('7')
    expect(holdLeft(h, 11_499)).toBe(1)
    h = nextSpeaker(h, ['8'], 11_500)
    expect(h.id).toBe('8')
  })

  it('a speaker who speaks again within the hold keeps it', () => {
    let h = nextSpeaker(NO_SPEAKER, ['7'], 0)
    h = nextSpeaker(h, [], 1_000)
    h = nextSpeaker(h, ['8'], 2_000)
    expect(h.id).toBe('7')
    h = nextSpeaker(h, ['8', '7'], 2_400)
    expect(h.id).toBe('7')
    h = nextSpeaker(h, ['8'], 3_000)
    expect(h.id).toBe('7')
    h = nextSpeaker(h, ['8'], 4_500)
    expect(h.id).toBe('8')
  })

  it('nobody talking keeps the last speaker', () => {
    let h = nextSpeaker(NO_SPEAKER, ['7'], 0)
    h = nextSpeaker(h, [], 60_000)
    expect(h.id).toBe('7')
  })

  it('your own voice never takes the picture', () => {
    const people: CallPerson[] = [
      { memberId: 'me', isSelf: true, user: 'You', speaking: true, muted: false },
      { memberId: '7', user: 'Ada', speaking: true, muted: false },
      { memberId: '8', user: 'Grace', speaking: false, muted: false },
    ]
    expect(speakingIds(people)).toEqual(['7'])
  })
})

describe('whose video the tile shows', () => {
  const people: CallPerson[] = [
    { memberId: 'me', isSelf: true, user: 'You', speaking: false, muted: false },
    { memberId: '7', user: 'Ada', speaking: false, muted: false },
    { memberId: '8', user: 'Grace', speaking: false, muted: false },
  ]
  const on = { camera: 'on' as const, screen: 'off' as const, reason: null }

  it('the active speaker, else someone else, else your self-view', () => {
    const tiles = videoTiles([fakeTrack('7'), fakeTrack('8')], on, people)
    expect(tilePicture(tiles, '8')?.key).toBe('TR_8')
    expect(tilePicture(tiles, '7')?.key).toBe('TR_7')
    expect(tilePicture(tiles, null)?.key).toBe('TR_7')
    expect(tilePicture(videoTiles([], on, people), '8')?.key).toBe(LOCAL_CAMERA_TRACK)
  })

  it("a speaker whose camera is off doesn't hold the picture; a live camera does", () => {
    const tiles = videoTiles([fakeTrack('7', { muted: true }), fakeTrack('8', { paused: 'congestion' }), fakeTrack('9')], LOCAL_VIDEO_OFF, people)
    expect(tilePicture(tiles, '7')?.key).toBe('TR_9')
  })

  it("a screen share shows the same way: the speaker's share first, then anyone's", () => {
    const share = fakeTrack('7', { source: 'screen', track: 'TR_S7' })
    const tiles = videoTiles([fakeTrack('7'), share, fakeTrack('8')], on, people)
    expect(tilePicture(tiles, '7')?.key).toBe('TR_S7')
    expect(tilePicture(tiles, null)?.key).toBe('TR_S7')
    expect(tilePicture(tiles, '8')?.key).toBe('TR_8')
    // Only your own screen: it shows.
    expect(tilePicture(videoTiles([], { camera: 'off', screen: 'on', reason: null }, people), null)?.key).toBe(LOCAL_SCREEN_TRACK)
  })

  it('video is live with any camera or share on, and not with every camera off', () => {
    expect(videoLive(videoTiles([fakeTrack('7', { muted: true })], LOCAL_VIDEO_OFF, people))).toBe(false)
    expect(videoLive(videoTiles([], LOCAL_VIDEO_OFF, people))).toBe(false)
    expect(videoLive(videoTiles([fakeTrack('7')], LOCAL_VIDEO_OFF, people))).toBe(true)
    expect(videoLive(videoTiles([], on, people))).toBe(true)
    expect(tilePicture(videoTiles([fakeTrack('7', { muted: true })], LOCAL_VIDEO_OFF, people), '7')).toBeNull()
  })
})
