// Layout loading for the tabs store (speed pass P4): the full layouts map is
// not part of the first load, and a layout this window does not hold is read
// before anything is written over it.

import { EMPTY_SERIALIZED_LAYOUT } from '@/lib/layout-merge'
import type { SerializedLayout } from './tabs'

/** One workspace's stored layout, as the tabs store reads and saves it. */
export interface StoredLayoutIO {
  /** `workspace-layouts/load?with_revision=1`. */
  fetch(): Promise<{ layout: SerializedLayout | null; revision: number | undefined }>
  /** `workspace-layouts/save` on `baseRevision` (none from an older daemon). */
  save(layout: SerializedLayout, baseRevision: number | undefined): Promise<{ revision?: number } | undefined>
  /** The save was refused because the layout changed since it was read (409). */
  isConflict(err: unknown): boolean
  maxAttempts: number
}

/** Read the stored layout with its revision, `change` it, save it on that
 *  base; a 409 reads again. `change` returns null to save nothing. Resolves
 *  to the saved layout and its new revision, or null. Throws when the layout
 *  kept changing for every attempt, or on any other error. */
export async function saveOnStoredLayout(
  io: StoredLayoutIO,
  change: (stored: SerializedLayout) => SerializedLayout | null,
): Promise<{ layout: SerializedLayout; revision: number | undefined } | null> {
  for (let attempt = 0; attempt < io.maxAttempts; attempt++) {
    const fetched = await io.fetch()
    const next = change(fetched.layout ?? EMPTY_SERIALIZED_LAYOUT)
    if (!next) return null
    try {
      const res = await io.save(next, fetched.revision)
      return { layout: next, revision: typeof res?.revision === 'number' ? res.revision : undefined }
    } catch (err) {
      if (io.isConflict(err)) continue
      throw err
    }
  }
  throw new Error('the layout kept changing elsewhere')
}

/** `stored` with `alone`'s tabs after its own; `alone` when it has none. */
export function withTabsAppended(stored: SerializedLayout, alone: SerializedLayout): SerializedLayout {
  return stored.tabs?.length ? { ...stored, tabs: [...stored.tabs, ...(alone.tabs ?? [])] } : alone
}

/** The background `load-all` of one room: asked for once the projects list
 *  has landed, run after the caller's turn, at most one waiting at a time.
 *  `wanted` says whether it was asked for on this server (the reconnect
 *  retry runs only then). */
export function createBackgroundLayoutsLoad(load: () => Promise<void>, loaded: () => boolean) {
  let wanted = false
  let scheduled = false
  return {
    request(): void {
      wanted = true
      if (loaded() || scheduled) return
      scheduled = true
      setTimeout(() => {
        void load().finally(() => {
          scheduled = false
        })
      }, 0)
    },
    wanted: (): boolean => wanted,
    /** A server switch: the new server's load waits to be asked for. */
    reset(): void {
      wanted = false
      scheduled = false
    },
  }
}
