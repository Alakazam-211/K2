import { type ReactNode } from 'react'
import { TOPBAR_HEIGHT } from '../../../shared/constants'
import { usePanelsStore } from '../../stores/panels'
import AnimatedDrawer from './AnimatedDrawer'
import DesktopChromeLeft from '@/components/TopBar/DesktopChromeLeft'
import { TRAFFIC_LIGHT_CLUSTER_GAP_PX } from '@/lib/desktop-chrome'
import DesktopChromeRight from '@/components/TopBar/DesktopChromeRight'
import K2MarkButton from '@/components/TopBar/K2MarkButton'
import TopBarUtilities from '@/components/TopBar/TopBarUtilities'
import { DrawerIcon } from '@/components/icons/K2Icons'
import ServerSwitcher from '@/components/TopBar/ServerSwitcher'
import { Surface } from '@/components/ui'
import { titleBarDragOnMouseDown, titleBarOnDoubleClick } from '@/lib/titlebar-drag'

interface FocusLayoutProps {
  children: ReactNode
  projectName?: string
  branchName?: string
  leftPanel?: ReactNode
  rightPanel?: ReactNode
}

export default function FocusLayout({
  children,
  projectName,
  branchName,
  leftPanel,
  rightPanel
}: FocusLayoutProps): React.JSX.Element {
  const leftPanelOpen = usePanelsStore((s) => s.leftPanelOpen)
  const rightPanelOpen = usePanelsStore((s) => s.rightPanelOpen)
  const toggleLeftPanel = usePanelsStore((s) => s.toggleLeftPanel)
  const toggleRightPanel = usePanelsStore((s) => s.toggleRightPanel)

  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-[var(--color-bg)]">
      {/* Top bar — no primary sidebar toggle, but has left/right panel toggles */}
      <Surface
        role2="surface"
        bordered={false}
        className="flex items-center justify-between border-b border-[var(--color-border)] px-3 select-none"
        onMouseDown={titleBarDragOnMouseDown}
        onDoubleClick={titleBarOnDoubleClick}
        style={{
          height: TOPBAR_HEIGHT,
          minHeight: TOPBAR_HEIGHT
        }}
      >
        <div className="flex items-center" style={{ gap: TRAFFIC_LIGHT_CLUSTER_GAP_PX }}>
          <DesktopChromeLeft />
          <K2MarkButton />
          <ServerSwitcher />
        </div>

        {/* Center: workspace name + branch */}
        <div className="flex items-center gap-1.5 text-xs">
          {projectName ? (
            <>
              <span className="text-[var(--color-text-primary)] font-medium">{projectName}</span>
              {branchName && (
                <>
                  <span className="text-[var(--color-text-muted)]">/</span>
                  <svg className="w-3 h-3 text-[var(--color-text-muted)]" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
                    <line x1="6" y1="3" x2="6" y2="15" />
                    <circle cx="14" cy="6" r="2" />
                    <circle cx="6" cy="15" r="0" />
                    <path d="M14 8a7 7 0 0 1-7 7" />
                  </svg>
                  <span className="text-[var(--color-text-muted)] font-mono">{branchName}</span>
                </>
              )}
            </>
          ) : (
            <span className="text-[var(--color-text-muted)]">Focus Window</span>
          )}
        </div>

        {/* Right: presence roster + timer + left/right panel toggles + window controls */}
        <DesktopChromeRight>
          <TopBarUtilities>
            <button
              onClick={toggleLeftPanel}
              className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
              style={{
                // @ts-expect-error -- Electron-specific CSS property
                WebkitAppRegion: 'no-drag'
              }}
              title="Toggle left panel"
            >
              <DrawerIcon side="left" open={leftPanelOpen} />
            </button>

            <button
              onClick={toggleRightPanel}
              className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
              style={{
                // @ts-expect-error -- Electron-specific CSS property
                WebkitAppRegion: 'no-drag'
              }}
              title="Toggle right panel"
            >
              <DrawerIcon side="right" open={rightPanelOpen} />
            </button>
          </TopBarUtilities>
        </DesktopChromeRight>
      </Surface>

      {/* Content area with optional left/right panels */}
      <div className="flex flex-1 overflow-hidden">
        {/* Left auxiliary panel */}
        {leftPanel && (
          <AnimatedDrawer open={leftPanelOpen} side="left">
            {leftPanel}
          </AnimatedDrawer>
        )}

        {/* Main content (terminal) */}
        <div className="flex-1 overflow-hidden">{children}</div>

        {/* Right auxiliary panel */}
        {rightPanel && (
          <AnimatedDrawer open={rightPanelOpen} side="right">
            {rightPanel}
          </AnimatedDrawer>
        )}
      </div>
    </div>
  )
}
