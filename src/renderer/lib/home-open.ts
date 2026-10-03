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
//   - "Open agents from other servers here" on (the default on macOS and
//     Linux from 0.43.2, `lib/remote-rooms-preview.ts`): a row on another
//     server opens that server's room in Home's main area without switching
//     (`stores/home-rooms.ts`). A server below the floor
//     (`lib/home-room-floor.ts`, Z5) still switches the window, with one
//     toast per server per session. A server the pool has not checked yet
//     opens a room; the floor is applied when the check lands (Z42).

import { useConnectHostStore } from '@/stores/connect-host'
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
import { remoteRoomsPreviewEnabled } from '@/lib/remote-rooms-preview'
import { homeRoomVerdict } from '@/lib/home-room-floor'
import { switchWindowToRow, toastOldServerOnce } from '@/lib/home-switch'
import { hostPool } from '@/lib/host-pool-instance'
import { homeRooms } from '@/stores/home-rooms'
import type { HomeRow } from '@/stores/homes'

export { switchTargetForHost } from '@/lib/home-switch'

export type OpenResult = 'selected' | 'room' | 'switching' | 'not-found' | 'unknown-server' | 'web-remote'

function unknownServer(host: string): OpenResult {
  useToastStore.getState().addToast(`${host} is not a saved server. Add it in Settings → Connections.`, 'warning')
  return 'unknown-server'
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

  if (parsed.host !== LOCAL_HOME_HOST && !savedHostForKey(hostState.hosts, parsed.host)) {
    return unknownServer(parsed.host)
  }

  // MS55 / Z1: with the setting on, the row opens as a pinned room on ITS
  // server in Home's main area. The window's server does not change.
  if (remoteRoomsPreviewEnabled()) {
    const boot = hostPool.entry(parsed.host)?.boot ?? null
    if (homeRoomVerdict(boot) === 'switch') {
      // Z5: below the floor (or no version) — switch, as with it off.
      homeRooms.showPrimary()
      toastOldServerOnce(parsed.host, boot?.version ?? null)
      return switchWindowToRow(row)
    }
    usePageViewStore.getState().setPage('home')
    void homeRooms.open(row, parsed.host)
    return 'room'
  }

  // Off: switch this window's server.
  return switchWindowToRow(row)
}
