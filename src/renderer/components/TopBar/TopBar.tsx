import { useEffect, useState, useCallback } from 'react'
import { TOPBAR_HEIGHT } from '../../../shared/constants'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useSettingsStore } from '@/stores/settings'
import { useTabsStore } from '@/stores/tabs'
import { useReviewQueueStore } from '@/stores/review-queue'
import { useRunningAgentsStore } from '@/stores/running-agents'
import { useActiveAgentsStore } from '@/stores/active-agents'
import TimerButton from '@/components/Timer/TimerButton'
import ServerSwitcher from './ServerSwitcher'

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
  return (
    <div
      className="flex items-center justify-between border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] px-3 select-none"
      data-tauri-drag-region
      onMouseDown={(e) => {
        // Only drag if clicking on the bar itself (not buttons)
        if ((e.target as HTMLElement).closest('button, input, select, .no-drag')) return
        getCurrentWindow().startDragging()
      }}
      style={{
        height: TOPBAR_HEIGHT,
        minHeight: TOPBAR_HEIGHT
      }}
    >
      {/* Left: traffic lights spacer + K2 branding + primary sidebar toggle.
          Top-bar wordmark only — the OS menu bar / app name (tauri.conf.json
          productName) stays "K2" until the 0.40.0 full rename. */}
      <div className="flex items-center gap-2" style={{ minWidth: 130 }}>
        {/* Traffic lights occupy ~70px on macOS */}
        <div style={{ width: 70 }} />
        {/* App name (in-app wordmark) */}
        <span className="text-[10px] font-bold tracking-widest text-[var(--color-text-muted)] uppercase flex-shrink-0">K2</span>
        {/* K2 Connect server switcher (This Mac / saved servers / add) */}
        <ServerSwitcher />
        {/* Primary sidebar toggle */}
        <button
          onClick={onTogglePrimarySidebar}
          className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
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
        {/* Settings gear */}
        <button
          onClick={() => useSettingsStore.getState().openSettings()}
          className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
          style={{
            // @ts-expect-error -- Electron-specific CSS property
            WebkitAppRegion: 'no-drag'
          }}
          title="Settings (⌘,)"
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="12" cy="12" r="3" />
            <path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
          </svg>
        </button>
        {/* Review Queue */}
        <ReviewQueueTopBarButton />
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

      {/* Right: run button + panel toggles */}
      <div className="flex items-center gap-1">
        {/* Run command button — only visible when project has a run command */}
        {hasRun && (
          <button
            onClick={handleRun}
            className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[#4ec9b0] transition-colors"
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

        {/* Timer */}
        <TimerButton />

        {/* Separator between timer and panel toggles */}
        <div className="w-px h-4 bg-[var(--color-border)] mx-1" />

        {/* Left panel toggle (opens panel to the left of terminal) */}
        <button
          onClick={onToggleLeftPanel}
          className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
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
          className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
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
    </div>
  )
}

function ReviewQueueTopBarButton(): React.JSX.Element | null {
  const agenticEnabled = useSettingsStore((s) => s.agenticSystemsEnabled)
  const pendingCount = useReviewQueueStore((s) => s.pendingCount)
  if (!agenticEnabled) return null
  return (
    <button
      onClick={() => useReviewQueueStore.getState().toggle()}
      className="relative flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
      style={{
        // @ts-expect-error -- Electron-specific CSS property
        WebkitAppRegion: 'no-drag'
      }}
      title="Review Queue"
    >
      <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinecap="round" strokeLinejoin="round">
        <path d="M9 11l3 3L22 4" />
        <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
      </svg>
      {pendingCount > 0 && (
        <span className="absolute -top-0.5 -right-0.5 min-w-[14px] h-[14px] flex items-center justify-center text-[8px] font-bold text-white bg-[var(--color-accent)] rounded-full px-0.5">
          {pendingCount > 99 ? '99+' : pendingCount}
        </span>
      )}
    </button>
  )
}

function RunningAgentsTopBarButton(): React.JSX.Element {
  const agentCount = useActiveAgentsStore((s) => s.getActiveAgentsList().length)
  return (
    <button
      onClick={() => useRunningAgentsStore.getState().toggle()}
      className="relative flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
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
        <span className="absolute -top-0.5 -right-0.5 min-w-[14px] h-[14px] flex items-center justify-center text-[8px] font-bold text-white bg-green-500 rounded-full px-0.5">
          {agentCount > 99 ? '99+' : agentCount}
        </span>
      )}
    </button>
  )
}

function NavButtons(): React.JSX.Element {
  const canBack = useTabsStore((s) => s.canGoBack())
  const canForward = useTabsStore((s) => s.canGoForward())

  return (
    <div className="flex items-center gap-0.5">
      <button
        onClick={() => useTabsStore.getState().goBack()}
        disabled={!canBack}
        className={`flex h-5 w-5 items-center justify-center transition-colors ${
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
        className={`flex h-5 w-5 items-center justify-center transition-colors ${
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
