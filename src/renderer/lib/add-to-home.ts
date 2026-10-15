// Putting an agent on a Home — the ONE way a row is added to `k2.homes.v1`.
//
// Two doors use it:
//   - Home's own Add Agent picker (components/Home/HomeAddPanels.tsx), for
//     the connected server and for any saved server;
//   - the Agents sidebar's right-click "Add to Home" (Sidebar.tsx), for an
//     agent on the connected server.
//
// Both write the same row shape (`handle::host`, the workspace id as a
// rename hint, the display name) through `useHomesStore.addRow`, and cache
// the agent's picture the same way. Homes are per-device view state
// (stores/homes.ts): no daemon call, no `fetchProjects()`.

import { useHomesStore, type Home } from '@/stores/homes'
import { homeAddress, workspaceHandle } from '@/lib/home-address'
import { putHomeAvatarOnAdd } from '@/lib/home-avatars'
import { useToastStore } from '@/stores/toast'
import { useAddToHomeDialogStore } from '@/stores/add-to-home-dialog'

/** A workspace as a server's `projects/list` sends it (the fields a Home
 *  row reads; the wire has more). */
export interface ListedWorkspace {
  id: string
  name: string
  handle?: string | null
  path?: string | null
  color?: string | null
  iconUrl?: string | null
}

/** Add `w` (on server `hostKey`) to `home`; returns the row's address
 *  (null: no handle, so it can't be addressed). A row already on that Home
 *  is left as it is (`addRow` dedupes per Home). */
export function addWorkspaceToHome(home: Home, hostKey: string, w: ListedWorkspace): string | null {
  const h = workspaceHandle(w)
  if (!h) return null
  const address = homeAddress(h, hostKey)
  useHomesStore.getState().addRow(home.id, {
    address,
    workspaceId: w.id,
    label: w.name,
  })
  // Cache the row's picture from this listing right away (picker-and-remote-avatars S4).
  putHomeAvatarOnAdd(hostKey, address, w)
  return address
}

/** The Home row address for `w` on `hostKey`, or null (no handle). */
export function homeAddressFor(hostKey: string, w: Pick<ListedWorkspace, 'handle' | 'name'>): string | null {
  const h = workspaceHandle(w)
  return h ? homeAddress(h, hostKey) : null
}

/** Is `address` already a row on `home`? */
export function homeHasAddress(home: Home, address: string): boolean {
  const a = address.trim().toLowerCase()
  return home.rows.some((r) => r.address === a)
}

export type AddToHomeOutcome =
  | { kind: 'added'; homeId: string }
  | { kind: 'already'; homeId: string }
  | { kind: 'pick' }
  | { kind: 'no-handle' }

/**
 * The sidebar's "Add to Home". `hostKey` is the server the agent lives on
 * (the connected one, `activeHomeHostKey(activeHost)`), captured at the
 * click so a server switch can't move the row to another host.
 *
 * One Home: add straight away (or say it's already there) with a toast.
 * Several: open the pick-a-Home dialog. There is never zero Homes — the
 * store seeds one "Home" on first run (in memory if storage fails) and
 * refuses to delete the last one — so there is no "create one" branch.
 */
export function addAgentToHomeFromMenu(hostKey: string, w: ListedWorkspace): AddToHomeOutcome {
  const address = homeAddressFor(hostKey, w)
  const toast = useToastStore.getState().addToast
  if (!address) {
    toast(`${w.name} has no address yet, so it can't go on a Home.`, 'error')
    return { kind: 'no-handle' }
  }
  const { homes } = useHomesStore.getState()
  if (homes.length === 1) {
    const home = homes[0]
    if (homeHasAddress(home, address)) {
      toast(`Already on ${home.name}`, 'info', 3000)
      return { kind: 'already', homeId: home.id }
    }
    addWorkspaceToHome(home, hostKey, w)
    toast(`Added to ${home.name}`, 'success', 3000)
    return { kind: 'added', homeId: home.id }
  }
  useAddToHomeDialogStore.getState().open({ hostKey, workspace: w })
  return { kind: 'pick' }
}

/** Toast text after the dialog adds to `names` (one or more Homes). */
export function addedToHomesMessage(names: string[]): string {
  if (names.length === 1) return `Added to ${names[0]}`
  if (names.length === 2) return `Added to ${names[0]} and ${names[1]}`
  return `Added to ${names.length} Homes`
}
