// "Move to group…" on a Home row's right-click menu (agents, people and
// channels alike), in the phone's words. It opens a submenu of the person's
// other groups: never the one the row is in, and a group that already has
// the row is off with "Already in it". "New group" at the end makes the next
// "Group N" (as the Groups dialog's New group does) and moves the row into
// it. With one group the item is shown but off, saying why. A built-in group
// (such as People) is a target like any other.
//
// The move is one Homes store change (`moveRowToHome`), so Home sync sends
// it to the daemon like every other edit and the person's other devices
// follow.

import { nextGroupName, useHomesStore, type Home, type HomeRow } from '@/stores/homes'
import type { ContextMenuItemDef } from '@/stores/context-menu'
import { ALREADY_IN_GROUP_NOTE, MOVE_TO_GROUP_LABEL, NEW_GROUP_LABEL, ONLY_ONE_GROUP_TEXT } from './group-copy'

const TARGET_PREFIX = 'home-move:'
export const MOVE_TO_GROUP_ID = 'home-move'
export const MOVE_TO_GROUP_NEW_ID = 'home-move-new'

/** The submenu row id for moving into the group `homeId`. */
export const moveToGroupTargetId = (homeId: string): string => `${TARGET_PREFIX}${homeId}`

/** The "Move to group…" item for `row` on `from`, given every group. */
export function moveToGroupItem(homes: readonly Home[], from: Home, row: HomeRow): ContextMenuItemDef {
  const others = homes.filter((h) => h.id !== from.id)
  if (others.length === 0) {
    return { id: MOVE_TO_GROUP_ID, label: MOVE_TO_GROUP_LABEL, enabled: false, hint: ONLY_ONE_GROUP_TEXT }
  }
  const submenu: ContextMenuItemDef[] = others.map((h) =>
    h.rows.some((r) => r.address === row.address)
      ? { id: moveToGroupTargetId(h.id), label: h.name, enabled: false, hint: ALREADY_IN_GROUP_NOTE }
      : { id: moveToGroupTargetId(h.id), label: h.name },
  )
  submenu.push({ id: 'home-move-sep', label: '', type: 'separator' })
  submenu.push({ id: MOVE_TO_GROUP_NEW_ID, label: NEW_GROUP_LABEL })
  return { id: MOVE_TO_GROUP_ID, label: MOVE_TO_GROUP_LABEL, submenu }
}

/** Act on a menu pick when it is one of "Move to group…"'s rows. True when
 *  the row moved. */
export function runMoveToGroup(clicked: string | null, from: Home, row: HomeRow): boolean {
  if (!clicked) return false
  const st = useHomesStore.getState()
  if (clicked === MOVE_TO_GROUP_NEW_ID) {
    const id = st.createHome(nextGroupName(st.homes))
    return id !== null && useHomesStore.getState().moveRowToHome(from.id, id, row.address)
  }
  if (!clicked.startsWith(TARGET_PREFIX)) return false
  return st.moveRowToHome(from.id, clicked.slice(TARGET_PREFIX.length), row.address)
}
