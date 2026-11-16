// What the scene's picture can't show, in plain words (k2-mpc-blend
// docs/WIRE.md at 3a0c5cee): the cameras the mesh read left out
// (`cameras_left_out [{name, key?, reason}]`, passed through in the
// route's listing) as one quiet line under the cameras, each name and why
// in its tooltip; and why a view couldn't be drawn, for every refusal and
// failure code, the two new ones (pairing_mismatch, invalid) included.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { camerasLeftOutWords, failureWords, LEFT_OUT_WHY } from './left-out'
import { parseSceneListing } from './preview-route'
import { pictureWords } from './picture-words'
import type { PictureStatus } from './picture-source'

const DIR = join(__dirname, '..', '..', '..', '..', '..', 'tests', 'fixtures', 'scene-preview')
const listing = (): Record<string, unknown> => ({ kind: 'scene', ...JSON.parse(readFileSync(join(DIR, 'listing.json'), 'utf8')) })

describe('cameras left out', () => {
  it('read from the listing when it carries them; none when it doesn’t', () => {
    expect(parseSceneListing(listing()).camerasLeftOut).toEqual([])
    const L = parseSceneListing({ ...listing(), cameras_left_out: [{ name: 'Camera.003', key: '@cam:31', reason: 'camera_not_perspective' }, { name: 'Camera.004', reason: 'camera_sensor_fit_unknown' }, { reason: 'camera_no_aspect' }] })
    expect(L.camerasLeftOut).toEqual([
      { name: 'Camera.003', key: '@cam:31', reason: 'camera_not_perspective' },
      { name: 'Camera.004', key: null, reason: 'camera_sensor_fit_unknown' },
    ])
  })

  it('one quiet line, the names and why in plain words', () => {
    expect(camerasLeftOutWords([])).toBeNull()
    expect(camerasLeftOutWords([{ name: 'Camera.003', key: null, reason: 'camera_not_perspective' }])).toEqual({ line: '1 camera can’t be shown here', details: ['Camera.003: it isn’t a perspective camera'] })
    const w = camerasLeftOutWords([
      { name: 'Camera.003', key: null, reason: 'camera_not_perspective' },
      { name: 'Camera.004', key: null, reason: 'camera_sensor_fit_unknown' },
      { name: 'Odd', key: null, reason: 'something_new' },
    ])!
    expect(w.line).toBe('3 cameras can’t be shown here')
    expect(w.details).toEqual(['Camera.003: it isn’t a perspective camera', 'Camera.004: its sensor fit isn’t known', 'Odd: K2 can’t place it'])
  })

  it('past the cap: the count takes the more, the tooltip ends “…and N more”', () => {
    const L = parseSceneListing({ ...listing(), cameras_left_out: [{ name: 'Camera.003', reason: 'camera_not_perspective' }], cameras_left_out_more: 12 })
    expect(L.camerasLeftOutMore).toBe(12)
    expect(camerasLeftOutWords(L.camerasLeftOut, L.camerasLeftOutMore)).toEqual({ line: '13 cameras can’t be shown here', details: ['Camera.003: it isn’t a perspective camera', '…and 12 more'] })
  })

  it('dropped entries (the route’s, and any this reader can’t read) count, and go in the “more”, never named', () => {
    const L = parseSceneListing({ ...listing(), cameras_left_out: [{ name: 'Camera.003', reason: 'camera_not_perspective' }, { reason: 'camera_no_aspect' }], cameras_left_out_dropped: 2 })
    expect(L.camerasLeftOut).toHaveLength(1)
    expect(L.camerasLeftOutMore).toBe(3)
    expect(camerasLeftOutWords(L.camerasLeftOut, L.camerasLeftOutMore)).toEqual({ line: '4 cameras can’t be shown here', details: ['Camera.003: it isn’t a perspective camera', '…and 3 more'] })
  })

  it('more and dropped together; nothing listed: the count alone, no names', () => {
    const L = parseSceneListing({ ...listing(), cameras_left_out_more: 5, cameras_left_out_dropped: 1 })
    expect(L.camerasLeftOut).toEqual([])
    expect(camerasLeftOutWords(L.camerasLeftOut, L.camerasLeftOutMore)).toEqual({ line: '6 cameras can’t be shown here', details: [] })
    // one, unnamed
    expect(camerasLeftOutWords([], 1)).toEqual({ line: '1 camera can’t be shown here', details: [] })
  })

  it('zero, absent or malformed counts add nothing', () => {
    expect(parseSceneListing({ ...listing(), cameras_left_out_more: 0, cameras_left_out_dropped: -1 }).camerasLeftOutMore).toBe(0)
    expect(parseSceneListing({ ...listing(), cameras_left_out_more: 'x' }).camerasLeftOutMore).toBe(0)
    expect(camerasLeftOutWords([], 0)).toBeNull()
  })

  it('every reason the wire names has its words', () => {
    for (const r of ['camera_not_perspective', 'camera_sensor_fit_unknown', 'camera_no_data', 'camera_no_lens', 'camera_no_clip', 'camera_no_aspect', 'camera_degenerate', 'camera_bad_fov', 'linked_object', 'transform_unplaced', 'transform_not_finite', 'delta_transform', 'parent_not_object', 'parent_unlisted'])
      expect(LEFT_OUT_WHY[r], r).toMatch(/^(it|its|it’s|the) /)
  })
})

describe('why a view couldn’t be drawn', () => {
  const failed = (reason: string): PictureStatus => ({ shown: null, want: 1, state: 'failed', renderMs: null, reusedFrom: null, withoutModifiers: false, reason, landed: false, recovered: false, retrying: false, again: false })
  const ctx = { loading: false, objects: 18, camera: 'Camera 1', askAgent: null } as unknown as Parameters<typeof pictureWords>[1]

  it('the two new codes, in plain words', () => {
    expect(failureWords('pairing_mismatch')).toBe('This view doesn’t match the file’s saved version.')
    expect(failureWords('invalid')).toBe('K2 couldn’t read this part of the file.')
  })

  it('says why on the card, and the rest stays live', () => {
    expect(pictureWords(failed('pairing_mismatch'), ctx).card?.body).toBe('This view doesn’t match the file’s saved version. The outliner and numbers are still live.')
    expect(pictureWords(failed('invalid'), ctx).card?.body).toBe('K2 couldn’t read this part of the file. The outliner and numbers are still live.')
    expect(pictureWords(failed('render_error'), ctx).card?.body).toBe('The picture didn’t finish. The outliner and numbers are still live.')
  })

  it('every refusal code has words; an unknown one reads as the picture not finishing', () => {
    for (const c of ['too_large', 'not_blend', 'unknown_form', 'unreadable', 'live_version', 'live_ambiguous', 'pairing_mismatch', 'invalid', 'timeout', 'renderer_error', 'unsupported_part', 'not_on_this_host'])
      expect(failureWords(c), c).toMatch(/\.$/)
    expect(failureWords('nope')).toBe('The picture didn’t finish.')
    expect(failureWords(null)).toBe('The picture didn’t finish.')
  })
})
