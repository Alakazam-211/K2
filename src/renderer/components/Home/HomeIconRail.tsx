// Home on the Agents shell — the collapsed rail. Same chrome as the Agents
// `IconRail` (48px, command palette, + then the collapse button), with
// the Home roster as icons. A connected-server row is the Agents rail icon
// itself (`ProjectIcon`); a row on another server is the same 32px button
// with its letter avatar. Above the bottom buttons, the group bar stands
// upright (one icon per group, the current one in the accent colour; five
// show, then it scrolls). Pinned agents sit first, under a short rule. The
// sideways swipe moves the icons as one strip with the next group's,
// its glyph at their leading edge.

import { useHomesStore, selectedHome, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useSidebarStore } from '@/stores/sidebar'
import { useCommandPaletteStore } from '@/stores/command-palette'
import { isWebClient } from '@/lib/is-web'
import { activeHomeHostKey, findWorkspaceForRow, parseHomeAddress } from '@/lib/home-address'
import { openHomeRow } from '@/lib/home-open'
import { useHomeRowAvatar, useHomeRowColor } from '@/lib/home-avatars'
import { ProjectIcon } from '@/components/Sidebar/IconRail'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { SidebarCollapseButton } from '@/components/Sidebar/SidebarCollapseButton'
import { useHomeAddPickerStore, useRowStatus } from './home-room'
import { SetupChecklistRailButton } from '@/components/SetupChecklist/SetupChecklist'
import { OTHER_SERVER_AVATAR_COLOR, homeRowContextMenu } from './HomeSidebar'
import HomeGroupBar from './groups/HomeGroupBar'
import GroupNameFlash from './groups/GroupNameFlash'
import { displayRows, isPinned } from '@/lib/home-row-order'
import { useHomeGroups } from './groups/useHomeGroups'
import { GroupSwipeStrip } from './groups/GroupSwipeSlide'

const RAIL_WIDTH = 48

function OtherRowIcon({ home, row }: { home: Home; row: HomeRow }): React.JSX.Element {
  const { status, place, onConnected } = useRowStatus(row)
  const canOpen = !(isWebClient() && !onConnected) && status.kind !== 'not-found'
  const title = [row.label, place, status.label].filter(Boolean).join(' • ')
  const avatarUrl = useHomeRowAvatar(row.address)
  const color = useHomeRowColor(row.address) ?? OTHER_SERVER_AVATAR_COLOR
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
      data-home-rail-row={row.address}
      data-row-status={status.kind}
    >
      <ProjectAvatar
        projectPath={`home:${row.address}`}
        projectName={row.label}
        projectColor={color}
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

/** A group's rail icons, pins first under a short rule. */
function RailRows({ home }: { home: Home }): React.JSX.Element {
  const rows = displayRows(home)
  const pinCount = rows.filter(isPinned).length
  return (
    <div className="flex w-full flex-col items-center gap-0.5">
      {rows.map((row, i) => [
        <RailRow key={row.address} home={home} row={row} />,
        pinCount > 0 && i === pinCount - 1 && i < rows.length - 1 ? (
          <span key="pin-rule" aria-hidden="true" data-home-rail-pin-rule="" className="mx-auto my-[3px] h-px w-5 flex-shrink-0 bg-[var(--color-divider)]" />
        ) : null,
      ])}
    </div>
  )
}

/** The group a swipe heads to, riding in beside the rail's icons. */
function RailRowsPreview({ home }: { home: Home }): React.JSX.Element {
  return (
    <div className="flex-1 flex flex-col items-center gap-0.5 overflow-hidden w-full">
      <RailRows home={home} />
    </div>
  )
}

export default function HomeIconRail(): React.JSX.Element {
  const home = useHomesStore(selectedHome)
  const groups = useHomeGroups()
  return (
    <div
      data-material="tile"
      className="flex flex-col items-center h-full bg-[var(--color-bg-surface)] border-r border-[var(--color-border)] py-2 flex-shrink-0"
      style={{ width: RAIL_WIDTH }}
      data-home-rail=""
      data-group-swipe-zone=""
    >
      <button
        data-material="frost"
        data-frost-shape="round"
        data-nav-control=""
        className="no-drag flex items-center justify-center w-8 h-8 flex-shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.06] transition-colors mb-1"
        onClick={() => useCommandPaletteStore.getState().toggle()}
        title="Search Workspaces (⌘K)"
      >
        <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
          <circle cx="11" cy="11" r="8" />
          <line x1="21" y1="21" x2="16.65" y2="16.65" />
        </svg>
      </button>

      <div className="relative flex min-h-0 w-full flex-1 flex-col overflow-hidden" data-home-rail-stage="" data-group-swipe-stage="">
        <GroupSwipeStrip variant="rail" renderIncoming={(g) => <RailRowsPreview home={g} />}>
          <div className="flex-1 flex flex-col items-center gap-0.5 overflow-y-auto overflow-x-hidden w-full" data-group-swipe-scroller="">
            <RailRows home={home} />
          </div>
        </GroupSwipeStrip>
      </div>

      <div className="flex flex-col items-center mt-1 flex-shrink-0" data-nav-rail-actions="">
        {/* The group icons, upright (no divider). The name
            flashes beside it after a switch (no room for it here). */}
        <div className="relative mb-1 w-full pt-1 flex justify-center">
          <HomeGroupBar {...groups.bar} orientation="vertical" />
          <GroupNameFlash id={groups.current.id} name={groups.current.name} />
        </div>
        <SetupChecklistRailButton />
        <button
          data-material="frost"
          data-frost-shape="round"
          data-nav-control=""
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
