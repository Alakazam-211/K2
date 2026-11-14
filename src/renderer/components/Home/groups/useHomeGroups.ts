// Home groups: the logic behind the group bar, the nav header and the edit
// card. It reads the Homes store (each Home is a group; the store syncs
// names, icons, pins, "Most recent at the top" and last sends across
// devices through the daemon) and the swipe state, and turns the bar's
// gestures into store calls. The components it feeds are presentational
// (`HomeGroupBar`, `GroupNameHeader`, `GroupEditModal`, `GroupList`, `GroupDetailPane`).
//
// K2 Design's contract (home-groups notes): groups {id, name, icon, order,
// pinned, recentFirst}, currentId, swipe {progress, direction} | null, and
// onSelect, onRename (an empty name is saved as the next unused "Group N"),
// onSetIcon, onCreate → id, onDelete (refused for the last group), onPin,
// onUnpin, onReorderPins, onSetRecentFirst, onReorderGroups. The swipe is
// not passed through here: the bar reads it from `useGroupSwipeStore`
// itself, so a swipe never re-renders the nav. Also here: each agent's last
// send by the person (`lastMessagedAt`), and this computer's recent icons.

import { useMemo, useState } from 'react'
import { cleanHomeName, nextGroupName, selectedHome, useHomesStore } from '@/stores/homes'
import { HOME_SWITCH_LIMIT, homeSwitchCombo } from '@/lib/home-shortcuts'
import { pinnedRows } from '@/lib/home-row-order'
import { readRecentGroupIcons } from '@/lib/group-icon-recents'
import type { GroupBarItem, GroupEditing, HomeGroupBarProps } from './HomeGroupBar'

export interface HomeGroups {
  bar: HomeGroupBarProps
  current: { id: string; name: string; icon: string | null; index: number; count: number }
  /** Open the All groups modal on the current group (the header's "…"). */
  editCurrent(): void
  /** The current group's agents (row address) → the person's last send to
   *  them (epoch ms), for "Most recent at the top". */
  lastMessagedAt: Readonly<Record<string, number>>
  /** This computer's recently picked icons, newest first (not synced). */
  recentIcons(): string[]
}

/** The group switch chord for the bar's tooltips (⌥⌘1–9). */
export function groupShortcutLabel(index: number): string | null {
  return index < HOME_SWITCH_LIMIT ? homeSwitchCombo(index + 1) : null
}

/** The name an empty rename saves: the next "Group N" no other group uses. */
export function emptyNameFallback(homes: readonly { id: string; name: string }[], id: string): string {
  return nextGroupName(homes.filter((h) => h.id !== id))
}

export function useHomeGroups(): HomeGroups {
  const homes = useHomesStore((s) => s.homes)
  const current = useHomesStore(selectedHome)
  const [editing, setEditing] = useState<GroupEditing | null>(null)

  const groups = useMemo<GroupBarItem[]>(
    () =>
      homes.map((h, order) => ({
        id: h.id,
        name: h.name,
        icon: h.icon ?? null,
        order,
        recentFirst: !!h.recentFirst,
        pinned: pinnedRows(h).map((r) => r.address),
        count: h.rows.length,
      })),
    [homes],
  )
  const index = Math.max(0, homes.findIndex((h) => h.id === current.id))
  const lastMessagedAt = useMemo(() => {
    const out: Record<string, number> = {}
    for (const r of current.rows) if (typeof r.lastMessagedAt === 'number') out[r.address] = r.lastMessagedAt
    return out
  }, [current])

  const bar: HomeGroupBarProps = {
    groups,
    currentId: current.id,
    editing,
    onEditingChange: setEditing,
    shortcutLabel: groupShortcutLabel,
    emptyNameFallback: (id) => emptyNameFallback(useHomesStore.getState().homes, id),
    onSelect: (id) => useHomesStore.getState().selectHome(id),
    onRename: (id, name) => {
      const st = useHomesStore.getState()
      st.renameHome(id, cleanHomeName(name) ?? emptyNameFallback(st.homes, id))
    },
    onSetIcon: (id, icon) => {
      useHomesStore.getState().setHomeIcon(id, icon)
    },
    onCreate: () => {
      const st = useHomesStore.getState()
      return st.createHome(nextGroupName(st.homes))
    },
    onDelete: (id) => {
      useHomesStore.getState().deleteHome(id)
    },
    onSetRecentFirst: (id, on) => {
      useHomesStore.getState().setRecentFirst(id, on)
    },
    onReorderGroups: (ids) => {
      useHomesStore.getState().reorderHomes(ids)
    },
    onPin: (groupId, rowId) => {
      useHomesStore.getState().pinRow(groupId, rowId)
    },
    onUnpin: (groupId, rowId) => {
      useHomesStore.getState().unpinRow(groupId, rowId)
    },
    onReorderPins: (groupId, rowIds) => {
      useHomesStore.getState().reorderPins(groupId, rowIds)
    },
  }

  return {
    bar,
    current: { id: current.id, name: current.name, icon: current.icon ?? null, index, count: current.rows.length },
    editCurrent: () => setEditing({ id: current.id, from: 'header' }),
    lastMessagedAt,
    recentIcons: readRecentGroupIcons,
  }
}
