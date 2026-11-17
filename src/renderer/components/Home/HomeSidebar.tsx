// Home on the Agents shell — the sidebar (prd-home-v1 H2/H3/H5/H6).
//
// Same chrome as the Agents `Sidebar`: resize handle, the top row at the
// tab strip's height (the current group's name and count, "…" for the
// Groups dialog and « to collapse the nav; no "⌘ 1-9" hint; the rows keep
// their 1–9 numbers and ⌘1–9 still opens them; ⌘K stays on the keyboard,
// the menus and the rails), the row list, the group icons right under it
// (one per group, no divider, `groups/HomeGroupBar`), Get started, and
// the bottom bar: the ⌘K field (the command palette's entry) and + (Add to
// Home on the current group, `AddToHomeModal`).
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

import { useCallback, useMemo, useRef } from 'react'
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
import { remoteAgentStatus, remoteRowBadges, rowActionWord, rowReach, summaryHit } from '@/lib/home-agent-status'
import { remoteRoomScope, scopeForHost, primaryScope, type ServerScope } from '@/kessel/server-scope'
import { threadSumForProject, useUnreadStore } from '@/stores/unread'
import { MARK_ALL_READ_MENU_ID, markAllReadMenuItems, runMarkAllRead } from '@/lib/mark-all-read-menu'
import { TicketCountBadge, UnreadBadge } from '@/components/common/UnreadBadge'
import { MachineChip } from './MachineChip'
import { AgentStatusCluster } from '@/components/AgentStatus/AgentStatusCluster'
import { openHomeRow } from '@/lib/home-open'
import { homeRowHoverPrefetch } from '@/lib/home-row-prefetch'
import { useHomeRowAvatar, useHomeRowColor } from '@/lib/home-avatars'
import { useRemoteRoomsPreview } from '@/lib/remote-rooms-preview'
import { homeRooms } from '@/stores/home-rooms'
import { useHomeRowOnScreen } from './home-row-on-screen'
import { AgentRowButton, NavRowDivider, SingleProjectItem } from '@/components/Sidebar/Sidebar'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import ResizeHandle from '@/components/Sidebar/ResizeHandle'
import { ScrollEdgeFrame } from '@/components/ui/ScrollEdgeFade'
import { PresenceAvatarCluster } from '@/components/Presence/PresenceWorkspaceAvatars'
import { NAV_ROW_TEXT_INSET, RowDropLine, dropTargetIndex, useRowDragReorder, type RowDragReorder } from '@/components/Sidebar/row-drag-reorder'
import HomeGroupBar from './groups/HomeGroupBar'
import GroupNameHeader from './groups/GroupNameHeader'
import { useHomeGroups } from './groups/useHomeGroups'
import { GroupSwipeStrip } from './groups/GroupSwipeSlide'
import { useRowFlip } from './groups/useRowFlip'
import { HomeRowCaption, HomeRowPinMark } from './groups/HomeRowCaption'
import { GROUP_EMPTY_TEXT, removeFromGroupLabel } from './groups/group-copy'
import { MAX_PINS, displayRows, isPinned, pinnedRows, planRowDrop, rowDropLanding, shortcutIndexFor } from '@/lib/home-row-order'
import { HomeAddButton, HomeNavFooterRow } from './HomeNavFooter'
import { AddToHomeHost } from './AddToHomeModal'
import { roomFailureActions } from './room/RoomFailure'
import { useConnectedRowNote, useHomeAddPickerStore, useRoomRowFacts, useRowStatus } from './home-room'
import { SetupChecklist } from '@/components/SetupChecklist/SetupChecklist'
import { GrokBotHomeRow } from '@/components/GrokBots/GrokBotRows'
import { isGrokBotAddress } from '@/lib/grokbots'
import { isHomeChatRow } from '@/lib/home-chat/address'
import { HomeChatHomeRow } from '@/components/HomeChat/HomeChatRow'

/** Avatar color for an agent on another server whose own color this
 *  computer doesn't know yet (`useHomeRowColor` is null). A known color
 *  always wins: the machine chip already says which server it is on. */
export const OTHER_SERVER_AVATAR_COLOR = 'var(--color-text-muted)'

export { MachineChip } from './MachineChip'

/** Get started in the Home nav: the closed button drops its own padding
 *  (the icon row's and the bottom bar's 8px are the gaps around it). */
export const HOME_SETUP_SLOT_CLASS = '[&[data-setup-checklist-collapsed]>div]:py-0'

