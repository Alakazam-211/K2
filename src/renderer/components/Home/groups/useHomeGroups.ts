// Home groups: the logic behind the group bar, the nav header and the edit
// card. It reads the Homes store (each Home is a group; the store syncs
// names and icons across devices through the daemon) and the swipe progress,
// and turns the bar's gestures into store calls. The components it feeds
// are presentational (`HomeGroupBar`, `GroupNameHeader`, `GroupEditCard`).

import { useMemo, useState } from 'react'
import { nextGroupName, selectedHome, useHomesStore } from '@/stores/homes'
import { HOME_SWITCH_LIMIT, homeSwitchCombo } from '@/lib/home-shortcuts'
import { useGroupSwipeStore } from '@/hooks/useGroupSwipe'
import { pinnedRows } from '@/lib/home-row-order'
import type { GroupBarItem, GroupEditing, HomeGroupBarProps } from './HomeGroupBar'

export interface HomeGroups {
  bar: HomeGroupBarProps
  current: { id: string; name: string; icon: string | null; index: number; count: number }
  /** Open the current group's card against `anchor` (the header's "…"). */
  editCurrent(anchor: HTMLElement): void
}

/** The group switch chord for the bar's tooltips (⌥⌘1–9). */
export function groupShortcutLabel(index: number): string | null {
  return index < HOME_SWITCH_LIMIT ? homeSwitchCombo(index + 1) : null
}

export function useHomeGroups(): HomeGroups {
  const homes = useHomesStore((s) => s.homes)
  const current = useHomesStore(selectedHome)
  const swipe = useGroupSwipeStore((s) => s.swipe)
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
      })),
    [homes],
  )
  const index = Math.max(0, homes.findIndex((h) => h.id === current.id))

  const bar: HomeGroupBarProps = {
    groups,
    currentId: current.id,
    swipe,
    editing,
    onEditingChange: setEditing,
    shortcutLabel: groupShortcutLabel,
    onSelect: (id) => useHomesStore.getState().selectHome(id),
    onRename: (id, name) => {
      useHomesStore.getState().renameHome(id, name)
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
    editCurrent: (anchor) => setEditing({ id: current.id, anchor }),
  }
}
