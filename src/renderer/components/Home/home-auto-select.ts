// Entering Home with nothing selected opens the top row (Rosson
// 2026-10-16: "When going to home, if there is no agent selected, select
// the top-most one").
//
// "Selected" is what the rows paint as active, in this order: an open Grok
// Bot chat (it covers the main area), a remote room on screen, else the
// window's active workspace when it is a row of this Home on the connected
// server. A selection that is not a row of this Home (the Agents page's
// agent, a room or chat from another Home) counts as none.
//
// The top row opens exactly as a click on it (and Cmd+1) does:
// `homeRowOpenableNow` then `openHomeRow` — the same selection store and
// room path, nothing more. An empty Home does nothing. On window open the
// answer may not be known yet (the server's list or the row's status still
// loading), so it waits for the stores, once per entry, and gives up after
// AUTO_SELECT_WAIT_MS or the moment something is selected. Picking another
// Home in the Home picker is an entry too (Rosson 2026-10-16).

import { useEffect } from 'react'
import { useHomesStore, selectedHome, type Home } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useHomeRoomsStore } from '@/stores/home-rooms'
import { useGrokBotsStore } from '@/stores/grokbots'
import { hostPool } from '@/lib/host-pool-instance'
import { activeHomeHostKey, findWorkspaceForRow, parseHomeAddress } from '@/lib/home-address'
import { grokBotIdFromAddress } from '@/lib/grokbots'
import { openHomeRow } from '@/lib/home-open'
import { homeRowOpenableNow } from './home-room'

/** How long an entry waits for the top row to become openable. */
export const AUTO_SELECT_WAIT_MS = 15_000

export type HomeSelection = 'selected' | 'none' | 'unknown'

/** Does this Home have a selected row right now? `unknown` while the
 *  connected server's list has not landed and could hold the selection. */
export function homeSelection(home: Home): HomeSelection {
  const grok = useGrokBotsStore.getState().open
  if (grok) {
    const inHome = home.rows.some((row) => {
      const id = grokBotIdFromAddress(row.address)
      return (
        id !== null &&
        id.toLowerCase() === grok.agentId.toLowerCase() &&
        parseHomeAddress(row.address)?.host === grok.hostKey
      )
    })
    return inHome ? 'selected' : 'none'
  }
  const shown = useHomeRoomsStore.getState().shown
  if (shown !== null) return home.rows.some((r) => r.address === shown) ? 'selected' : 'none'

  const { activeProjectId, projects } = useProjectsStore.getState()
  if (!activeProjectId) return 'none'
  const host = useConnectHostStore.getState()
  const connectedKey = activeHomeHostKey(host.activeHost)
  const connectedRows = home.rows.filter(
    (r) => !grokBotIdFromAddress(r.address) && parseHomeAddress(r.address)?.host === connectedKey,
  )
  if (connectedRows.length === 0) return 'none'
  if (host.connectionStatus !== 'connected' || projects.length === 0) return 'unknown'
  return connectedRows.some((r) => findWorkspaceForRow(projects, r)?.id === activeProjectId) ? 'selected' : 'none'
}

/** One step of an entry: true when the entry is settled (done). */
export function trySelectTopHomeRow(): boolean {
  const home = selectedHome(useHomesStore.getState())
  const top = home.rows[0]
  if (!top) return true
  const selection = homeSelection(home)
  if (selection === 'selected') return true
  if (selection === 'unknown') return false
  if (!homeRowOpenableNow(top)) return false
  openHomeRow(top)
  return true
}

/** Mounted with Home on screen (`HomeShellEffects`): each mount is an
 *  entry (tab, shortcut, a window opening on Home), and so is picking
 *  another Home in the Home picker. */
export function useSelectTopHomeRowOnEnter(): void {
  const homeId = useHomesStore((s) => selectedHome(s).id)
  useEffect(() => {
    if (trySelectTopHomeRow()) return
    let done = false
    const unsubs: Array<() => void> = []
    const stop = (): void => {
      done = true
      clearTimeout(timer)
      for (const u of unsubs.splice(0)) u()
    }
    const check = (): void => {
      if (!done && trySelectTopHomeRow()) stop()
    }
    const timer = setTimeout(stop, AUTO_SELECT_WAIT_MS)
    unsubs.push(
      useProjectsStore.subscribe(check),
      useConnectHostStore.subscribe(check),
      useHomesStore.subscribe(check),
      useHomeRoomsStore.subscribe(check),
      useGrokBotsStore.subscribe(check),
      hostPool.store.subscribe(check),
    )
    return stop
  }, [homeId])
}
