// The owner's preview switches for the live views, one per type: workbooks,
// 3D scenes and timelines. Each is off by default; on, that type's file tab
// offers "Watch live (preview)", which plays K2's sample (never the file).
//
// Per computer, in this window's storage ('1' on, anything else off), like
// the other view preferences; every window on this computer shares them
// through the `storage` event. Renderer keys only: there is no daemon
// setting behind them yet.
//
// One list, in one place. Rosson may want one "Live views (preview)" switch
// instead of one per type: then LIVE_PREVIEW_SWITCHES becomes one entry with
// `types: ['workbook', 'scene', 'timeline']`, and nothing else changes (the
// Settings rows, the entry and the keys all read this list).

import { useSyncExternalStore } from 'react'

export type LivePreviewType = 'workbook' | 'scene' | 'timeline'

export interface LivePreviewSwitch {
  /** The switch's own id (the Settings search id is built from it). */
  id: string
  /** Its key in this window's storage. */
  storageKey: string
  /** The Settings row's name and its one line under it. */
  name: string
  line: string
  /** The types it turns on. */
  types: readonly LivePreviewType[]
  /** The Settings toggle's test id. */
  testId: string
}

export const LIVE_PREVIEW_SWITCHES: readonly LivePreviewSwitch[] = [
  {
    id: 'live-workbook-preview',
    storageKey: 'k2.liveWorkbookPreview.v1',
    name: 'Watch workbooks live (preview)',
    line: 'Workbook (.xlsx) tabs get a “Watch live” view: who’s here, what an agent holds and what it changed, cell by cell. For now it plays K2’s sample workbook, not your file. On this computer only.',
    types: ['workbook'],
    testId: 'live-workbook-preview-toggle',
  },
  {
    id: 'live-scene-preview',
    storageKey: 'k2.liveScenePreview.v1',
    name: 'Watch 3D scenes live (preview)',
    line: '3D scene tabs get a “Watch live” view: who’s here, whom you’re following and what changed. For now it plays K2’s sample scene, not your file. On this computer only.',
    types: ['scene'],
    testId: 'live-scene-preview-toggle',
  },
  {
    id: 'live-timeline-preview',
    storageKey: 'k2.liveTimelinePreview.v1',
    name: 'Watch timelines live (preview)',
    line: 'Timeline tabs get a “Watch live” view: who’s here, whom you’re following and what changed in the edit. For now it plays K2’s sample edit, not your file. On this computer only.',
    types: ['timeline'],
    testId: 'live-timeline-preview-toggle',
  },
]

/** Which files each type is for, by name. A timeline is every format the
 *  timeline pack reads (v0.1.0: OTIO, Final Cut Pro XML and Premiere
 *  projects); it writes back .otio only, so the others are read only live. */
const MATCH: Record<LivePreviewType, RegExp> = {
  workbook: /\.xlsx$/i,
  scene: /\.blend$/i,
  timeline: /\.(otio|fcpxml|prproj)$/i,
}

export function previewTypeFor(path: string): LivePreviewType | null {
  for (const type of Object.keys(MATCH) as LivePreviewType[]) if (MATCH[type].test(path)) return type
  return null
}

export function switchFor(type: LivePreviewType): LivePreviewSwitch {
  const s = LIVE_PREVIEW_SWITCHES.find((x) => x.types.includes(type))
  if (!s) throw new Error(`no preview switch lists ${type}`)
  return s
}

const listeners = new Set<() => void>()

/** Blocked storage holds the switches for this window only. */
const memory = new Map<string, boolean>()

function readKey(key: string): boolean {
  try {
    return window.localStorage.getItem(key) === '1'
  } catch {
    return memory.get(key) === true
  }
}

export function readPreviewSwitch(s: LivePreviewSwitch): boolean {
  return readKey(s.storageKey)
}

export function setPreviewSwitch(s: LivePreviewSwitch, on: boolean): void {
  try {
    if (on) window.localStorage.setItem(s.storageKey, '1')
    else window.localStorage.removeItem(s.storageKey)
  } catch {
    memory.set(s.storageKey, on)
  }
  for (const l of listeners) l()
}

export const readLivePreview = (type: LivePreviewType): boolean => readPreviewSwitch(switchFor(type))
export const setLivePreview = (type: LivePreviewType, on: boolean): void => setPreviewSwitch(switchFor(type), on)

/** Every switch change, in this window or another on this computer. */
export function subscribeLivePreview(cb: () => void): () => void {
  listeners.add(cb)
  const keys = new Set(LIVE_PREVIEW_SWITCHES.map((s) => s.storageKey))
  const onStorage = (e: StorageEvent): void => {
    if (e.key === null || keys.has(e.key)) cb()
  }
  window.addEventListener('storage', onStorage)
  return () => {
    listeners.delete(cb)
    window.removeEventListener('storage', onStorage)
  }
}

/** A type's switch, live: a change in any window on this computer re-renders. */
export function useLivePreview(type: LivePreviewType | null): boolean {
  return useSyncExternalStore(subscribeLivePreview, () => (type ? readLivePreview(type) : false), () => false)
}

/** One switch, live (the Settings row). */
export function usePreviewSwitch(s: LivePreviewSwitch): boolean {
  return useSyncExternalStore(subscribeLivePreview, () => readPreviewSwitch(s), () => false)
}
