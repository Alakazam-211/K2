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
import type { HomeRow } from '@/stores/homes'

export type OpenResult = 'selected' | 'switching' | 'not-found' | 'unknown-server' | 'web-remote'

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
    useProjectsStore.getState().setActiveProject(ws.id)
    return 'selected'
  }

  if (isWebClient()) return 'web-remote'

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
