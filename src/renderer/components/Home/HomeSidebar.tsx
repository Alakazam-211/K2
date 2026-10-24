// Home on the Agents shell — the sidebar (prd-home-v1 H2/H3/H5/H6).
//
// Same chrome as the Agents `Sidebar`: resize handle, the header control
// row (here the Home picker in the focus-group dropdown's spot, plus the
// rows' "⌘ 1-9" hint; ⌘K stays on the keyboard, the menus and the rails),
// the row list, and the bottom bar with ONE
// button in the Add Workspace spot — Add Agent — beside the collapse
// button. Add Agent opens one picker with This server and From a server.
//
// Rows:
//   - A row on the connected server paints as the Agents row itself
//     (`SingleProjectItem`: avatar, presence, tags, working glyph,
//     worktrees). Clicking it selects that workspace; the room opens in
//     the main area and the page stays Home.
//   - A row on another server paints through the same row shell
//     (`AgentRowButton`) with a small machine chip and its live / offline /
//     sign-in state on the second line, plus that server's presence.
//     With "Open other servers' agents in Home" on (the default on macOS
//     and Linux from 0.43.2), clicking it opens that server's room in the
//     main area without switching (usable from K2 0.43.0, view only from
//     the floor up to that). Off, or below the floor, it switches this
//     window's server (H15) and lands on Home.

