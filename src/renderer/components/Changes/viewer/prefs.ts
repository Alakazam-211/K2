// Per-viewer reading preferences (this window only): word wrap and
// collapse unchanged lines. They are how one person likes to read diffs,
// not repo truth, so they stay in the client. Inline vs split is C2's
// (saved in the tab layout) and arrives as the `mode` prop.

import { useCallback, useState } from 'react'

const KEY = 'k2.changesDiff.prefs'

export interface DiffPrefs {
  wrap: boolean
  collapse: boolean
}

export const DEFAULT_DIFF_PREFS: DiffPrefs = { wrap: false, collapse: true }

export function parseDiffPrefs(raw: string | null): DiffPrefs {
  if (!raw) return DEFAULT_DIFF_PREFS
  let v: unknown
  try {
    v = JSON.parse(raw)
  } catch {
    return DEFAULT_DIFF_PREFS
  }
  if (!v || typeof v !== 'object') return DEFAULT_DIFF_PREFS
  const o = v as Record<string, unknown>
  return {
    wrap: typeof o.wrap === 'boolean' ? o.wrap : DEFAULT_DIFF_PREFS.wrap,
    collapse: typeof o.collapse === 'boolean' ? o.collapse : DEFAULT_DIFF_PREFS.collapse,
  }
}

function readStored(): DiffPrefs {
  try {
    return parseDiffPrefs(window.localStorage.getItem(KEY))
  } catch {
    // Storage blocked (private window, preview): defaults are fine.
    return DEFAULT_DIFF_PREFS
  }
}

function writeStored(p: DiffPrefs): void {
  try {
    window.localStorage.setItem(KEY, JSON.stringify(p))
  } catch {
    // Storage blocked: the choice lasts until the viewer unmounts.
  }
}

export type DiffPrefsState = DiffPrefs & { set: (patch: Partial<DiffPrefs>) => void }

export function useDiffPrefs(): DiffPrefsState {
  const [prefs, setPrefs] = useState<DiffPrefs>(readStored)
  const set = useCallback((patch: Partial<DiffPrefs>) => {
    setPrefs((prev) => {
      const next = { ...prev, ...patch }
      writeStored(next)
      return next
    })
  }, [])
  return { ...prefs, set }
}
