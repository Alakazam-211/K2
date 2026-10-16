import { type ReactNode } from 'react'
import TopBar from '../TopBar/TopBar'
import IconRail from '../Sidebar/IconRail'
import { Surface } from '../ui'
import { useSidebarStore } from '../../stores/sidebar'
import { usePanelsStore } from '../../stores/panels'
import { usePageViewStore } from '../../stores/page-view'
import { TAB_LABELS } from '../TabbedPanel/TabbedPanel'
import DrawerHandle from './DrawerHandle'
import AnimatedDrawer from './AnimatedDrawer'
import { DrawerTabBarTogglesContext, drawerHandlesOn, drawerTabBarTogglesOn } from './drawer-handles'

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
}

export default function Layout({
  sidebar,
  rail,
  leftPanel,
  rightPanel,
  children,
  projectName,
  workspaceName
}: LayoutProps): React.JSX.Element {
  const sidebarWidth = useSidebarStore((s) => s.width)
  const isCollapsed = useSidebarStore((s) => s.isCollapsed)

  const leftPanelOpen = usePanelsStore((s) => s.leftPanelOpen)
  const rightPanelOpen = usePanelsStore((s) => s.rightPanelOpen)
  const toggleLeftPanel = usePanelsStore((s) => s.toggleLeftPanel)
  const toggleRightPanel = usePanelsStore((s) => s.toggleRightPanel)
  const leftTabs = usePanelsStore((s) => s.leftPanelTabs)
  const rightTabs = usePanelsStore((s) => s.rightPanelTabs)
  // Drawer-handles experiment: on these pages the drawers open from edge
  // handles and the top bar drops its drawer buttons.
  const handles = usePageViewStore((s) => drawerHandlesOn(s.page))
  const leftHandle = handles && !!leftPanel && leftTabs.length > 0
  const rightHandle = handles && !!rightPanel && rightTabs.length > 0
  // Same experiment: » / « drawer toggles in the room and drawer tab bars.
  const tabBarToggles = usePageViewStore((s) => drawerTabBarTogglesOn(s.page))

  // App shell — ST3: window inset + canvas layer (both flush/0 in Square)
  return (
    <DrawerTabBarTogglesContext.Provider value={tabBarToggles}>
    <div className="flex h-full w-full flex-col overflow-hidden bg-[var(--color-bg-canvas)] p-[var(--inset-window)]">
      {/* TopBar */}
      <TopBar
        projectName={projectName}
        workspaceName={workspaceName}
        leftPanelVisible={leftPanelOpen}
        rightPanelVisible={rightPanelOpen}
        onToggleLeftPanel={toggleLeftPanel}
        onToggleRightPanel={toggleRightPanel}
        showDrawerToggles={!handles}
      />

      {/* Content area — ST3: sidebar | panes gap (0 in Square) */}
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
        {leftPanel && (
          <AnimatedDrawer open={leftPanelOpen} side="left">
            {leftPanel}
          </AnimatedDrawer>
        )}

        {/* Main content (terminal). The outer box does not clip, so an
            open drawer's handle can sit across the drawer border; the
            inner box clips the room. Same tree with or without handles,
            so switching page never remounts the room. */}
        <div className="relative min-w-0 flex-1">
          {leftHandle && (
            <DrawerHandle
              side="left"
              open={leftPanelOpen}
              label={leftTabs.map((t) => TAB_LABELS[t]).join(', ')}
            />
          )}
          <div className="h-full w-full overflow-hidden">{children}</div>
          {rightHandle && (
            <DrawerHandle
              side="right"
              open={rightPanelOpen}
              label={rightTabs.map((t) => TAB_LABELS[t]).join(', ')}
            />
          )}
        </div>

        {/* Right auxiliary panel (tabbed) */}
        {rightPanel && (
          <AnimatedDrawer open={rightPanelOpen} side="right">
            {rightPanel}
          </AnimatedDrawer>
        )}
      </div>
    </div>
    </DrawerTabBarTogglesContext.Provider>
  )
}
