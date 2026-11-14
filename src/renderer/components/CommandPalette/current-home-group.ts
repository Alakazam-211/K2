// The palette on Home: the agents of the current Home group rank first
// (K2 Design's Home nav), under "In <group>". Off Home, nothing ranks.

import { useMemo } from 'react'
import { selectedHome, useHomesStore, type Home } from '@/stores/homes'
import { useProjectsStore } from '@/stores/projects'
import { useConnectHostStore } from '@/stores/connect-host'
import { usePageViewStore } from '@/stores/page-view'
import { activeHomeHostKey, findWorkspaceForRow, parseHomeAddress } from '@/lib/home-address'

const NONE: ReadonlySet<string> = new Set()

/** The connected server's workspace ids that are rows of `home`. */
export function homeGroupProjectIds<P extends { id: string; handle?: string | null; name: string }>(
  home: Home,
  projects: P[],
  connectedKey: string,
): ReadonlySet<string> {
  const ids = new Set<string>()
  for (const row of home.rows) {
    if (parseHomeAddress(row.address)?.host !== connectedKey) continue
    const ws = findWorkspaceForRow(projects, row)
    if (ws) ids.add(ws.id)
  }
  return ids
}

/** On Home: the current group's name and its agents' ids on this server. */
export function useCurrentHomeGroup(): { name: string; ids: ReadonlySet<string> } {
  const onHome = usePageViewStore((s) => s.page === 'home')
  const home = useHomesStore(selectedHome)
  const projects = useProjectsStore((s) => s.projects)
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  return useMemo(
    () => ({ name: home.name, ids: onHome ? homeGroupProjectIds(home, projects, connectedKey) : NONE }),
    [onHome, home, projects, connectedKey],
  )
}
