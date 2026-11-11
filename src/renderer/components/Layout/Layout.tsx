import { useRef, type ReactNode } from 'react'
import TopBar from '../TopBar/TopBar'
import type { TopBarRemoteServer } from '../TopBar/TopBarTitle'
import IconRail from '../Sidebar/IconRail'
import { Surface } from '../ui'
import { useSidebarDisplayWidth, useSidebarStore } from '../../stores/sidebar'
import { usePanelsStore } from '../../stores/panels'
import AnimatedDrawer from './AnimatedDrawer'
import { useSidebarFloorMotion } from './sidebar-floor-motion'

interface LayoutProps {
  /** Content for the primary sidebar (projects list) — shown when expanded */
  sidebar?: ReactNode
  /** Collapsed-sidebar rail. Defaults to the workspaces IconRail; Home
   *  passes its roster rail (same chrome, Home rows). */
  rail?: ReactNode
  /** Content for the left auxiliary panel */
  leftPanel?: ReactNode
  /** Content for the right auxiliary panel */
  rightPanel?: ReactNode
  /** Main content area (terminal) */
  children: ReactNode
  /** Project name shown in TopBar */
  projectName?: string
  /** Workspace name shown in TopBar */
  workspaceName?: string
  /** Home: the other server the shown agent is on (TopBar centre). */
  remoteServer?: TopBarRemoteServer
}

export default function Layout({
  sidebar,
  rail,
  leftPanel,
  rightPanel,
  children,
  projectName,
  workspaceName,
  remoteServer
}: LayoutProps): React.JSX.Element {
  // The saved width, raised to the setup checklist's floor while it shows.
  const sidebarWidth = useSidebarDisplayWidth()
  // Moves to and from that floor with the list's fold (not at boot or on a drag).
  const sidebarRef = useRef<HTMLDivElement>(null)
  useSidebarFloorMotion(sidebarRef, sidebarWidth)
  const isCollapsed = useSidebarStore((s) => s.isCollapsed)

  const leftPanelOpen = usePanelsStore((s) => s.leftPanelOpen)
  const rightPanelOpen = usePanelsStore((s) => s.rightPanelOpen)

  // App shell — ST3: window inset + canvas layer (both flush/0 in Terminal)
  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-[var(--color-bg-canvas)] p-[var(--inset-window)]">
      {/* TopBar */}
      <TopBar
        projectName={projectName}
        workspaceName={workspaceName}
        remoteServer={remoteServer}
      />

      {/* Content area — ST3: sidebar | panes gap (0 in Terminal) */}
      <div className="flex flex-1 overflow-hidden gap-[var(--gap-pane)]">
        {/* Primary sidebar: icon rail (always) + expanded panel (when not collapsed) */}
        {isCollapsed ? (
          rail ?? <IconRail />
        ) : (
          <>
            {/* The Sidebar's painted surface — Sidebar.tsx's own root is
                transparent; this wrapper is the chrome container Glass
                will paint with material. */}
            <Surface
              ref={sidebarRef}
              role2="surface" material="tile"
              bordered={false}
              data-nav-tile=""
              className="relative flex-shrink-0 overflow-y-auto border-r border-[var(--color-border)]"
              style={{ width: sidebarWidth }}
            >
              {sidebar}
            </Surface>
          </>
        )}

        {/* Left auxiliary panel (tabbed) */}
        {leftPanel && (
          <AnimatedDrawer open={leftPanelOpen} side="left">
            {leftPanel}
          </AnimatedDrawer>
        )}

        {/* Main content (terminal) */}
        <div className="flex-1 overflow-hidden" data-call-tile-area="">{children}</div>

        {/* Right auxiliary panel (tabbed) */}
        {rightPanel && (
          <AnimatedDrawer open={rightPanelOpen} side="right">
            {rightPanel}
          </AnimatedDrawer>
        )}
      </div>
    </div>
  )
}
