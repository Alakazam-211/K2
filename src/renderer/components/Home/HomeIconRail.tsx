// Home on the Agents shell — the collapsed rail (48px). At the top, « (open
// the nav) then … (the Groups dialog); at the bottom, the one search (the
// command palette) then + (Add to Home), as the nav's footer; between them
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
import { useHomeRowAvatar, useHomeRowColor } from '@/lib/home-avatars'
import { ProjectIcon } from '@/components/Sidebar/IconRail'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { DoubleChevron } from '@/components/Sidebar/SidebarCollapseButton'
import { NavFooterPlus } from '@/components/Sidebar/NavFooterButton'
import { EllipsisIcon, SearchIcon } from 'lucide-react'
import { K2_ICON_PX, K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { NAV_PALETTE_LABEL } from './HomeNavFooter'
import { AddToHomeHost } from './AddToHomeModal'
import { useHomeAddPickerStore, useRowStatus } from './home-room'
import { SetupChecklistRailButton } from '@/components/SetupChecklist/SetupChecklist'
import { OTHER_SERVER_AVATAR_COLOR, homeRowContextMenu, openOtherHomeRow } from './HomeSidebar'
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
        if (canOpen) openOtherHomeRow(row, status.kind)
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

/** A group's rail icons, pins first under a short rule. An empty group
 *  shows only a + (Add to Home), its title saying the group is empty;
 *  `preview` (the group a swipe heads to) draws it, not live. */
function RailRows({ home, preview = false }: { home: Home; preview?: boolean }): React.JSX.Element {
  const rows = displayRows(home)
  const pinCount = rows.filter(isPinned).length
  if (rows.length === 0) {
    return (
      <div className="flex w-full flex-col items-center gap-0.5">
        <button
          type="button"
          data-k2-focus=""
          data-home-add-open="empty"
          className={RAIL_BUTTON_CLASS}
          title={`${home.name} is empty: add agents`}
          aria-label={`${home.name} is empty. Add to Home`}
          aria-haspopup="dialog"
          aria-hidden={preview ? true : undefined}
          tabIndex={preview ? -1 : undefined}
          onClick={preview ? undefined : (e) => useHomeAddPickerStore.getState().setOpen(true, e.currentTarget)}
        >
          <NavFooterPlus />
        </button>
      </div>
    )
  }
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
      <RailRows home={home} preview />
    </div>
  )
}

/** A rail button (the mock's rail control): 32 × 32, no fill at rest and
 *  muted ink; the row hover and secondary ink on hover; round in Glass. */
export const RAIL_BUTTON_CLASS =
  'no-drag flex h-8 w-8 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-row-hover)] hover:text-[var(--color-text-secondary)] [[data-style=glass]_&]:rounded-[var(--radius-selector)]'

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
      {/* Top: « (open the nav), then … (the Groups dialog) beneath it. */}
      <div className="flex flex-shrink-0 flex-col items-center gap-0.5 mb-1" data-home-rail-top="">
        <button
          type="button"
          data-k2-focus=""
          data-home-rail-open=""
          className={RAIL_BUTTON_CLASS}
          onClick={() => useSidebarStore.getState().expand()}
          title="Open workspaces sidebar"
          aria-label="Open workspaces sidebar"
        >
          <DoubleChevron direction="right" className="h-4 w-4" />
        </button>
        <button
          type="button"
          data-k2-focus=""
          data-group-edit-open=""
          className={RAIL_BUTTON_CLASS}
          onClick={() => groups.editCurrent()}
          title={`Edit groups (${groups.current.name})`}
          aria-label={`Edit groups (${groups.current.name})`}
          aria-haspopup="dialog"
        >
          <EllipsisIcon size={K2_ICON_PX} strokeWidth={K2_ICON_STROKE} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true" focusable="false" />
        </button>
      </div>

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
        {/* Bottom: the one search (the command palette), then + (Add to Home). */}
        <div className="flex flex-col items-center gap-0.5 mt-1" data-home-rail-bottom="">
          <button
            type="button"
            data-k2-focus=""
            data-home-rail-search=""
            className={RAIL_BUTTON_CLASS}
            onClick={() => useCommandPaletteStore.getState().toggle()}
            title={NAV_PALETTE_LABEL}
            aria-label={NAV_PALETTE_LABEL}
            aria-haspopup="dialog"
            aria-keyshortcuts="Meta+K"
          >
            <SearchIcon size={K2_ICON_PX} strokeWidth={K2_ICON_STROKE} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true" focusable="false" />
          </button>
          <button
            type="button"
            data-k2-focus=""
            data-home-add-open="rail"
            className={RAIL_BUTTON_CLASS}
            onClick={(e) => useHomeAddPickerStore.getState().setOpen(true, e.currentTarget)}
            title="Add to Home"
            aria-label="Add to Home"
            aria-haspopup="dialog"
          >
            <NavFooterPlus />
          </button>
        </div>
      </div>
      <AddToHomeHost />
    </div>
  )
}
