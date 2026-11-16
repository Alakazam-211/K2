// The lanes model beyond the mock's checks: tracks paired by (name, kind)
// across an outside save, as the kit pairs them.

import { describe, expect, it } from 'vitest'
import { modelOf, pairTracks } from './fold'
import { HOLDGONE, WATCH } from './scenarios'
import { at, raw } from './test-world'

describe('tracks across an outside save', () => {
  it('pair by name and kind: a save that keeps the tracks keeps every lane', () => {
    const j = at(WATCH, 'outside save')
    const p = pairTracks(modelOf(raw(WATCH, j - 1).B), modelOf(raw(WATCH, j).B))
    expect(p.pairs).toEqual([
      ['@trk:1', '@trk:1'],
      ['@trk:2', '@trk:2'],
      ['@trk:3', '@trk:3'],
      ['@trk:4', '@trk:4'],
    ])
    expect(p.gone).toEqual([])
    expect(p.added).toEqual([])
  })

  it('a track renamed outside K2 has no partner: it is gone, which is why the hold on it ended', () => {
    const p = pairTracks(modelOf(raw(HOLDGONE, 0).B), modelOf(raw(HOLDGONE, 1).B))
    expect(p.gone).toEqual(['@trk:4'])
    expect(p.added).toEqual(['@trk:4'])
    const S = raw(HOLDGONE, 1)
    const end = S.log.find((e) => e.kind === 'claim_end')
    expect(end && end.kind === 'claim_end' ? [end.how, end.keys, end.was] : null).toEqual(['gone_outside_save', p.gone, 'Music'])
  })

  it('pairs in list order when two tracks share a name and kind', () => {
    const B0 = raw(WATCH, 0).B
    const M0 = modelOf(B0)
    const B1 = { ...B0, lists: { ...B0.lists, tracks: [...B0.lists.tracks, ['@trk:9', 'audio'] as [string, string]] }, maps: { ...B0.maps, track_props: { ...B0.maps.track_props, '@trk:9': { name: 'Music' } } } }
    const p = pairTracks(M0, modelOf(B1))
    expect(p.pairs.find(([, b]) => b === '@trk:4')).toEqual(['@trk:4', '@trk:4'])
    expect(p.added).toEqual(['@trk:9'])
  })
})
