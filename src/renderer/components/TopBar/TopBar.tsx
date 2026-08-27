import { useEffect, useState, useCallback } from 'react'
import { TOPBAR_HEIGHT } from '../../../shared/constants'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet } from '@/lib/daemon-cli'
import { titleBarDragOnMouseDown, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
import { useTabsStore } from '@/stores/tabs'
import { useRunningAgentsStore } from '@/stores/running-agents'
import { useActiveAgentsStore } from '@/stores/active-agents'
import TimerButton from '@/components/Timer/TimerButton'
import K2NounsCheatSheet from '@/components/CheatSheet/K2NounsCheatSheet'
import PresenceRoster from '@/components/Presence/PresenceRoster'
import ModeToggle from '@/components/Presence/ModeToggle'
import ServerSwitcher from './ServerSwitcher'
import PageTabs from './PageTabs'
import DesktopChromeLeft from './DesktopChromeLeft'
import DesktopChromeRight from './DesktopChromeRight'
import { Surface } from '@/components/ui'
import {
  APP_MENU_BUTTON_MIN_WIDTH_PX,
  getDesktopChrome,
  TRAFFIC_LIGHT_SPACER_BASE_PX,
} from '@/lib/desktop-chrome'

interface TopBarProps {
  projectName?: string
  projectPath?: string
  workspaceName?: string
  primarySidebarVisible?: boolean
  leftPanelVisible?: boolean
  rightPanelVisible?: boolean
  onTogglePrimarySidebar?: () => void
  onToggleLeftPanel?: () => void
  onToggleRightPanel?: () => void
  onRunCommand?: (command: string) => void
}

export default function TopBar({
  projectName,
  projectPath,
  workspaceName,
  primarySidebarVisible = true,
  leftPanelVisible = false,
  rightPanelVisible = false,
  onTogglePrimarySidebar,
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
  const chrome = getDesktopChrome()
  const leftMinWidth = chrome.trafficLightSpacer
    ? TRAFFIC_LIGHT_SPACER_BASE_PX + 60
    : chrome.appMenuButton
      ? APP_MENU_BUTTON_MIN_WIDTH_PX + 60
      : undefined

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
      {/* Left: chrome spacer/Menu + K2 branding + sidebar toggle. */}
      <div
        className="flex items-center gap-2"
        style={{ minWidth: leftMinWidth }}
      >
        <DesktopChromeLeft />
        {/* App name (in-app wordmark) */}
        <span className="text-[10px] font-bold tracking-widest text-[var(--color-text-muted)] uppercase flex-shrink-0">K2</span>
        {/* K2 Connect server switcher (This Mac / saved servers / add) */}
        <ServerSwitcher />
        {/* §6.0 — ⚙ | Agents | Projects | Tickets (settings is first). */}
        <PageTabs />
        {/* Primary sidebar toggle */}
        <button
          onClick={onTogglePrimarySidebar}
          className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors no-drag"
          style={{
            // @ts-expect-error -- Electron-specific CSS property
            WebkitAppRegion: 'no-drag'
          }}
          title="Toggle workspaces sidebar"
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
            {primarySidebarVisible ? (
              <>
                <rect x="1" y="2" width="12" height="10" rx="0" />
                <line x1="5" y1="2" x2="5" y2="12" />
              </>
            ) : (
              <>
                <rect x="1" y="2" width="12" height="10" rx="0" />
                <line x1="5" y1="2" x2="5" y2="12" strokeDasharray="1.5 1.5" />
              </>
            )}
          </svg>
        </button>
        {/* Running Agents */}
        <RunningAgentsTopBarButton />
        {/* Back / Forward navigation */}
        <NavButtons />
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
        <div className="flex items-center gap-1">
          {/* Run command button — only visible when project has a run command */}
          {hasRun && (
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
          )}

          {/* Presence roster — who's connected to this daemon (hidden when
              alone or when the host predates the presence routes) */}
          <PresenceRoster />

          {/* Timer */}
          <TimerButton />

          <K2NounsCheatSheet />

          {/* Per-window viewer/claimer mode toggle */}
          <ModeToggle />

          {/* Separator between timer and panel toggles */}
          <div className="w-px h-4 bg-[var(--color-border)] mx-1" />

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
        </div>
      </DesktopChromeRight>
    </Surface>
  )
}

function RunningAgentsTopBarButton(): React.JSX.Element {
  const agentCount = useActiveAgentsStore((s) => s.getActiveAgentsList().length)
  return (
    <button
      onClick={() => useRunningAgentsStore.getState().toggle()}
      className="relative flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors no-drag"
      style={{
        // @ts-expect-error -- Electron-specific CSS property
        WebkitAppRegion: 'no-drag'
      }}
      title="Running Agents (⌘J)"
    >
      <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinecap="round" strokeLinejoin="round">
        <path d="M7 8L12 12L7 16" />
        <path d="M13 17H18" />
      </svg>
      {agentCount > 0 && (
        <span className="absolute -top-0.5 -right-0.5 min-w-[14px] h-[14px] flex items-center justify-center text-[8px] font-bold text-[var(--color-on-accent)] bg-[var(--color-status-ok)] rounded-full px-0.5">
          {agentCount > 99 ? '99+' : agentCount}
        </span>
      )}
    </button>
  )
}

// FeedbackTopBarButton (v0.40.26) was absorbed into the §6.0 page
// switcher — see PageTabs.tsx (the Feedback tab keeps its waiting-count
// badge and the event-wiring effect verbatim).

function NavButtons(): React.JSX.Element {
  const canBack = useTabsStore((s) => s.canGoBack())
  const canForward = useTabsStore((s) => s.canGoForward())

  return (
    <div className="flex items-center gap-0.5">
      <button
        onClick={() => useTabsStore.getState().goBack()}
        disabled={!canBack}
        className={`flex h-5 w-5 items-center justify-center transition-colors no-drag ${
          canBack
            ? 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)]'
            : 'text-[var(--color-text-muted)] opacity-30'
        }`}
        style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
        title="Go Back (⌘[)"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <polyline points="6 2 3 5 6 8" />
        </svg>
      </button>
      <button
        onClick={() => useTabsStore.getState().goForward()}
        disabled={!canForward}
        className={`flex h-5 w-5 items-center justify-center transition-colors no-drag ${
          canForward
            ? 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)]'
            : 'text-[var(--color-text-muted)] opacity-30'
        }`}
        style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
        title="Go Forward (⌘])"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <polyline points="4 2 7 5 4 8" />
        </svg>
      </button>
    </div>
  )
}
