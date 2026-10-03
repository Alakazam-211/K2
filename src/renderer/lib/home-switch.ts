// Home — switching THIS window to a row's server (H15 / P1.1), shared by
// the row click with the setting off, the context menu's "Switch to its
// server and open", and the 0.43.2 floor fallback (prd-home-seamless-0432
// Z5, Z42). The caller hides any shown remote room first
// (`homeRooms.showPrimary()`); this module never imports the rooms store.

import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { usePageViewStore } from '@/stores/page-view'
import { useToastStore } from '@/stores/toast'
import { LOCAL_HOME_HOST, parseHomeAddress, savedHostForKey } from '@/lib/home-address'
import { requestHostSelect } from '@/lib/home-pending-select'
import { oldServerToastText } from '@/lib/home-room-floor'
import type { HomeRow } from '@/stores/homes'

/** The switcher target for a row host key: `'local'`, a saved host, or
 *  null when the server is not saved on this client. */
export function switchTargetForHost(hostKey: string, hosts: ConnectHost[]): 'local' | ConnectHost | null {
  if (hostKey === LOCAL_HOME_HOST) return 'local'
  return savedHostForKey(hosts, hostKey)
}

/** A server's label for copy: "This computer", the saved label, its
 *  hostname, or the key itself. */
export function serverLabelForKey(hostKey: string): string {
  if (hostKey === LOCAL_HOME_HOST) return 'This computer'
  const saved = savedHostForKey(useConnectHostStore.getState().hosts, hostKey)
  return saved ? saved.label || saved.hostname : hostKey
}

/** Switch this window to the row's server and select the row's workspace
 *  once that server's list lands, then land on Home. */
export function switchWindowToRow(row: HomeRow): 'switching' | 'unknown-server' | 'not-found' {
  const parsed = parseHomeAddress(row.address)
  if (!parsed) return 'not-found'
  const state = useConnectHostStore.getState()
  const target = switchTargetForHost(parsed.host, state.hosts)
  if (!target) return 'unknown-server'
  requestHostSelect(target === 'local' ? 'local' : target.id, row, () =>
    usePageViewStore.getState().setPage('home'),
  )
  state.pickHost(target)
  return 'switching'
}

const toasted = new Set<string>()

/** Z5: one toast per server per session when a row below the floor
 *  switches the window instead of opening here. */
export function toastOldServerOnce(hostKey: string, version: string | null): void {
  if (toasted.has(hostKey)) return
  toasted.add(hostKey)
  useToastStore.getState().addToast(oldServerToastText(serverLabelForKey(hostKey), version), 'info')
}

/** Tests: forget which servers were toasted. */
export function __resetOldServerToastsForTests(): void {
  toasted.clear()
}
