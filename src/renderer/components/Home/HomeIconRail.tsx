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

import { memo, useDeferredValue } from 'react'
import { useHomesStore, selectedHome, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useSidebarStore } from '@/stores/sidebar'
import { useCommandPaletteStore } from '@/stores/command-palette'
import { isWebClient } from '@/lib/is-web'
import { activeHomeHostKey, findWorkspaceForRow, parseHomeAddress } from '@/lib/home-address'
import { useHomeRowAvatar, useHomeRowColor } from '@/lib/home-avatars'
import { ProjectIcon, RAIL_TOP_ROW_CLASS } from '@/components/Sidebar/IconRail'
import ProjectAvatar, { cachedProjectIcon } from '@/components/Sidebar/ProjectAvatar'
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
import { useHomeRowOnScreen } from './home-row-on-screen'
import { openHomeRow } from '@/lib/home-open'

const RAIL_WIDTH = 48

/** A row that isn't a workspace on the connected server: another server's
 *  agent, a person or channel, a Grok Bot. Lit like `ProjectIcon` while it
 *  is the one on screen (its room or chat). */
function OtherRowIcon({ home, row, isActive }: { home: Home; row: HomeRow; isActive: boolean }): React.JSX.Element {
  const { status, place, onConnected } = useRowStatus(row)
  // As the nav's row: "No access" rows never open a room (MS83).
  const canOpen = !(isWebClient() && !onConnected) && status.kind !== 'not-found' && status.kind !== 'no-access'
  const title = [row.label, place, status.label].filter(Boolean).join(' • ')
  const avatarUrl = useHomeRowAvatar(row.address)
  const color = useHomeRowColor(row.address) ?? OTHER_SERVER_AVATAR_COLOR
  return (
    <button
      className={`no-drag relative flex items-center justify-center w-8 h-8 flex-shrink-0 transition-colors ${
        isActive
          ? 'bg-white/[0.12] text-[var(--color-text-primary)] icon-rail-active'
          : 'text-[var(--color-text-muted)] hover:bg-white/[0.06] hover:text-[var(--color-text-secondary)]'
      }${status.kind === 'offline' || status.kind === 'not-found' ? ' opacity-60' : ''}`}
      style={isActive ? ({ '--accent-color': color } as React.CSSProperties) : undefined}
      onClick={() => {
        if (canOpen) openOtherHomeRow(row, status.kind)
      }}
      onContextMenu={(e) => void homeRowContextMenu(e, home, row)}
      title={title}
      aria-current={isActive ? 'true' : undefined}
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
  const parsed = parseHomeAddress(row.address)
  const ws =
    parsed && parsed.host === connectedKey && connectionStatus === 'connected'
      ? findWorkspaceForRow(projects, row)
      : null
  // The nav's answer, not just the active workspace: with another server's
  // room on screen, that room's icon is the lit one (home-row-on-screen).
  const isActive = useHomeRowOnScreen(row, ws)
  if (ws) {
    return (
      <ProjectIcon
        project={ws}
        isActive={isActive}
        // As ⌘N and the nav's row: give the main area back from a room or
        // chat, then select the workspace.
        onClick={() => void openHomeRow(row)}
        onContextMenu={(e) => void homeRowContextMenu(e, home, row)}
        homeRailRow={row.address}
      />
    )
  }
  return <OtherRowIcon home={home} row={row} isActive={isActive} />
}

/** The short rule under a group's pinned icons. */
function RailPinRule(): React.JSX.Element {
  return <span aria-hidden="true" data-home-rail-pin-rule="" className="mx-auto my-[3px] h-px w-5 flex-shrink-0 bg-[var(--color-divider)]" />
}

/** A group's rail icons, pins first under a short rule. An empty group
 *  shows only a + (Add to Home), its title saying the group is empty;
 *  `preview` (a neighbouring group beside the rail) draws it, not live. */
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
        pinCount > 0 && i === pinCount - 1 && i < rows.length - 1 ? <RailPinRule key="pin-rule" /> : null,
      ])}
    </div>
  )
}

/** One icon of a neighbouring group, cheap: the live icon's 32px box and
 *  avatar, no status, presence or effects. */
