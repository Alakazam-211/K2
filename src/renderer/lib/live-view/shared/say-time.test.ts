// Where in time a message points: the playhead's TimePosition shape (the
// kit's participant.rs: `at` in the pack's unit as a decimal string, `in`
// the sequence it's in, by address), and reading a say frame's `time` in
// either shape for one release.

import { describe, expect, it } from 'vitest'
import { readSayTime, sayTime } from './say-time'

describe('a message’s time', () => {
  it('is built as the playhead’s time position: at in ticks, in the sequence', () => {
    expect(sayTime(1234n, '@seq:S1')).toEqual({ at: '1234', in: '@seq:S1' })
    expect(sayTime('0', '@d-seq:1')).toEqual({ at: '0', in: '@d-seq:1' })
  })

  it('reads the TimePosition shape, and (this release only) the old bare ticks', () => {
    expect(readSayTime({ at: '1234', in: '@seq:S2' })).toEqual({ at: '1234', in: '@seq:S2' })
    expect(readSayTime({ at: '1234' })).toEqual({ at: '1234' })
    expect(readSayTime('1234')).toEqual({ at: '1234' })
    for (const bad of [null, undefined, '', 'soon', 12, { at: 12 }, { at: '1.5' }, { at: '12', in: 3 }]) expect(readSayTime(bad), JSON.stringify(bad)).toBeNull()
  })
})
