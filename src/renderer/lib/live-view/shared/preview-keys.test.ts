// @vitest-environment jsdom
// The per-type preview switches: one list, one key each (the workbook's key
// unchanged), a file's type by its name, and every window kept in step.

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import {
  LIVE_PREVIEW_SWITCHES,
  previewTypeFor,
  readLivePreview,
  setLivePreview,
  subscribeLivePreview,
  switchFor,
} from './preview-keys'
import { LS_LIVE_WORKBOOK_PREVIEW, readLiveWorkbookPreview, setLiveWorkbookPreview } from '../preview-key'

beforeEach(() => window.localStorage.clear())
afterEach(() => window.localStorage.clear())

describe('the switch list', () => {
  it('has one switch per type, with the workbook key unchanged', () => {
    expect(LIVE_PREVIEW_SWITCHES.map((s) => s.types)).toEqual([['workbook'], ['scene'], ['timeline']])
    expect(switchFor('workbook').storageKey).toBe('k2.liveWorkbookPreview.v1')
    expect(switchFor('workbook').storageKey).toBe(LS_LIVE_WORKBOOK_PREVIEW)
    expect(new Set(LIVE_PREVIEW_SWITCHES.map((s) => s.storageKey)).size).toBe(3)
    expect(switchFor('scene').name).toBe('Watch 3D scenes live (preview)')
    expect(switchFor('timeline').name).toBe('Watch timelines live (preview)')
  })

  it('names no vendor app and no file extension in its words', () => {
    for (const s of LIVE_PREVIEW_SWITCHES.slice(1)) {
      expect(`${s.name} ${s.line}`).not.toMatch(/blender|otio|opentimeline|premiere|resolve|final cut|\.blend/i)
    }
  })
})

describe('a file’s type', () => {
  it('goes by the file name', () => {
    expect(previewTypeFor('/ws/Budget.XLSX')).toBe('workbook')
    expect(previewTypeFor('/ws/scenes/living-room.blend')).toBe('scene')
    expect(previewTypeFor('/ws/edits/rough-cut.otio')).toBe('timeline')
    expect(previewTypeFor('/ws/notes.csv')).toBeNull()
    expect(previewTypeFor('/ws/living-room.blend1')).toBeNull()
  })
})

describe('the keys', () => {
  it('are off by default and each gates only its own type', () => {
    expect(readLivePreview('scene')).toBe(false)
    setLivePreview('scene', true)
    expect(readLivePreview('scene')).toBe(true)
    expect(readLivePreview('timeline')).toBe(false)
    expect(readLivePreview('workbook')).toBe(false)
    setLivePreview('timeline', true)
    setLivePreview('scene', false)
    expect(readLivePreview('scene')).toBe(false)
    expect(readLivePreview('timeline')).toBe(true)
  })

  it('share storage with the workbook’s old helpers', () => {
    setLiveWorkbookPreview(true)
    expect(readLivePreview('workbook')).toBe(true)
    setLivePreview('workbook', false)
    expect(readLiveWorkbookPreview()).toBe(false)
  })

  it('tell every listener, here and from another window', () => {
    let calls = 0
    const off = subscribeLivePreview(() => calls++)
    setLivePreview('scene', true)
    expect(calls).toBe(1)
    window.dispatchEvent(new StorageEvent('storage', { key: switchFor('timeline').storageKey }))
    expect(calls).toBe(2)
    window.dispatchEvent(new StorageEvent('storage', { key: 'k2.somethingElse' }))
    expect(calls).toBe(2)
    off()
    setLivePreview('scene', false)
    expect(calls).toBe(2)
  })
})
