import { useEffect, useState, useCallback } from 'react'
import { TOPBAR_HEIGHT } from '../../../shared/constants'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet } from '@/lib/daemon-cli'
import { titleBarDragOnMouseDown, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
import ServerSwitcher from './ServerSwitcher'
import PageTabs from './PageTabs'
import DesktopChromeLeft from './DesktopChromeLeft'
import DesktopChromeRight from './DesktopChromeRight'
import K2MarkButton from './K2MarkButton'
import TopBarUtilities from './TopBarUtilities'
import { Surface } from '@/components/ui'
import {
  topBarLeftClusterMinWidth,
  TRAFFIC_LIGHT_CLUSTER_GAP_PX,
} from '@/lib/desktop-chrome'

interface TopBarProps {
  projectName?: string
  projectPath?: string
  workspaceName?: string
  leftPanelVisible?: boolean
  rightPanelVisible?: boolean
  onToggleLeftPanel?: () => void
  onToggleRightPanel?: () => void
  onRunCommand?: (command: string) => void
}

export default function TopBar({
  projectName,
  projectPath,
  workspaceName,
  leftPanelVisible = false,
  rightPanelVisible = false,
  onToggleLeftPanel,
  onToggleRightPanel,
  onRunCommand
}: TopBarProps): React.JSX.Element {
  const [hasRun, setHasRun] = useState(false)

  useEffect(() => {
    if (!projectPath) {
      setHasRun(false)
      return
    }

    let cancelled = false
    daemonCliGet<{ hasRunCommand: boolean }>('project-config/has-run-command', { project: projectPath })
      .then((r) => {
        if (!cancelled) setHasRun(r.hasRunCommand)
      })
      .catch(() => {
        if (!cancelled) setHasRun(false)
      })

    return () => {
      cancelled = true
    }
  }, [projectPath])

  const handleRun = async (): Promise<void> => {
    if (!projectPath || !onRunCommand) return
    try {
      const result = await daemonCliGet<{ command: string }>('project-config/run-command', { project: projectPath })
      onRunCommand(result.command)
    } catch {
      // No run command configured
    }
  }
  const leftMinWidth = topBarLeftClusterMinWidth()

  return (
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
      {/* Left: spacer or Linux squares, logo, server, page tabs. */}
      <div
        className="flex items-center [&>*]:shrink-0"
        style={{ minWidth: leftMinWidth, gap: TRAFFIC_LIGHT_CLUSTER_GAP_PX }}
      >
        <DesktopChromeLeft />
        <K2MarkButton />
        {/* K2 Connect server switcher (This Mac / saved servers / add) */}
        <ServerSwitcher />
        {/* §6.0 — ⚙ | Agents | Projects | Tickets (settings is first). */}
        <PageTabs />
      </div>

      {/* Center: workspace + worktree name */}
      <div className="flex items-center gap-1.5 text-xs">
        {projectName ? (
          <>
            <span className="text-[var(--color-text-secondary)]">{projectName}</span>
            {workspaceName && (
              <>
                <span className="text-[var(--color-text-muted)]">/</span>
                <span className="text-[var(--color-text-primary)] font-medium">
                  {workspaceName}
                </span>
              </>
            )}
          </>
        ) : (
          <span className="text-[var(--color-text-muted)]">No workspace selected</span>
        )}
      </div>

      {/* Right: run button + panel toggles + window controls */}
      <DesktopChromeRight>
        <TopBarUtilities
          leading={
            hasRun ? (
              <button
                onClick={handleRun}
                className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[#4ec9b0] transition-colors no-drag"
                style={{
                  // @ts-expect-error -- Electron-specific CSS property
                  WebkitAppRegion: 'no-drag'
                }}
                title="Run workspace command"
              >
                <svg
                  width="12"
                  height="12"
                  viewBox="0 0 12 12"
                  fill="currentColor"
                  stroke="none"
                >
                  <polygon points="2,0 2,12 11,6" />
                </svg>
              </button>
            ) : null
          }
        >
          {/* Left panel toggle (opens panel to the left of terminal) */}
          <button
            onClick={onToggleLeftPanel}
            className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors no-drag"
            style={{
              // @ts-expect-error -- Electron-specific CSS property
              WebkitAppRegion: 'no-drag'
            }}
            title="Toggle left panel"
          >
            <svg
              width="14"
              height="14"
              viewBox="0 0 14 14"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.3"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              {leftPanelVisible ? (
                <>
                  <rect x="1" y="2" width="12" height="10" rx="0" />
                  <line x1="5.5" y1="2" x2="5.5" y2="12" />
                  <line x1="3" y1="5" x2="3" y2="9" strokeWidth="1.5" />
                </>
              ) : (
                <>
                  <rect x="1" y="2" width="12" height="10" rx="0" />
                  <line x1="5.5" y1="2" x2="5.5" y2="12" strokeDasharray="1.5 1.5" />
                </>
              )}
            </svg>
          </button>

          {/* Right panel toggle (opens panel to the right of terminal) */}
          <button
            onClick={onToggleRightPanel}
            className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors no-drag"
            style={{
              // @ts-expect-error -- Electron-specific CSS property
              WebkitAppRegion: 'no-drag'
            }}
            title="Toggle right panel"
          >
            <svg
              width="14"
              height="14"
              viewBox="0 0 14 14"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.3"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              {rightPanelVisible ? (
                <>
                  <rect x="1" y="2" width="12" height="10" rx="0" />
                  <line x1="8.5" y1="2" x2="8.5" y2="12" />
                  <line x1="11" y1="5" x2="11" y2="9" strokeWidth="1.5" />
                </>
              ) : (
                <>
                  <rect x="1" y="2" width="12" height="10" rx="0" />
                  <line x1="8.5" y1="2" x2="8.5" y2="12" strokeDasharray="1.5 1.5" />
                </>
              )}
            </svg>
          </button>
        </TopBarUtilities>
      </DesktopChromeRight>
    </Surface>
  )
}

// FeedbackTopBarButton (v0.40.26) was absorbed into the §6.0 page
// switcher — see PageTabs.tsx (the Feedback tab keeps its waiting-count
// badge and the event-wiring effect verbatim).
