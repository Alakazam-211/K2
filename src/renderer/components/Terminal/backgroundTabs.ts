// The tab on screen first; the workspace's other tabs after it has painted
// (switch speed, 0.46.18).
//
// Rosson's design: only the pinned Chat is kept warm across workspaces;
// once you are on an agent's page, the OTHER tabs of that workspace load
// behind the scenes so they are ready when you click them. They used to
// load in the same commit as the tab on screen: every hidden tab's tree
// mounted at once, and each hidden terminal POSTed its spawn alongside the
// visible one's, competing for the daemon and for a remote server's request
// slots before the first frame.
//
// Now a column mounts its active tab at once and the others after the next
// paint plus BACKGROUND_TAB_DELAY_MS, in a React transition (low priority,
// interruptible). A tab you click before then mounts at once (it is the
// active tab). Once loaded a tab stays mounted (the retained view) until
// the workspace leaves the window. `K5_DEFER_TABS_OFF = '1'` restores the
// old all-at-once mount.

import { startTransition, useEffect, useState } from 'react'
import { readDebugKey } from '@/lib/debug-keys'

/** After the first paint, how long the tab on screen has the wire to
 *  itself before the background tabs load. */
export const BACKGROUND_TAB_DELAY_MS = 150

export function deferTabsDisabled(): boolean {
  return readDebugKey('DEFER_TABS_OFF') === '1'
}

interface Loaded {
  /** The workspace these ids belong to (a switch starts over). */
  key: string | null
  ids: ReadonlySet<string>
}

const NONE: ReadonlySet<string> = new Set()

/**
 * Which tabs of a column to mount now. `workspaceKey` is the workspace on
 * screen: when it changes, only the active tab mounts at once again.
 */
export function useBackgroundTabLoad(
  tabIds: readonly string[],
  activeTabId: string | null,
  workspaceKey: string | null,
): (tabId: string) => boolean {
  const off = deferTabsDisabled()
  const [loaded, setLoaded] = useState<Loaded>({ key: workspaceKey, ids: NONE })
  const ids = loaded.key === workspaceKey ? loaded.ids : NONE
  const idsKey = tabIds.join('\u0000')
  const pending = !off && tabIds.some((id) => id !== activeTabId && !ids.has(id))

  // The active tab, once shown, stays loaded after you switch away from it.
  useEffect(() => {
    if (off || !activeTabId || ids.has(activeTabId)) return
    setLoaded((prev) => {
      const base = prev.key === workspaceKey ? prev.ids : NONE
      if (base.has(activeTabId)) return prev
      const next = new Set(base)
      next.add(activeTabId)
      return { key: workspaceKey, ids: next }
    })
  }, [off, activeTabId, ids, workspaceKey])

  // The rest after the next paint, at low priority.
  useEffect(() => {
    if (!pending) return
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | null = null
    const afterPaint = (): void => {
      timer = setTimeout(() => {
        if (cancelled) return
        startTransition(() => {
          setLoaded((prev) => {
            const base = prev.key === workspaceKey ? prev.ids : NONE
            const next = new Set(base)
            for (const id of idsKey.split('\u0000')) if (id) next.add(id)
            return { key: workspaceKey, ids: next }
          })
        })
      }, BACKGROUND_TAB_DELAY_MS)
    }
    const raf =
      typeof requestAnimationFrame === 'function' ? requestAnimationFrame(afterPaint) : null
    if (raf === null) afterPaint()
    return () => {
      cancelled = true
      if (raf !== null && typeof cancelAnimationFrame === 'function') cancelAnimationFrame(raf)
      if (timer) clearTimeout(timer)
    }
  }, [pending, idsKey, workspaceKey])

  return (tabId: string) => off || tabId === activeTabId || ids.has(tabId)
}