import { useCallback, useEffect, useMemo, useRef } from 'react'
import { useHomesStore, selectedHome, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { usePageViewStore } from '@/stores/page-view'
import { isWebClient } from '@/lib/is-web'
import { showContextMenu } from '@/lib/context-menu'
import type { ContextMenuItemDef } from '@/stores/context-menu'
import { activeHomeHostKey, findWorkspaceForRow, parseHomeAddress } from '@/lib/home-address'
import { switchWindowToRow } from '@/lib/home-switch'
import { homeRoomVerdict, type HomeRoomVerdict } from '@/lib/home-room-floor'
import { hostPool } from '@/lib/host-pool-instance'
import { useStore } from 'zustand'
import type { RowStatus } from '@/lib/home-status'
import { remoteAgentStatus, remoteRowBadges, rowActionWord, rowReach } from '@/lib/home-agent-status'
import { TicketCountBadge, UnreadBadge } from '@/components/common/UnreadBadge'
import { MachineChip } from './MachineChip'
import { AgentStatusCluster } from '@/components/AgentStatus/AgentStatusCluster'
import { openHomeRow } from '@/lib/home-open'
import { useHomeRowAvatar } from '@/lib/home-avatars'
import { useRemoteRoomsPreview } from '@/lib/remote-rooms-preview'
import { homeRooms, useShownHomeRoom } from '@/stores/home-rooms'
import { AgentRowButton, ShortcutRangeHint, SingleProjectItem } from '@/components/Sidebar/Sidebar'
import { HOME_ROW_SHORTCUT_HINT } from '@/lib/home-shortcuts'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import ResizeHandle from '@/components/Sidebar/ResizeHandle'
import { SidebarCollapseButton } from '@/components/Sidebar/SidebarCollapseButton'
import { PresenceAvatarCluster } from '@/components/Presence/PresenceWorkspaceAvatars'
import { RowDropLine, dropTargetIndex, useRowDragReorder } from '@/components/Sidebar/row-drag-reorder'
import HomePicker from './HomePicker'
import { AddAgentPicker } from './HomeAddPanels'
import { useConnectedRowNote, useHomeAddPickerStore, useRoomRowFacts, useRowStatus } from './home-room'
import { GrokBotHomeRow } from '@/components/GrokBots/GrokBotRows'
import { isGrokBotAddress } from '@/lib/grokbots'

/** Avatar color for an agent on another server (its color lives there). */
export const OTHER_SERVER_AVATAR_COLOR = 'var(--color-text-muted)'

export { MachineChip } from './MachineChip'

/** The one word a remote row keeps: an action (Sign in, No access, Not
 *  found), just before the cluster (Q2: glyphs everywhere else). */
function RowActionWord({ status }: { status: RowStatus }): React.JSX.Element | null {
  const word = rowActionWord(status)
  if (!word) return null
  const action = status.kind === 'sign-in'
  return (
    <span
      className={`inline-flex h-3.5 flex-shrink-0 items-center whitespace-nowrap text-[10px] leading-none ${
        action
          ? 'text-[var(--color-text-primary)] underline underline-offset-2'
          : 'text-[var(--color-text-muted)]'
      }`}
      data-row-action={status.kind}
    >
      {word}
    </span>
  )
}

// Avatar color only. There is no viewer role; an older server's `viewer`
// row paints with the member color.
function asRole(role: string): 'owner' | 'admin' | 'member' {
  return role === 'owner' || role === 'admin' ? role : 'member'
}

/** "Same server as …" (MS81): one daemon saved at two addresses stays two
 *  rows; this says so. */
function SameServerNote({ note }: { note: string }): React.JSX.Element {
  return (
    <span
      className="truncate text-[9px] leading-none text-[var(--color-text-muted)]"
      title={`${note}: both addresses answer with the same daemon`}
      data-same-server=""
    >
      {note.toLowerCase()}
    </span>
  )
}

/** Right-click menu on a Home row (the Agents rows' menu idiom). */
export async function homeRowContextMenu(e: React.MouseEvent, home: Home, row: HomeRow): Promise<void> {
  e.preventDefault()
  const parsed = parseHomeAddress(row.address)
  const connectedKey = activeHomeHostKey(useConnectHostStore.getState().activeHost)
  const ws =
    parsed && parsed.host === connectedKey ? findWorkspaceForRow(useProjectsStore.getState().projects, row) : null
  const items: ContextMenuItemDef[] = []
  if (ws) {
    items.push({ id: 'home-settings', label: 'Workspace Settings' })
    items.push({ id: 'home-wiki', label: 'View Wiki' })
  } else if (parsed && parsed.host !== connectedKey && !isWebClient()) {
    items.push({ id: 'home-open', label: 'Switch to its server and open' })
  }
  const roomOpen = homeRooms.store.getState().entries[row.address]
  if (roomOpen) items.push({ id: 'home-close-room', label: 'Close room' })
  if (items.length > 0) items.push({ id: 'home-sep', label: '', type: 'separator' })
  items.push({ id: 'home-remove', label: `Remove from ${home.name}` })

  const clicked = await showContextMenu(items)
  if (clicked === 'home-settings' && ws) {
    useSettingsStore.getState().openSettings('projects', ws.id)
  } else if (clicked === 'home-wiki' && ws) {
    usePageViewStore.getState().openWiki(ws.path)
  } else if (clicked === 'home-open') {
    switchToRowServer(row)
  } else if (clicked === 'home-close-room') {
    void homeRooms.close(row.address)
  } else if (clicked === 'home-remove') {
    useHomesStore.getState().removeRow(home.id, row.address)
  }
}

/** "Switch to its server and open": the window switch, even with rooms on
 *  (the explicit gesture). */
function switchToRowServer(row: HomeRow): void {
  homeRooms.showPrimary()
  switchWindowToRow(row)
}

/** What clicking a row on another server does, for its tooltip. */
export function otherRowOpenTitle(
  rooms: boolean,
  verdict: HomeRoomVerdict,
  label: string,
  place: string | null,
): string {
  const where = place ?? 'its server'
  if (!rooms || verdict === 'switch') return `Switch this window to ${where} and open ${label}`
  if (verdict === 'view-only') return `Open ${label} from ${where} here, view only`
  return `Open ${label} from ${where} here`
}

/** A row that does not paint as a live Agents row. */
function OtherHomeRow({ home, row, index }: { home: Home; row: HomeRow; index: number }): React.JSX.Element {
  const { status, place, onConnected } = useRowStatus(row)
  const preview = useRemoteRoomsPreview()
  const shownRoom = useShownHomeRoom()
  const rowHost = parseHomeAddress(row.address)?.host ?? ''
  const boot = useStore(hostPool.store, (s) => s.entries[rowHost]?.boot ?? null)
  const verdict = homeRoomVerdict(boot)
  const summary = useStore(hostPool.store, (s) => s.entries[rowHost]?.activity ?? null)
  const room = useStore(homeRooms.store, (s) => s.entries[row.address]?.room ?? null)
  const roomFacts = useRoomRowFacts(room)
  // plan-agent-status-unify-v1 S4: the same cluster as a connected row.
  const view = useMemo(
    () =>
      remoteAgentStatus({
        status,
        row,
        activity: summary,
        room: roomFacts,
        serverLiveBusy: boot?.features.includes('activity-live-busy') ?? false,
      }),
    [status, row, summary, roomFacts, boot],
  )
  // agent-unread-tickets-v1: your unread / waiting on this agent's Threads
  // and its waiting tickets, from that server's summary (read with your
  // login there). An older server reports neither: no badges.
  const badges = useMemo(
    () =>
      remoteRowBadges({
        status,
        row,
        activity: summary,
        serverReports: boot?.features.includes('agent-unread-tickets-v1') ?? false,
      }),
    [status, row, summary, boot],
  )
  const reach = rowReach(status.kind)
  const remoteOnWeb = isWebClient() && !onConnected
  // MS83: "No access" rows never open a room.
  const canOpen = !remoteOnWeb && status.kind !== 'not-found' && status.kind !== 'no-access'
  const base = remoteOnWeb
    ? 'Open this agent from the desktop app — the web page has no server switcher'
    : status.kind === 'no-access'
      ? (status.detail ?? `Your login on ${place} cannot open agents there`)
      : status.kind === 'sign-in'
        ? `${status.detail ? `${status.detail} ` : ''}Sign in to ${place} and open ${row.label}`
        : !onConnected
          ? otherRowOpenTitle(preview, verdict, row.label, place)
          : status.kind === 'not-found'
            ? `${row.label} is not on this server any more`
            : `Open ${row.label}`
  const title = status.note ? `${base} (${status.note})` : base
  // The agent's picture from this computer's cache (data URL), or null.
  const avatarUrl = useHomeRowAvatar(row.address)

  return (
    <AgentRowButton
      isActive={preview && shownRoom === row.address}
      color={OTHER_SERVER_AVATAR_COLOR}
      dimmed={status.kind === 'offline' || status.kind === 'not-found'}
      onClick={() => {
        if (canOpen) openHomeRow(row)
      }}
      onContextMenu={(e) => void homeRowContextMenu(e, home, row)}
      title={title}
      avatar={
        <ProjectAvatar
          projectPath={`home:${row.address}`}
          projectName={row.label}
          projectColor={OTHER_SERVER_AVATAR_COLOR}
          iconUrl={avatarUrl}
          size={32}
          fetchIcon={false}
        />
      }
      name={row.label}
      nameAside={
        <>
          {badges && (
            <span className="mr-1 inline-flex items-center gap-0.5" data-remote-row-badges="">
              <TicketCountBadge count={badges.tickets} mine={badges.ticketsMine} />
              <UnreadBadge
                unread={badges.unread}
                waiting={badges.waiting}
                muted={badges.muted}
                label="Thread messages"
              />
            </span>
          )}
          <PresenceAvatarCluster
            users={status.people.map((p) => ({
              user: p.user,
              role: asRole(p.role),
              color: p.color,
              firstName: p.firstName ?? null,
              lastName: p.lastName ?? null,
            }))}
          />
        </>
      }
      subline={
        <>
          <div className="flex min-w-0 flex-1 items-center gap-0.5 overflow-hidden">
            {place !== null && <MachineChip place={place} reach={reach} />}
            {status.note && <SameServerNote note={status.note} />}
          </div>
          <RowActionWord status={status} />
          <span className="inline-flex" data-row-status={status.kind}>
            <AgentStatusCluster view={view} index={index} />
          </span>
        </>
      }
    />
  )
}

/** `index` is the row's position: its Cmd+N badge (useWorkspaceIndexShortcuts
 *  selects `rows[N - 1]` on Home), shown for the first nine rows like the
 *  Agents pinned area. */
function HomeRowView({ home, row, index }: { home: Home; row: HomeRow; index: number }): React.JSX.Element {
  // Local Grok Bots (0.45.7): not a workspace, its own row.
  if (isGrokBotAddress(row.address)) return <GrokBotHomeRow home={home} row={row} index={index} />
  return <WorkspaceHomeRowView home={home} row={row} index={index} />
}

function WorkspaceHomeRowView({ home, row, index }: { home: Home; row: HomeRow; index: number }): React.JSX.Element {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const parsed = parseHomeAddress(row.address)
  const ws =
    parsed && parsed.host === connectedKey && connectionStatus === 'connected'
      ? findWorkspaceForRow(projects, row)
      : null
  const note = useConnectedRowNote(row)
  // Home M4: while a remote room is on screen, no connected row is active;
  // clicking one gives the main area back to the window's own room.
  const remoteShown = useShownHomeRoom() !== null
  const isActive = !!ws && ws.id === activeProjectId && !remoteShown
  if (ws && !note) {
    return (
      <div onClickCapture={() => homeRooms.showPrimary()}>
        <SingleProjectItem
          project={ws}
          isActive={isActive}
          onContextMenu={(e) => void homeRowContextMenu(e, home, row)}
          shortcutIndex={index}
        />
      </div>
    )
  }
  if (ws && note) {
    return (
      <div className="relative" title={note} onClickCapture={() => homeRooms.showPrimary()}>
        <SingleProjectItem
          project={ws}
          isActive={isActive}
          onContextMenu={(e) => void homeRowContextMenu(e, home, row)}
          shortcutIndex={index}
        />
        <div className="pointer-events-none absolute right-2 top-1">
          <SameServerNote note={note} />
        </div>
      </div>
    )
  }
  return (
    <div className="no-drag">
      <OtherHomeRow home={home} row={row} index={index} />
    </div>
  )
}

export default function HomeSidebar(): React.JSX.Element {
  const home = useHomesStore(selectedHome)
  const pickerOpen = useHomeAddPickerStore((s) => s.open)
  const setPickerOpen = useHomeAddPickerStore((s) => s.setOpen)
  const barRef = useRef<HTMLDivElement | null>(null)
  const listRef = useRef<HTMLDivElement | null>(null)

  // ── Drag-to-reorder (shared with the Agents sidebar: same ghost, line, cursor) ──
  const rowDrag = useRowDragReorder<'home'>()
  const dropHomeRow = useCallback((from: number, gap: number) => {
    const to = dropTargetIndex(from, gap)
    if (to === null) return
    const current = selectedHome(useHomesStore.getState())
    useHomesStore.getState().moveRow(current.id, from, to)
  }, [])

  // Close the picker on outside click / Esc.
  useEffect(() => {
    if (!pickerOpen) return
    const onDown = (e: MouseEvent): void => {
      if (barRef.current && e.target instanceof Node && !barRef.current.contains(e.target)) setPickerOpen(false)
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') setPickerOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [pickerOpen, setPickerOpen])

  const rows = home.rows

  return (
    <div className="relative flex flex-col h-full" data-home-sidebar="">
      <ResizeHandle />

      {/* Header controls — the focus-group row's spot and look. */}
      <div className="px-3 pt-3 pb-2 no-drag flex items-center gap-1.5">
        <div className="flex-1 min-w-0">
          <HomePicker />
        </div>
        {/* The rows' Cmd+1–9 hint, where the palette button was (P1/P2).
            The per-Home ⌥⌘N badges stay inside the open picker. */}
        {rows.length > 0 && (
          <span
            className="flex-shrink-0 px-1.5"
            title="⌘1–⌘9 open this Home's agents 1–9"
            data-home-row-hint=""
          >
            <ShortcutRangeHint combo={HOME_ROW_SHORTCUT_HINT} />
          </span>
        )}
      </div>

      {/* Home roster */}
      <div className="flex-1 overflow-y-auto overflow-x-hidden py-1">
        <div ref={listRef}>
          {rows.map((row, idx) => (
            <div
              key={row.address}
              data-home-row={row.address}
              style={{ opacity: rowDrag.isDragging('home', idx) ? 0.4 : 1 }}
              onMouseDown={(e) =>
                rowDrag.onRowMouseDown(e, {
                  zone: 'home',
                  index: idx,
                  container: () => listRef.current,
                  rowSelector: '[data-home-row]',
                  onDrop: dropHomeRow,
                })
              }
              onClickCapture={rowDrag.onRowClickCapture}
            >
              {rowDrag.showsDropLineAt('home', idx) && <RowDropLine />}
              <HomeRowView home={home} row={row} index={idx} />
              {idx < rows.length - 1 && !rowDrag.showsDropLineAt('home', idx + 1) && (
                <div className="border-b border-[var(--color-border)]" />
              )}
            </div>
          ))}
          {rowDrag.showsDropLineAt('home', rows.length) && <RowDropLine />}
        </div>

        {rows.length === 0 && (
          <div className="px-3 py-6 text-center">
            <p className="text-xs text-[var(--color-text-muted)]">No agents on this Home yet</p>
            <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-60">Add an agent to get started</p>
          </div>
        )}
      </div>

      {/* Add Agent + collapse the nav — the Add Workspace bar. (Zen Mode is
          the window's top-bar toggle, prd-zen-gardens-v1 G5/G6.) */}
      <div ref={barRef} className="relative p-3 border-t border-[var(--color-border)] flex flex-col gap-2">
        {pickerOpen && <AddAgentPicker home={home} />}
        <div className="flex gap-2">
          <button
            className="no-drag flex-1 flex items-center justify-center gap-2 px-3 py-2 text-xs bg-white/[0.04] transition-colors text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.08]"
            onClick={() => setPickerOpen(!pickerOpen)}
            aria-expanded={pickerOpen}
            aria-haspopup="dialog"
          >
            <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
            </svg>
            Add Agent
          </button>
          <SidebarCollapseButton />
        </div>
      </div>
    </div>
  )
}
