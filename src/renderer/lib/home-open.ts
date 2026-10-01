// Home P1 (vs-live H15, Rosson go 2026-09-30) — opening a Home row.
//
//   - Row on the connected server: select that workspace. The page stays
//     Home — Home is the Agents shell, so the room opens right there.
//     No switch.
//   - Row on another saved server (or `local` while on a remote): switch
//     THIS window's server through the existing switcher path (`pickHost`:
//     silent with a token, auto-login with a remembered password, else the
//     normal full-screen sign-in), and select the workspace once that
//     server's list lands (lib/home-pending-select), then land on Home.
//     Homes never change.
//   - Web: there is no switcher, so another server's row has no Open.
//   - Home M4, "Remote rooms (preview)" on: a row on another server opens
//     that server's room in Home's main area, view-only, without switching
//     (`stores/home-rooms.ts`).

import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { usePageViewStore } from '@/stores/page-view'
import { useToastStore } from '@/stores/toast'
import { isWebClient } from '@/lib/is-web'
import {
  LOCAL_HOME_HOST,
  activeHomeHostKey,
  findWorkspaceForRow,
  parseHomeAddress,
  savedHostForKey,
} from '@/lib/home-address'
import { requestHostSelect } from '@/lib/home-pending-select'
import { remoteRoomsPreviewEnabled } from '@/lib/remote-rooms-preview'
import { homeRooms } from '@/stores/home-rooms'
import type { HomeRow } from '@/stores/homes'

export type OpenResult = 'selected' | 'room' | 'switching' | 'not-found' | 'unknown-server' | 'web-remote'

/** The switcher target for a row host key: `'local'`, a saved host, or
 *  null when the server is not saved on this client. */
export function switchTargetForHost(hostKey: string, hosts: ConnectHost[]): 'local' | ConnectHost | null {
  if (hostKey === LOCAL_HOME_HOST) return 'local'
  return savedHostForKey(hosts, hostKey)
}

export function openHomeRow(row: HomeRow): OpenResult {
  const parsed = parseHomeAddress(row.address)
  if (!parsed) return 'not-found'
  const hostState = useConnectHostStore.getState()
  const connectedKey = activeHomeHostKey(hostState.activeHost)

  if (parsed.host === connectedKey) {
    const ws = findWorkspaceForRow(useProjectsStore.getState().projects, row)
    if (!ws) {
      useToastStore.getState().addToast(`${row.label} is not on this server any more.`, 'warning')
      return 'not-found'
    }
    // A remote room on screen gives way to the window's own room.
    homeRooms.showPrimary()
    useProjectsStore.getState().setActiveProject(ws.id)
    return 'selected'
  }

  if (isWebClient()) return 'web-remote'

  // Home M4 (MS55): with "Remote rooms (preview)" on, the row opens as a
  // pinned room on ITS server in Home's main area. The window's server does
  // not change. Off: today's switch below.
  if (remoteRoomsPreviewEnabled()) {
    if (parsed.host !== LOCAL_HOME_HOST && !savedHostForKey(hostState.hosts, parsed.host)) {
      useToastStore
        .getState()
        .addToast(`${parsed.host} is not a saved server. Add it in Settings → Connections.`, 'warning')
      return 'unknown-server'
    }
    usePageViewStore.getState().setPage('home')
    void homeRooms.open(row, parsed.host)
    return 'room'
  }

  const target = switchTargetForHost(parsed.host, hostState.hosts)
  if (!target) {
    useToastStore
      .getState()
      .addToast(`${parsed.host} is not a saved server. Add it in Settings → Connections.`, 'warning')
    return 'unknown-server'
  }
  requestHostSelect(target === 'local' ? 'local' : target.id, row, () =>
    usePageViewStore.getState().setPage('home'),
  )
  hostState.pickHost(target)
  return 'switching'
}