/** What the nav's scroll edges tint with: the nav tile's own fill (Layout's
 *  Surface: `--color-bg-surface`, which Glass's tile reads as `--tile-fill`),
 *  so a band melts into the nav in every Style and palette, never darkens it. */
export const HOME_NAV_EDGE_TINT = 'var(--tile-fill, var(--color-bg-surface))'

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

/** What "Mark all as read" on a Home row acts on (thread-read-all-v1): the
 *  agent's server, its workspace id there, and the unread its badge shows
 *  now. A row on the window's server reads the unread store; a row on
 *  another server reads that server's summary (the pool's) and goes through
 *  a room scope on that server, like the room's own read marks. */
export interface HomeRowReadTarget {
  scope: ServerScope
  projectId: string
  unread: number
  /** The other server's host key; null for the window's own server. */
  remoteHost: string | null
}

export function homeRowReadTarget(row: HomeRow, ws: { id: string } | null): HomeRowReadTarget | null {
  if (ws) {
    const unread = threadSumForProject(useUnreadStore.getState(), ws.id)?.unread ?? 0
    return { scope: primaryScope(), projectId: ws.id, unread, remoteHost: null }
  }
  const host = parseHomeAddress(row.address)?.host
  if (!host) return null
  const hit = summaryHit(hostPool.store.getState().entries[host]?.activity ?? null, row)
  if (!hit) return null
  return { scope: remoteRoomScope(scopeForHost(host)), projectId: hit.workspaceId, unread: hit.unread ?? 0, remoteHost: host }
}

/** Mark all as read for a row: optimistic on another server's summary (the
 *  row's badge drops its unread at once), then that summary is re-read. */
export async function markHomeRowAllRead(target: HomeRowReadTarget, label: string): Promise<void> {
  const host = target.remoteHost
  if (host) {
    hostPool.store.setState((s) => {
      const entry = s.entries[host]
      if (!entry?.activity) return s
      const activity = entry.activity.map((a) => (a.workspaceId === target.projectId ? { ...a, unread: 0 } : a))
      return { entries: { ...s.entries, [host]: { ...entry, activity } } }
    })
  }
  try {
    await runMarkAllRead(target.scope, target.projectId, label)
  } finally {
    // That server's truth either way (a failure brings the badge back).
    if (host) void hostPool.check(host)
  }
}

/** Pin / Unpin in a row's menu. A tenth pin is offered but off, saying why. */
export function pinMenuItem(home: Home, row: HomeRow): ContextMenuItemDef {
  if (isPinned(row)) return { id: 'home-unpin', label: 'Unpin' }
  const full = pinnedRows(home).length >= MAX_PINS
  return full
    ? { id: 'home-pin', label: 'Pin', enabled: false, hint: `Up to ${MAX_PINS} pins. Unpin one first.` }
    : { id: 'home-pin', label: 'Pin', shortcut: `⌘${pinnedRows(home).length + 1}` }
}

