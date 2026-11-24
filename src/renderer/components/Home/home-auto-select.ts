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
//
// Each group remembers the agent it last had on screen (Rosson 2026-11-24,
// lib/home-group-focus): an entry, and so coming back to a group by click,
// swipe or ⌥⌘1–9, opens that agent instead of the top row while it is
// still in the group and can open now. A group with nothing remembered, or
// whose remembered agent has left it, opens its top row as before.

import { useEffect } from 'react'
import { useHomesStore, selectedHome, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useHomeRoomsStore } from '@/stores/home-rooms'
import { useGrokBotsStore } from '@/stores/grokbots'
import { hostPool } from '@/lib/host-pool-instance'
import { activeHomeHostKey, findWorkspaceForRow, parseHomeAddress } from '@/lib/home-address'
import { grokBotIdFromAddress } from '@/lib/grokbots'
import { openHomeRow } from '@/lib/home-open'
import { homeRowOpenableNow } from './home-room'
import { applyWorkspaceSwitchFocus } from '@/lib/workspace-switch-focus'
import { rememberGroupFocus, rememberedGroupFocus } from '@/lib/home-group-focus'

/** How long an entry waits for the top row to become openable. */
export const AUTO_SELECT_WAIT_MS = 15_000

export type HomeSelection = 'selected' | 'none' | 'unknown'

/** The row of this Home that is selected right now: null when none is,
 *  `unknown` while the connected server's list has not landed and could
 *  hold the selection. */
export function selectedHomeRow(home: Home): HomeRow | null | 'unknown' {
  const grok = useGrokBotsStore.getState().open
  if (grok) {
    return (
      home.rows.find((row) => {
        const id = grokBotIdFromAddress(row.address)
        return (
          id !== null &&
          id.toLowerCase() === grok.agentId.toLowerCase() &&
          parseHomeAddress(row.address)?.host === grok.hostKey
        )
      }) ?? null
    )
  }
  const shown = useHomeRoomsStore.getState().shown
  if (shown !== null) return home.rows.find((r) => r.address === shown) ?? null

  const { activeProjectId, projects } = useProjectsStore.getState()
  if (!activeProjectId) return null
  const host = useConnectHostStore.getState()
  const connectedKey = activeHomeHostKey(host.activeHost)
  const connectedRows = home.rows.filter(
    (r) => !grokBotIdFromAddress(r.address) && parseHomeAddress(r.address)?.host === connectedKey,
  )
  if (connectedRows.length === 0) return null
  if (host.connectionStatus !== 'connected' || projects.length === 0) return 'unknown'
  return connectedRows.find((r) => findWorkspaceForRow(projects, r)?.id === activeProjectId) ?? null
}

/** Does this Home have a selected row right now? `unknown` while the
 *  connected server's list has not landed and could hold the selection. */
export function homeSelection(home: Home): HomeSelection {
  const row = selectedHomeRow(home)
  return row === 'unknown' ? 'unknown' : row ? 'selected' : 'none'
}

/** The agent row an entry opens: the group's remembered agent while it is
 *  still in the group and can open now, else the top agent row (people
 *  and channels never open by themselves). */
function entryRow(home: Home): HomeRow | undefined {
  const remembered = rememberedGroupFocus(home.id)
  const kept = remembered ? home.rows.find((r) => !r.kind && r.address === remembered) : undefined
  if (kept && homeRowOpenableNow(kept)) return kept
  return home.rows.find((r) => !r.kind)
}

/** One step of an entry: true when the entry is settled (done). */
export function trySelectTopHomeRow(): boolean {
  const home = selectedHome(useHomesStore.getState())
  const row = entryRow(home)
  if (!row) return true
  const selection = homeSelection(home)
  if (selection === 'selected') return true
  if (selection === 'unknown') return false
  if (!homeRowOpenableNow(row)) return false
  openHomeRow(row)
  return true
}

/** Remember the selected Home's agent on screen, if it is one of its agent
 *  rows (nothing of the group on screen leaves its memory as it was). */
function rememberSelectedRow(): void {
  const st = useHomesStore.getState()
  const home = selectedHome(st)
  const row = selectedHomeRow(home)
  if (!row || row === 'unknown' || row.kind) return
  rememberGroupFocus(
    home.id,
    row.address,
    st.homes.map((h) => h.id),
  )
}

/** Mounted with Home on screen: keeps each group's agent on screen
 *  (lib/home-group-focus), whichever way it was picked (click, rail,
 *  ⌘1–9, entry). */
function useRememberGroupFocus(): void {
  useEffect(() => {
    rememberSelectedRow()
    const unsubs = [
      useProjectsStore.subscribe(rememberSelectedRow),
      useConnectHostStore.subscribe(rememberSelectedRow),
      useHomesStore.subscribe(rememberSelectedRow),
      useHomeRoomsStore.subscribe(rememberSelectedRow),
      useGrokBotsStore.subscribe(rememberSelectedRow),
    ]
    return () => {
      for (const u of unsubs) u()
    }
  }, [])
}

/** Mounted with Home on screen (`HomeShellEffects`): each mount is an
 *  entry (tab, shortcut, a window opening on Home), and so is picking
 *  another Home in the Home picker. */
export function useSelectTopHomeRowOnEnter(): void {
  const homeId = useHomesStore((s) => selectedHome(s).id)
  useRememberGroupFocus()
  useEffect(() => {
    // Arriving on Home lands the caret per Settings → Workspaces / Agents → Workspace settings
    // ("auto-select"), Thread composer first (Rosson 2026-10-16). Opening
    // a row below re-applies for that row. A Grok Bot chat has no room.
    if (!useGrokBotsStore.getState().open) applyWorkspaceSwitchFocus()
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
