// The workbook's preview key (`liveWorkbookPreview`), kept for its callers.
// The switches for every live view, this one included, live in
// shared/preview-keys.ts (one list, one key per type); these are thin names
// for the workbook's entry there. The key is unchanged:
// `k2.liveWorkbookPreview.v1`, '1' on, anything else off, per computer.

import { readLivePreview, setLivePreview, switchFor, useLivePreview } from './shared/preview-keys'

export const LS_LIVE_WORKBOOK_PREVIEW = switchFor('workbook').storageKey

export const readLiveWorkbookPreview = (): boolean => readLivePreview('workbook')

export const setLiveWorkbookPreview = (on: boolean): void => setLivePreview('workbook', on)

/** The key, live: a change in any window on this computer re-renders. */
export const useLiveWorkbookPreview = (): boolean => useLivePreview('workbook')

/** A file the preview applies to: a workbook (.xlsx). */
export const isWorkbookPath = (path: string): boolean => /\.xlsx$/i.test(path)
