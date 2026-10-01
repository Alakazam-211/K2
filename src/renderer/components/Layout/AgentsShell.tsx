// The Agents page shell — top bar, sidebar, drawers, and the workspace
// room (tab strip, panes, Thread overlay, Browser tabs, file tabs). Home
// is this SAME shell (prd-home-v1 H2): only the sidebar's list differs —
// the Home roster instead of every workspace on the connected server.
// The room in the main area is the window's active workspace on both
// pages, mounted once here, so Home keeps matching Agents as Agents
// changes.
//
// On Home the room shows only while the active workspace is a row of the
// selected Home on the connected server. Otherwise the main area shows
// the Agents empty state with Home wording, and the room stays mounted
// but hidden (like under a full-page overlay: PageLive is false then, so
// its sockets wind down the same way).

import Layout from './Layout'
import Sidebar from '@/components/Sidebar/Sidebar'
import { TerminalArea } from '@/components/Terminal/TerminalArea'
import HomeSidebar from '@/components/Home/HomeSidebar'
import HomeIconRail from '@/components/Home/HomeIconRail'
import { useHomeRoomSelected } from '@/components/Home/home-room'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { usePageViewStore } from '@/stores/page-view'
import { LeftPanelContent, RightPanelContent } from './WorkspaceDrawers'

interface ShellProject {
  name: string
  path: string
}
interface ShellWorkspace {
  name: string
  worktreePath?: string | null
}

/** The main area with no room — one look for both pages. */
export function RoomEmptyState({ hint }: { hint: string }): React.JSX.Element {
  return (
    <div
      className="relative flex h-full w-full flex-1 items-center justify-center overflow-hidden"
      data-toast-host="workspace"
      data-room-empty=""
    >
      <div className="text-center">
        <h2 className="text-lg font-medium text-[var(--color-text-muted)]">K2</h2>
        <p className="text-xs text-[var(--color-text-muted)] mt-2 opacity-60">{hint}</p>
      </div>
    </div>
  )
}

/** True when the room (not the empty state) is on screen for this page. */
export function useRoomShown(): boolean {
  const page = usePageViewStore((s) => s.page)
  const homeRoom = useHomeRoomSelected()
  return page === 'home' ? homeRoom : true
}

export default function AgentsShell({
  activeProject,
  activeWorkspace,
  cwd,
}: {
  activeProject: ShellProject | undefined
  activeWorkspace: ShellWorkspace | undefined
  cwd: string
}): React.JSX.Element {
  const onHome = usePageViewStore((s) => s.page === 'home')
  const homeEmpty = useHomesStore((s) => selectedHome(s).rows.length === 0)
  const roomShown = useRoomShown()
  const hasRoom = !!(activeProject && activeWorkspace)
  const rootPath = activeWorkspace?.worktreePath ?? activeProject?.path
  const emptyHint = onHome
    ? homeEmpty
      ? 'Add an agent to get started'
      : 'Pick an agent on this Home'
    : 'Add a workspace to get started'

  return (
    <Layout
      sidebar={onHome ? <HomeSidebar /> : <Sidebar />}
      rail={onHome ? <HomeIconRail /> : undefined}
      leftPanel={roomShown ? <LeftPanelContent rootPath={rootPath} /> : undefined}
      rightPanel={roomShown ? <RightPanelContent rootPath={rootPath} /> : undefined}
      projectName={roomShown ? activeProject?.name : undefined}
      workspaceName={roomShown ? activeWorkspace?.name : undefined}
    >
      {hasRoom ? (
        <>
          <div
            className="h-full w-full"
            style={roomShown ? undefined : { display: 'none' }}
            aria-hidden={roomShown ? undefined : true}
            data-room-area=""
          >
            <TerminalArea cwd={cwd} />
          </div>
          {!roomShown && <RoomEmptyState hint={emptyHint} />}
        </>
      ) : (
        <RoomEmptyState hint={emptyHint} />
      )}
    </Layout>
  )
}
