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
//
// Home M4: with "Remote rooms (preview)" on, a Home row on another server
// opens THAT server's room in this same main area (`HomeRemoteRooms`), with
// the same drawers bound to it. The window's own room is then hidden, as
// for the empty state above.

import Layout from './Layout'
import Sidebar from '@/components/Sidebar/Sidebar'
import { TerminalArea } from '@/components/Terminal/TerminalArea'
import HomeSidebar from '@/components/Home/HomeSidebar'
import HomeIconRail from '@/components/Home/HomeIconRail'
import { useHomeRoomSelected } from '@/components/Home/home-room'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { usePageViewStore } from '@/stores/page-view'
import { LeftPanelContent, RightPanelContent } from './WorkspaceDrawers'
import { RoomProvider } from '@/components/Room/RoomContext'
import { primaryRoom } from '@/stores/room'
import { HomeRemoteRooms, RemoteRoomDrawer, useShownRemoteRoom } from '@/components/Home/room/HomeRemoteRooms'

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
  // Home M3 — Agents and Home on the connected server show the window's
  // PRIMARY room: today's tabs store on the window's server. Its drawers
  // and its main area get it explicitly; the main area registers it as the
  // shown (focused) room for window-level input (MS17). The sidebar is
  // window-level, outside the room.
  const room = primaryRoom()
  const onHome = usePageViewStore((s) => s.page === 'home')
  const homeEmpty = useHomesStore((s) => selectedHome(s).rows.length === 0)
  const remote = useShownRemoteRoom()
  const remoteRoom = remote?.room ?? null
  const roomShown = useRoomShown() && remote === null
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
      leftPanel={remoteRoom ? (
        <RemoteRoomDrawer key={remoteRoom.key} room={remoteRoom} side="left" />
      ) : roomShown ? (
        <RoomProvider room={room}>
          <LeftPanelContent rootPath={rootPath} />
        </RoomProvider>
      ) : undefined}
      rightPanel={remoteRoom ? (
        <RemoteRoomDrawer key={remoteRoom.key} room={remoteRoom} side="right" />
      ) : roomShown ? (
        <RoomProvider room={room}>
          <RightPanelContent rootPath={rootPath} />
        </RoomProvider>
      ) : undefined}
      projectName={remote ? remote.entry.label : roomShown ? activeProject?.name : undefined}
      workspaceName={remote ? undefined : roomShown ? activeWorkspace?.name : undefined}
    >
      <HomeRemoteRooms />
      <RoomProvider room={room} shown={remote === null}>
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
            {!roomShown && remote === null && <RoomEmptyState hint={emptyHint} />}
          </>
        ) : remote === null ? (
          <RoomEmptyState hint={emptyHint} />
        ) : null}
      </RoomProvider>
    </Layout>
  )
}
