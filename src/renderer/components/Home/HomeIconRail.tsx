// Home on the Agents shell — the collapsed rail. Same chrome as the Agents
// `IconRail` (48px, command palette, + then the collapse button), with
// the Home roster as icons. A connected-server row is the Agents rail icon
// itself (`ProjectIcon`); a row on another server is the same 32px button
// with its letter avatar.

import { useHomesStore, selectedHome, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useSidebarStore } from '@/stores/sidebar'
import { useCommandPaletteStore } from '@/stores/command-palette'
import { isWebClient } from '@/lib/is-web'
import { activeHomeHostKey, findWorkspaceForRow, parseHomeAddress } from '@/lib/home-address'
import { openHomeRow } from '@/lib/home-open'
import { useHomeRowAvatar } from '@/lib/home-avatars'
import { ProjectIcon } from '@/components/Sidebar/IconRail'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { SidebarCollapseButton } from '@/components/Sidebar/SidebarCollapseButton'
import { useHomeAddPickerStore, useRowStatus } from './home-room'
import { OTHER_SERVER_AVATAR_COLOR, homeRowContextMenu } from './HomeSidebar'

const RAIL_WIDTH = 48

function OtherRowIcon({ home, row }: { home: Home; row: HomeRow }): React.JSX.Element {
  const { status, place, onConnected } = useRowStatus(row)
  const canOpen = !(isWebClient() && !onConnected) && status.kind !== 'not-found'
  const title = [row.label, place, status.label].filter(Boolean).join(' • ')
  const avatarUrl = useHomeRowAvatar(row.address)
  return (
    <button
      className={`no-drag relative flex items-center justify-center w-8 h-8 flex-shrink-0 transition-colors text-[var(--color-text-muted)] hover:bg-white/[0.06] hover:text-[var(--color-text-secondary)]${
        status.kind === 'offline' || status.kind === 'not-found' ? ' opacity-60' : ''
      }`}
      onClick={() => {
        if (canOpen) openHomeRow(row)
      }}
      onContextMenu={(e) => void homeRowContextMenu(e, home, row)}
      title={title}
    >
      <ProjectAvatar
        projectPath={`home:${row.address}`}
        projectName={row.label}
        projectColor={OTHER_SERVER_AVATAR_COLOR}
        iconUrl={avatarUrl}
        size={20}
        fetchIcon={false}
      />
    </button>
  )
}

function RailRow({ home, row }: { home: Home; row: HomeRow }): React.JSX.Element {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const parsed = parseHomeAddress(row.address)
  const ws =
    parsed && parsed.host === connectedKey && connectionStatus === 'connected'
      ? findWorkspaceForRow(projects, row)
      : null
  if (ws) {
    return (
      <ProjectIcon
        project={ws}
        isActive={ws.id === activeProjectId}
        onClick={() => useProjectsStore.getState().setActiveProject(ws.id)}
        onContextMenu={(e) => void homeRowContextMenu(e, home, row)}
      />
    )
  }
  return <OtherRowIcon home={home} row={row} />
}

export default function HomeIconRail(): React.JSX.Element {
  const home = useHomesStore(selectedHome)
  return (
    <div
      className="flex flex-col items-center h-full bg-[var(--color-bg-surface)] border-r border-[var(--color-border)] py-2 flex-shrink-0"
      style={{ width: RAIL_WIDTH }}
      data-home-rail=""
    >
      <button
        className="no-drag flex items-center justify-center w-8 h-8 flex-shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.06] transition-colors mb-1"
        onClick={() => useCommandPaletteStore.getState().toggle()}
        title="Search Workspaces (⌘K)"
      >
        <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
          <circle cx="11" cy="11" r="8" />
          <line x1="21" y1="21" x2="16.65" y2="16.65" />
        </svg>
      </button>

      <div className="flex-1 flex flex-col items-center gap-0.5 overflow-y-auto overflow-x-hidden w-full">
        {home.rows.map((row) => (
          <RailRow key={row.address} home={home} row={row} />
        ))}
      </div>

      <div className="flex flex-col items-center mt-1 flex-shrink-0">
        <button
          className="no-drag flex items-center justify-center w-8 h-8 flex-shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.06] transition-colors"
          onClick={() => {
            useSidebarStore.getState().expand()
            useHomeAddPickerStore.getState().setOpen(true)
          }}
          title="Add Agent"
        >
          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
          </svg>
        </button>
        <SidebarCollapseButton />
      </div>
    </div>
  )
}