/** Right-click menu on a Home row (the Agents rows' menu idiom). */
export async function homeRowContextMenu(e: React.MouseEvent, home: Home, row: HomeRow): Promise<void> {
  e.preventDefault()
  const parsed = parseHomeAddress(row.address)
  const connectedKey = activeHomeHostKey(useConnectHostStore.getState().activeHost)
  const ws =
    parsed && parsed.host === connectedKey ? findWorkspaceForRow(useProjectsStore.getState().projects, row) : null
  const items: ContextMenuItemDef[] = []
  // thread-read-all-v1: while the row's badge has unread.
  const readTarget = homeRowReadTarget(row, ws)
  if (readTarget) items.push(...markAllReadMenuItems(readTarget.scope, readTarget.unread, { separator: false }))
  if (ws) {
    items.push({ id: 'home-settings', label: 'Workspace Settings' })
    items.push({ id: 'home-wiki', label: 'View Wiki' })
  } else if (parsed && parsed.host !== connectedKey && !isWebClient()) {
    items.push({ id: 'home-open', label: 'Switch to its server and open' })
  }
  const roomOpen = homeRooms.store.getState().entries[row.address]
  if (roomOpen) items.push({ id: 'home-close-room', label: 'Close room' })
  if (items.length > 0) items.push({ id: 'home-sep', label: '', type: 'separator' })
  items.push(pinMenuItem(home, row))
  items.push({ id: 'home-remove', label: removeFromGroupLabel(home.name) })

  // On Glass a clear frost: the list shows through, blurred (K2 Design).
  const clicked = await showContextMenu(items, { look: 'clear' })
  if (clicked === 'home-pin') {
    useHomesStore.getState().pinRow(home.id, row.address)
  } else if (clicked === 'home-unpin') {
    useHomesStore.getState().unpinRow(home.id, row.address)
  } else if (clicked === MARK_ALL_READ_MENU_ID && readTarget) {
    void markHomeRowAllRead(readTarget, row.label)
  } else if (clicked === 'home-settings' && ws) {
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

/** A click on a row on another server (the nav row or its rail icon).
 *  A row that says "Sign in" runs the same action as the room's Sign in
 *  button: the non-switching sign-in for THIS row's server. Opening the
 *  room alone only showed its banner, or nothing when that room was
 *  already on screen. A window switch (`pickHost`) signs in on its own. */
export function openOtherHomeRow(row: HomeRow, kind: RowStatus['kind']): void {
  const opened = openHomeRow(row)
  if (kind !== 'sign-in' || opened !== 'room') return
  const host = parseHomeAddress(row.address)?.host
  if (host) roomFailureActions(host).signIn()
}

/** A row that does not paint as a live Agents row. */
function OtherHomeRow({ home, row, index }: { home: Home; row: HomeRow; index: number }): React.JSX.Element {
  const { status, place, onConnected } = useRowStatus(row)
  const preview = useRemoteRoomsPreview()
  const isActive = useHomeRowOnScreen(row, null)
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
  // The people on this row are that server's: their pictures are the ones
  // that server keeps (0.46.8, per-server pictures), read with its login.
  const peopleScope = useMemo((): ServerScope | null => {
    if (onConnected) return primaryScope()
    if (!rowHost || isWebClient()) return null
    return scopeForHost(rowHost)
  }, [onConnected, rowHost])
  // Its own color (a validated `#rrggbb` from that server's listing), else muted.
  const color = useHomeRowColor(row.address) ?? OTHER_SERVER_AVATAR_COLOR

  return (
    <AgentRowButton
      isActive={isActive}
      color={color}
      dimmed={status.kind === 'offline' || status.kind === 'not-found'}
      onClick={() => {
        if (canOpen) openOtherHomeRow(row, status.kind)
      }}
      onContextMenu={(e) => void homeRowContextMenu(e, home, row)}
      title={title}
      avatar={
        <ProjectAvatar
          projectPath={`home:${row.address}`}
          projectName={row.label}
          projectColor={color}
          iconUrl={avatarUrl}
          size={32}
          fetchIcon={false}
        />
      }
      name={row.label}
      nameMark={isPinned(row) ? <HomeRowPinMark /> : undefined}
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
            pictureScope={peopleScope}
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
  // People and channels (prd-home-people-channels-v1 §5): not agents.
  if (isHomeChatRow(row)) return <HomeChatHomeRow home={home} row={row} index={index} />
  // Local Grok Bots (0.45.7): not a workspace, its own row.
  if (isGrokBotAddress(row.address)) return <GrokBotHomeRow home={home} row={row} index={index} />
  return <WorkspaceHomeRowView home={home} row={row} index={index} />
}

function WorkspaceHomeRowView({ home, row, index }: { home: Home; row: HomeRow; index: number }): React.JSX.Element {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const projects = useProjectsStore((s) => s.projects)
  const parsed = parseHomeAddress(row.address)
  const ws =
    parsed && parsed.host === connectedKey && connectionStatus === 'connected'
      ? findWorkspaceForRow(projects, row)
      : null
  const note = useConnectedRowNote(row)
  // Home M4: while a remote room is on screen, no connected row is active;
  // clicking one gives the main area back to the window's own room. The
  // collapsed rail reads the same answer (home-row-on-screen).
  const isActive = useHomeRowOnScreen(row, ws)
  if (ws && !note) {
    return (
      <div onClickCapture={() => homeRooms.showPrimary()}>
        <SingleProjectItem
          project={ws}
          isActive={isActive}
          onContextMenu={(e) => void homeRowContextMenu(e, home, row)}
          shortcutIndex={index}
          nameMark={isPinned(row) ? <HomeRowPinMark /> : undefined}
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
          nameMark={isPinned(row) ? <HomeRowPinMark /> : undefined}
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

interface HomeRowDrag {
  reorder: RowDragReorder<'home'>
  listRef: React.RefObject<HTMLDivElement | null>
  onDrop(from: number, gap: number): void
}

/** A group's rows as the nav draws them: pins first under "Pinned", then
 *  "Agents". With `drag`, the live list (reorder, hover read-ahead);
 *  without it, the group riding in beside the list during a swipe. */
function HomeRowItems({ home, drag }: { home: Home; drag?: HomeRowDrag }): React.JSX.Element {
  const rows = displayRows(home)
  const pinCount = rows.filter(isPinned).length
  // "Pinned" over the pins; "Agents" over the rest once there are pins or
  // "Most recent at the top" is on (K2 Design's mock).
  const agentsCaption = pinCount > 0 || !!home.recentFirst
  // A drag moves no row: the list's RowDropLine floats where it will land,
  // kept to the dragged row's own section (rowDropLanding).
  // The row Add to Home just added: a brief wash, so it's easy to find.
  const justAdded = useHomeAddPickerStore((s) => s.justAdded)
  return (
    <>
      {pinCount > 0 && <HomeRowCaption kind="pinned" />}
      {rows.map((row, idx) => [
        agentsCaption && idx === pinCount ? <HomeRowCaption key="agents-caption" kind="agents" recentFirst={!!home.recentFirst} /> : null,
        <div
          key={row.address}
          {...(drag ? { 'data-home-row': row.address } : { 'data-home-row-preview': row.address })}
          data-nav-row=""
          data-pinned={isPinned(row) ? '' : undefined}
          data-just-added={drag && justAdded === row.address ? '' : undefined}
          className={drag && justAdded === row.address ? 'bg-[var(--color-row-hover)]' : undefined}
          style={drag ? { opacity: drag.reorder.isDragging('home', idx) ? 0.4 : 1 } : undefined}
          onMouseDown={
            drag
              ? (e) =>
                  drag.reorder.onRowMouseDown(e, {
                    zone: 'home',
                    index: idx,
                    container: () => drag.listRef.current,
                    rowSelector: '[data-home-row]',
                    landingGap: (from, gap) => rowDropLanding(selectedHome(useHomesStore.getState()), from, gap),
                    bounds: () => drag.listRef.current?.closest('[data-home-list-stage]') ?? null,
                    onDrop: drag.onDrop,
                  })
              : undefined
          }
          onClickCapture={drag?.reorder.onRowClickCapture}
          // Read a remote row's Thread ahead (remote Thread load Fix 4).
          onMouseEnter={drag && !isHomeChatRow(row) ? () => homeRowHoverPrefetch.enter(row.address) : undefined}
          onMouseLeave={drag ? () => homeRowHoverPrefetch.leave() : undefined}
        >
          <HomeRowView home={home} row={row} index={shortcutIndexFor(home, idx)} />
          {idx < rows.length - 1 && idx !== pinCount - 1 && <NavRowDivider />}
        </div>,
      ])}
    </>
  )
}

/** An empty group, centred and calm in the list area: "This group is empty",
 *  "Add agents with + below." and a small + (the footer's) that opens Add
 *  to Home; focus comes back to it when that closes. `preview`: the group
 *  a swipe heads to, drawn but not live. */
function EmptyGroupNote({ name, preview = false }: { name: string; preview?: boolean }): React.JSX.Element {
  return (
    <div
      role={preview ? undefined : 'status'}
      aria-hidden={preview ? true : undefined}
      className="absolute inset-0 flex flex-col items-center justify-center gap-1.5 p-4 text-center"
      data-home-empty=""
    >
      <b className="max-w-full truncate type-heading text-[var(--color-text-primary)]">{GROUP_EMPTY_TEXT}</b>
      <span className="type-hint text-[var(--color-text-secondary)]">Add agents with + below.</span>
      {preview ? (
        <span className="mt-1.5 h-8 w-8" />
      ) : (
        <HomeAddButton place="empty" label={`Add to ${name}`} title="Add to Home" className="mt-1.5" />
      )}
    </div>
  )
}

/** The group a swipe heads to, riding in beside the list: its rows from
 *  the top, drawn as the list will draw them when it lands. */
function HomeRowsPreview({ home }: { home: Home }): React.JSX.Element {
  return (
    <ScrollEdgeFrame className="flex-1 min-h-0" scrollClassName="overflow-hidden py-1" tint={HOME_NAV_EDGE_TINT}>
      <div>
        <HomeRowItems home={home} />
      </div>
      {displayRows(home).length === 0 && <EmptyGroupNote name={home.name} preview />}
    </ScrollEdgeFrame>
  )
}

export default function HomeSidebar(): React.JSX.Element {
  const home = useHomesStore(selectedHome)
  const listRef = useRef<HTMLDivElement | null>(null)
  const groups = useHomeGroups()

  // ── Drag-to-reorder (shared with the Agents sidebar: same ghost, line, cursor) ──
  const rowDrag = useRowDragReorder<'home'>()
  // Positions are as shown (pins first): a pinned row moves among the pins,
  // an unpinned one among the unpinned (not at all with "Most recent at
  // the top" on: that order is automatic).
  const dropHomeRow = useCallback((from: number, gap: number) => {
    const to = dropTargetIndex(from, gap)
    if (to === null) return
    const current = selectedHome(useHomesStore.getState())
    const plan = planRowDrop(current, from, to)
    if (!plan) return
    if ('pins' in plan) useHomesStore.getState().reorderPins(current.id, plan.pins)
    else useHomesStore.getState().setRowOrder(current.id, plan.rows)
  }, [])

  // Pins first, then the rest (manual or most recent first).
  const rows = displayRows(home)
  // Most recent at the top: a row that moves up glides there.
  useRowFlip(listRef, rows.map((r) => r.address), !!home.recentFirst, home.id)

  return (
    <div className="relative flex flex-col h-full" data-home-sidebar="" data-group-swipe-zone="">
      <ResizeHandle />

      {/* The top row, at the tab strip's height: the current group's name
          and count, "…" for the Groups dialog and « to collapse the nav. Switching is the group icons at the
          bottom, the sideways swipe and ⌥⌘1–9 (no ⌘ 1-9 hint here). */}
      <div className="no-drag" data-home-header="">
        <GroupNameHeader
          name={groups.current.name}
          icon={groups.current.icon}
          index={groups.current.index}
          count={groups.current.count}
          onEdit={groups.editCurrent}
        />
      </div>

      {/* Home roster. When it scrolls, its ends fade softly under the Home
          picker and the bottom bar, as the Thread's do (Rosson 2026-11-14),
          tinted with the nav's own fill. The sideways swipe moves it as one
          strip with the group it heads to (groups/GroupSwipeSlide). */}
      <div className="relative flex min-h-0 flex-1 flex-col overflow-hidden" data-home-list-stage="" data-group-swipe-stage="">
      <GroupSwipeStrip variant="nav" renderIncoming={(g) => <HomeRowsPreview home={g} />}>
      <ScrollEdgeFrame
        className="flex-1 min-h-0"
        scrollClassName="overflow-y-auto overflow-x-hidden py-1"
        tint={HOME_NAV_EDGE_TINT}
        testId="home-nav-list"
      >
        <div ref={listRef} className="relative" data-home-list="">
          <RowDropLine top={rowDrag.dropLineTop('home')} inset={NAV_ROW_TEXT_INSET} />
          <HomeRowItems home={home} drag={{ reorder: rowDrag, listRef, onDrop: dropHomeRow }} />
        </div>
        {rows.length === 0 && <EmptyGroupNote name={home.name} />}
      </ScrollEdgeFrame>
      </GroupSwipeStrip>
      </div>

      {/* The footer, with no divider: the group icons on their own row,
          then Get started (the setup checklist; nothing while it is off,
          hidden or finished), then the ⌘K field and + (« is in the top
          row). The list's own bottom edge fade (ScrollEdgeFrame)
          runs out just above them, only while there is more below. The
          checklist stays a child of the nav's column, so its open list keeps
          its cap of 45% of the nav. (Zen Mode is the window's top-bar
          toggle, prd-zen-gardens-v1 G5/G6.) */}
      {/* The same 8px (NAV_COLLAPSE_GAP_PX) from the icons to Get started's
          top edge, closed (the button's fill) or open (the list's line),
          and from its bottom to the bottom bar. The gaps are the icon row's
          bottom padding and the bar's top padding, which never change while
          Get started folds open or shut (both parts are drawn then), so
          nothing jumps at either end; the button drops its own padding for
          them. With no Get started, one gap: the icon row lets the bar's
          padding be it. */}
      <div className="flex flex-col px-3 pt-1 pb-2 has-[+[data-nav-footer]]:pb-0" data-home-footer="">
        <div className="flex justify-center no-drag" data-home-group-row="">
          <HomeGroupBar {...groups.bar} />
        </div>
      </div>

      <SetupChecklist className={HOME_SETUP_SLOT_CLASS} />

      {/* The row: the ⌘K field (the command palette's entry) and + (Add to
          Home). */}
      <div className="relative flex flex-col gap-2 px-3 pb-3 pt-2" data-nav-footer="">
        <HomeNavFooterRow />
      </div>
      <AddToHomeHost />
    </div>
  )
}
