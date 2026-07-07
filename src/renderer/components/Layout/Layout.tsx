import { type ReactNode } from 'react'
import TopBar from '../TopBar/TopBar'
import IconRail from '../Sidebar/IconRail'
import { Surface } from '../ui'
import { useSidebarStore } from '../../stores/sidebar'
import { usePanelsStore } from '../../stores/panels'

interface LayoutProps {
  /** Content for the primary sidebar (projects list) — shown when expanded */
  sidebar?: ReactNode
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
}

export default function Layout({
  sidebar,
  leftPanel,
  rightPanel,
  children,
  projectName,
  workspaceName
}: LayoutProps): React.JSX.Element {
  const sidebarWidth = useSidebarStore((s) => s.width)
  const isCollapsed = useSidebarStore((s) => s.isCollapsed)
  const toggleSidebar = useSidebarStore((s) => s.toggle)

  const leftPanelOpen = usePanelsStore((s) => s.leftPanelOpen)
  const rightPanelOpen = usePanelsStore((s) => s.rightPanelOpen)
  const toggleLeftPanel = usePanelsStore((s) => s.toggleLeftPanel)
  const toggleRightPanel = usePanelsStore((s) => s.toggleRightPanel)

  // App shell — ST3: window inset + canvas layer (both flush/0 in Square)
  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-[var(--color-bg-canvas)] p-[var(--inset-window)]">
      {/* TopBar */}
      <TopBar
        projectName={projectName}
        workspaceName={workspaceName}
        primarySidebarVisible={!isCollapsed}
        leftPanelVisible={leftPanelOpen}
        rightPanelVisible={rightPanelOpen}
        onTogglePrimarySidebar={toggleSidebar}
        onToggleLeftPanel={toggleLeftPanel}
        onToggleRightPanel={toggleRightPanel}
      />

      {/* Content area — ST3: sidebar | panes gap (0 in Square) */}
      <div className="flex flex-1 overflow-hidden gap-[var(--gap-pane)]">
        {/* Primary sidebar: icon rail (always) + expanded panel (when not collapsed) */}
        {isCollapsed ? (
          <IconRail />
        ) : (
          <>
            {/* The Sidebar's painted surface — Sidebar.tsx's own root is
                transparent; this wrapper is the chrome container Glass
                will paint with material. */}
            <Surface
              role2="surface"
              bordered={false}
              className="relative flex-shrink-0 overflow-y-auto border-r border-[var(--color-border)]"
              style={{ width: sidebarWidth }}
            >
              {sidebar}
            </Surface>
          </>
        )}

        {/* Left auxiliary panel (tabbed) */}
        {leftPanelOpen && leftPanel && (
          <div className="flex-shrink-0 border-r border-[var(--color-border)]">
            {leftPanel}
          </div>
        )}

        {/* Main content (terminal) */}
        <div className="flex-1 overflow-hidden">{children}</div>

        {/* Right auxiliary panel (tabbed) */}
        {rightPanelOpen && rightPanel && (
          <div className="flex-shrink-0 border-l border-[var(--color-border)]">
            {rightPanel}
          </div>
        )}
      </div>
    </div>
  )
}
