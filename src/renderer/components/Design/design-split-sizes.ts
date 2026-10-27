// Design tab Chat | page split width, per WINDOW and per room (DD6, D-A8).
// Same storage rule as `column-sizes.ts` / `window-chrome.ts`
// (`k2.windowChrome.<…>.<label>`): localStorage in the desktop app,
// sessionStorage on the hosted web client, keyed by the Tauri window label
// so two windows keep their own widths. One width per room, shared by every
// Design tab in it. Never daemon state: a view preference of this screen.

import { getWindowLabel } from '@/lib/window-chrome'
import { isWebClient } from '@/lib/is-web'

export const DESIGN_SPLIT_KEY_PREFIX = 'k2.windowChrome.design.'

/** `[chatFlex, pageFlex]` before any drag. */
export const DEFAULT_DESIGN_SPLIT: readonly [number, number] = Object.freeze([35, 65]) as readonly [number, number]

/** `<room>` → `[chatFlex, pageFlex]`. */
type SplitMap = Record<string, [number, number]>

function storage(): Storage | null {
  try {
    if (isWebClient()) return typeof sessionStorage === 'undefined' ? null : sessionStorage
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

function validSplit(value: unknown): value is [number, number] {
  return (
    Array.isArray(value) &&
    value.length === 2 &&
    value.every((v) => typeof v === 'number' && Number.isFinite(v) && v > 0)
  )
}

/** One Thread | content split store under its own key prefix. The Design
 *  tab uses `k2.windowChrome.design.`; the Mobile tab (prd-mobile-dev-v1
 *  M-A7) the same rules under `k2.windowChrome.mobile.`. */
export interface SplitSizes {
  key(label: string): string
  read(roomKey: string | null, label?: string): [number, number]
  write(roomKey: string, split: readonly number[], label?: string): void
}

export function createSplitSizes(prefix: string, fallback: readonly [number, number]): SplitSizes {
  const key = (label: string): string => `${prefix}${label}`
  const readMap = (label: string): SplitMap => {
    const s = storage()
    if (!s) return {}
    let raw: string | null = null
    try {
      raw = s.getItem(key(label))
    } catch {
      return {}
    }
    if (raw == null) return {}
    try {
      const parsed = JSON.parse(raw) as unknown
      if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) return {}
      return parsed as SplitMap
    } catch {
      return {}
    }
  }
  return {
    key,
    read(roomKey, label = getWindowLabel()) {
      if (!roomKey) return [fallback[0], fallback[1]]
      const saved = readMap(label)[roomKey]
      return validSplit(saved) ? [saved[0], saved[1]] : [fallback[0], fallback[1]]
    },
    write(roomKey, split, label = getWindowLabel()) {
      if (!roomKey || !validSplit(split)) return
      const s = storage()
      if (!s) return
      const map = readMap(label)
      map[roomKey] = [split[0], split[1]]
      try {
        s.setItem(key(label), JSON.stringify(map))
      } catch {
        // Private mode / quota — the width still holds until the view unmounts.
      }
    },
  }
}

const designSplit = createSplitSizes(DESIGN_SPLIT_KEY_PREFIX, DEFAULT_DESIGN_SPLIT)

export function designSplitKey(label: string): string {
  return designSplit.key(label)
}

/** This window's saved split for `roomKey`, or the default. */
export function readDesignSplit(roomKey: string | null, label = getWindowLabel()): [number, number] {
  return designSplit.read(roomKey, label)
}

/** Remember the split for `roomKey` in this window. Malformed input is not saved. */
export function writeDesignSplit(roomKey: string, split: readonly number[], label = getWindowLabel()): void {
  designSplit.write(roomKey, split, label)
}
