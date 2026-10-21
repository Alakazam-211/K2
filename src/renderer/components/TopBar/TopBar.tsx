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
import TopBarTitle, { type TopBarRemoteServer } from './TopBarTitle'
import { RunIcon } from '@/components/icons/K2Icons'
import { Surface } from '@/components/ui'
import {
  topBarLeftClusterMinWidth,
  TRAFFIC_LIGHT_CLUSTER_GAP_PX,
} from '@/lib/desktop-chrome'
import { primaryScope } from '@/kessel/server-scope'

interface TopBarProps {
  projectName?: string
  projectPath?: string
  workspaceName?: string
  /** Home: the other server the shown agent is on (centre breadcrumb). */
  remoteServer?: TopBarRemoteServer
  onRunCommand?: (command: string) => void
}

export default function TopBar({
  projectName,
  projectPath,
  workspaceName,
  remoteServer,
  onRunCommand
}: TopBarProps): React.JSX.Element {
  const [hasRun, setHasRun] = useState(false)

  useEffect(() => {
    if (!projectPath) {
      setHasRun(false)
      return
    }

    let cancelled = false
    daemonCliGet<{ hasRunCommand: boolean }>(primaryScope(), 'project-config/has-run-command', { project: projectPath })
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
      const result = await daemonCliGet<{ command: string }>(primaryScope(), 'project-config/run-command', { project: projectPath })
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
        <ServerSwitcher followRoom />
        {/* §6.0 — Home | Agents | Projects | Tickets | Wiki | ⚙ (cog last, 0.46.3). */}
        <PageTabs />
      </div>

      {/* Center: [other server /] agent / branch */}
      <TopBarTitle agent={projectName} branch={workspaceName} remoteServer={remoteServer} />

      {/* Right: run button + utilities + window controls. The drawer
          toggles live in the room and drawer tab bars
          (`Layout/DrawerTabBarToggle.tsx`), not here. */}
      <DesktopChromeRight>
        {/* 0.43.2 Z14/Z36: only this bar (Agents and Home) follows a
            focused remote Home room. */}
        <TopBarUtilities
          followRoom
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
                <RunIcon />
              </button>
            ) : null
          }
        />
      </DesktopChromeRight>
    </Surface>
  )
}

// FeedbackTopBarButton (v0.40.26) was absorbed into the §6.0 page
// switcher — see PageTabs.tsx (the Feedback tab keeps its waiting-count
// badge and the event-wiring effect verbatim).