function RailRowShell({ row, project }: { row: HomeRow; project: { name: string; path: string; color: string; iconUrl?: string | null } | null }): React.JSX.Element {
  const avatarUrl = useHomeRowAvatar(row.address)
  const rowColor = useHomeRowColor(row.address)
  const color = project?.color ?? rowColor ?? OTHER_SERVER_AVATAR_COLOR
  return (
    <span className="relative flex h-8 w-8 flex-shrink-0 items-center justify-center" data-home-rail-row-shell={row.address}>
      <ProjectAvatar
        projectPath={project?.path ?? `home:${row.address}`}
        projectName={project?.name ?? row.label}
        projectColor={color}
        iconUrl={project ? (project.iconUrl ?? cachedProjectIcon(project.path)?.dataUrl ?? null) : avatarUrl}
        size={20}
        fetchIcon={false}
      />
    </span>
  )
}

/** A neighbouring group beside the rail's icons, drawn ahead of a swipe:
 *  cheap icons (`RailRowShell`), pins first under the rule. */
const RailRowShells = memo(function RailRowShells({ home }: { home: Home }): React.JSX.Element {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const connected = useConnectHostStore((s) => s.connectionStatus === 'connected')
  const projects = useProjectsStore((s) => s.projects)
  const rows = displayRows(home)
  const pinCount = rows.filter(isPinned).length
  return (
    <div className="flex-1 flex flex-col items-center gap-0.5 overflow-hidden w-full">
      {rows.length === 0 ? (
        <RailRows home={home} preview />
      ) : (
        <div className="flex w-full flex-col items-center gap-0.5">
          {rows.map((row, i) => {
            const onHere = connected && parseHomeAddress(row.address)?.host === connectedKey
            const ws = onHere ? findWorkspaceForRow(projects, row) : null
            return [
              <RailRowShell key={row.address} row={row} project={ws} />,
              pinCount > 0 && i === pinCount - 1 && i < rows.length - 1 ? <RailPinRule key="pin-rule" /> : null,
            ]
          })}
        </div>
      )}
    </div>
  )
})

/** A rail button (the mock's rail control): 32 × 32, no fill at rest and
 *  muted ink; the row hover and secondary ink on hover; round in Glass. */
export const RAIL_BUTTON_CLASS =
  'no-drag flex h-8 w-8 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-row-hover)] hover:text-[var(--color-text-secondary)] [[data-style=glass]_&]:rounded-[var(--radius-selector)]'

export default function HomeIconRail(): React.JSX.Element {
  // As the nav: the icons draw a switched-to group in the background.
  const home = useDeferredValue(useHomesStore(selectedHome))
  const groups = useHomeGroups()
  return (
    <div
      data-material="tile"
      className="flex flex-col items-center h-full bg-[var(--color-bg-surface)] border-r border-[var(--color-border)] pb-2 flex-shrink-0"
      style={{ width: RAIL_WIDTH }}
      data-home-rail=""
      data-group-swipe-zone=""
    >
      {/* Top: » (open the nav) in the tab strip's top row, as the open
          nav's « is; … (the Groups dialog) under the row's line. */}
      <div className="flex w-full flex-shrink-0 flex-col items-center mb-1" data-home-rail-top="">
        <div className={RAIL_TOP_ROW_CLASS} data-rail-top-row="">
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
        </div>
        <button
          type="button"
          data-k2-focus=""
          data-group-edit-open=""
          className={`${RAIL_BUTTON_CLASS} mt-2`}
          onClick={() => groups.editCurrent()}
          title={`Edit groups (${groups.current.name})`}
          aria-label={`Edit groups (${groups.current.name})`}
          aria-haspopup="dialog"
        >
          <EllipsisIcon size={K2_ICON_PX} strokeWidth={K2_ICON_STROKE} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true" focusable="false" />
        </button>
      </div>

      <div className="relative flex min-h-0 w-full flex-1 flex-col overflow-hidden" data-home-rail-stage="" data-group-swipe-stage="">
        <GroupSwipeStrip variant="rail" groupId={home.id} renderNeighbour={(g) => <RailRowShells home={g} />}>
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
