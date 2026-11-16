// What a real timeline doc's wire gives, in the view's shapes: the sequence a
// say row's time is in, the monitor's honest preview states, a refusal's words.

import { describe, expect, it } from 'vitest'
import { modelOf } from './fold'
import { declineWords, FIRST_SEQUENCE, realPreview, sequenceOf } from './live-map'
import type { Book } from './schema'

const book = (timeline: Record<string, Record<string, string>>): Book => ({
  rev: 1,
  lists: { tracks: [['@trk:1', 'video']], items: [['@item:1', 'clip']], media: [['@med:1', 'file']] },
  hidden: {},
  maps: {
    timeline,
    track_props: { '@trk:1': { name: 'V1', nid: 't1' } },
    item_props: { '@item:1': { name: 'Shot A', track: 't1', geom: '0,254016000000', media: 'm1', src: '0' } },
    media_props: { '@med:1': { file: 'a.mov', name: 'a.mov', nid: 'm1' } },
  },
  derived: { containers: {} },
})

describe('live-map', () => {
  it('a say row’s time is in the doc’s own sequence when its keyed state names one, else the first timeline’s derived key', () => {
    expect(sequenceOf(modelOf(book({ rate: { '': '24/1' }, name: { '': 'Cut' } })))).toBe(FIRST_SEQUENCE)
    expect(FIRST_SEQUENCE).toBe('@d-seq:1')
    expect(sequenceOf(modelOf(book({ rate: { '': '24/1' }, uid: { '': 'a1b2' } })))).toBe('@uid:a1b2')
  })

  it('the monitor’s honest states: nothing ready yet while preparing; every video source is no preview where the host can’t decode', () => {
    const M = modelOf(book({ rate: { '': '24/1' } }))
    expect(realPreview(M, 'preparing').prep).toEqual({ ready: [], cant: [] })
    expect(realPreview(M, 'preparing').nopreview).toEqual([])
    expect(realPreview(M, 'not_on_this_host').prep).toBeUndefined()
  })

  it('a refusal’s words end on what you can still do; an unknown code keeps the kit’s message', () => {
    for (const c of ['live_many_timelines', 'live_time_off_tick', 'live_too_large', 'live_time_range', 'live_structure', 'unreadable']) expect(declineWords(c).text).toMatch(/You can still open the saved file\.$/)
    expect(declineWords('live_new_thing', 'Something new.')).toMatchObject({ title: 'This file can’t be watched live', text: 'Something new. You can still open the saved file.' })
  })
})
