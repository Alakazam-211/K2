// The owner's key for the live workbook preview (`liveWorkbookPreview`), off
// by default. With it on, an .xlsx file tab offers "Watch live (preview)",
// which plays K2's sample workbook (never the file) in the live view.
//
// Per computer, in this window's storage (`k2.liveWorkbookPreview.v1`: '1' on,
// anything else off), like the other view preferences; every window on this
// computer shares it through the `storage` event. It is a renderer key only:
// turning it on from the CLI (`k2 settings set liveWorkbookPreview true`)
// needs a daemon setting, which this change doesn't add.

import { useSyncExternalStore } from 'react'

export const LS_LIVE_WORKBOOK_PREVIEW = 'k2.liveWorkbookPreview.v1'

const listeners = new Set<() => void>()

/** Blocked storage holds the switch for this window only. */
let memory: boolean | null = null

export function readLiveWorkbookPreview(): boolean {
  try {
    return window.localStorage.getItem(LS_LIVE_WORKBOOK_PREVIEW) === '1'
  } catch {
    return memory === true
  }
}

export function setLiveWorkbookPreview(on: boolean): void {
  try {
    if (on) window.localStorage.setItem(LS_LIVE_WORKBOOK_PREVIEW, '1')
    else window.localStorage.removeItem(LS_LIVE_WORKBOOK_PREVIEW)
  } catch {
    memory = on
  }
  for (const l of listeners) l()
}

function subscribe(cb: () => void): () => void {
  listeners.add(cb)
  const onStorage = (e: StorageEvent): void => {
    if (e.key === LS_LIVE_WORKBOOK_PREVIEW || e.key === null) cb()
  }
  window.addEventListener('storage', onStorage)
  return () => {
    listeners.delete(cb)
    window.removeEventListener('storage', onStorage)
  }
}

/** The key, live: a change in any window on this computer re-renders. */
export function useLiveWorkbookPreview(): boolean {
  return useSyncExternalStore(subscribe, readLiveWorkbookPreview, () => false)
}

/** A file the preview applies to: a workbook (.xlsx). */
export const isWorkbookPath = (path: string): boolean => /\.xlsx$/i.test(path)
