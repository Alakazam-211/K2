// Home on the Agents shell — the sidebar (prd-home-v1 H2/H3/H5/H6).
//
// Same chrome as the Agents `Sidebar`: resize handle, the header control
// row (here the Home picker in the focus-group dropdown's spot, plus the
// command palette button), the row list, and the bottom bar with ONE
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
//     With "Open agents from other servers here" on (the default on macOS
//     and Linux from 0.43.2), clicking it opens that server's room in the
//     main area without switching (usable from K2 0.43.0, view only from
//     the floor up to that). Off, or below the floor, it switches this
//     window's server (H15) and lands on Home.

import { useCallback, useEffect, useRef, useState } from 'react'
import { useHomesStore, selectedHome, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { usePageViewStore } from '@/stores/page-view'
import { useCommandPaletteStore } from '@/stores/command-palette'
import { isWebClient } from '@/lib/is-web'
import { showContextMenu } from '@/lib/context-menu'
import type { ContextMenuItemDef } from '@/stores/context-menu'
import { activeHomeHostKey, findWorkspaceForRow, parseHomeAddress } from '@/lib/home-address'
import { switchWindowToRow } from '@/lib/home-switch'
import { homeRoomVerdict, type HomeRoomVerdict } from '@/lib/home-room-floor'
import { hostPool } from '@/lib/host-pool-instance'
import { useStore } from 'zustand'
import type { RowStatus } from '@/lib/home-status'
import { openHomeRow } from '@/lib/home-open'
import { useRemoteRoomsPreview } from '@/lib/remote-rooms-preview'
import { homeRooms, useShownHomeRoom } from '@/stores/home-rooms'
import { AgentRowButton, ShortcutIndexBadge, SingleProjectItem } from '@/components/Sidebar/Sidebar'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import ResizeHandle from '@/components/Sidebar/ResizeHandle'
import { SidebarCollapseButton } from '@/components/Sidebar/SidebarCollapseButton'
import { PresenceAvatarCluster } from '@/components/Presence/PresenceWorkspaceAvatars'
import HomePicker from './HomePicker'
import { AddAgentPicker } from './HomeAddPanels'
import { useConnectedRowNote, useHomeAddPickerStore, useRowStatus } from './home-room'

/** Avatar color for an agent on another server (its color lives there). */
export const OTHER_SERVER_AVATAR_COLOR = 'var(--color-text-muted)'

const STATUS_TEXT: Record<RowStatus['kind'], string> = {
  working: 'text-[var(--color-status-working)]',
  permission: 'text-[var(--color-status-error-soft)]',
  review: 'text-[var(--color-status-ok-soft)]',
  idle: 'text-[var(--color-text-muted)]',
  live: 'text-[var(--color-status-ok-soft)]',
  starting: 'text-[var(--color-text-muted)]',
  offline: 'text-[var(--color-text-muted)]',
  'signing-in': 'text-[var(--color-text-muted)]',
  'sign-in': 'text-[var(--color-status-warn)]',
  'no-access': 'text-[var(--color-status-error-soft)]',
  checking: 'text-[var(--color-text-muted)]',
  'not-found': 'text-[var(--color-status-error-soft)]',
}

/** Which machine a row is on — sized like the Agents row's group chips. */
export function MachineChip({ place }: { place: string }): React.JSX.Element {
  return (
    <span
      className="box-border inline-flex h-3.5 max-w-[6.5rem] items-center gap-0.5 truncate px-1 py-0 text-[9px] font-medium leading-none border border-[var(--color-border)] text-[var(--color-text-muted)]"
      title={`On ${place}`}
      data-machine-chip=""
    >
      <svg className="w-2 h-2 flex-shrink-0" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
        <rect x="2" y="2" width="12" height="9" rx="1" />
        <path d="M5 14h6" />
        <path d="M8 11v3" />
      </svg>
      <span className="truncate">{place}</span>
    </span>
  )
}

/** The second-line state of a row that is not live on this server, in the
 *  AgentSpinner slot's type (10px mono, one leading). */
function RowStateText({ status }: { status: RowStatus }): React.JSX.Element {
  return (
    <span
      className={`inline-flex h-3.5 flex-shrink-0 items-center leading-none text-[10px] font-mono ${STATUS_TEXT[status.kind]}`}
      data-row-status={status.kind}
    >
      {status.label.toLowerCase()}
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
  const verdict = homeRoomVerdict(useStore(hostPool.store, (s) => s.entries[rowHost]?.boot ?? null))
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
          size={32}
          fetchIcon={false}
        />
      }
      name={row.label}
      nameAside={
        <PresenceAvatarCluster users={status.people.map((p) => ({ user: p.user, role: asRole(p.role) }))} />
      }
      subline={
        <>
          <div className="flex min-w-0 flex-1 items-center gap-0.5 overflow-hidden">
            {place !== null && <MachineChip place={place} />}
            {status.note && <SameServerNote note={status.note} />}
          </div>
          <RowStateText status={status} />
          <ShortcutIndexBadge index={index} />
        </>
      }
    />
  )
}

/** `index` is the row's position: its Cmd+N badge (useWorkspaceIndexShortcuts
 *  selects `rows[N - 1]` on Home), shown for the first nine rows like the
 *  Agents pinned area. */
function HomeRowView({ home, row, index }: { home: Home; row: HomeRow; index: number }): React.JSX.Element {
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

  // ── Drag-to-reorder (the Agents sidebar's mouse drag + accent line) ──
  const [dragIndex, setDragIndex] = useState<number | null>(null)
  const [dropIndex, setDropIndex] = useState<number | null>(null)
  const dropIndexRef = useRef<number | null>(null)
  const suppressClickRef = useRef(false)

  const handleRowMouseDown = useCallback(
    (e: React.MouseEvent, fromIndex: number) => {
      if (e.button !== 0) return
      // Group chips and other in-row controls keep their own click.
      if ((e.target as HTMLElement).closest('[role="button"]')) return
      const startX = e.clientX
      const startY = e.clientY
      let started = false
      const onMove = (ev: MouseEvent): void => {
        if (!started && (Math.abs(ev.clientX - startX) > 3 || Math.abs(ev.clientY - startY) > 5)) {
          started = true
          setDragIndex(fromIndex)
          document.body.style.cursor = 'grabbing'
          document.body.style.userSelect = 'none'
        }
        if (!started || !listRef.current) return
        const items = listRef.current.querySelectorAll('[data-home-row]')
        let idx = 0
        for (let i = 0; i < items.length; i++) {
          const rect = items[i].getBoundingClientRect()
          if (ev.clientY > rect.top + rect.height / 2) idx = i + 1
        }
        dropIndexRef.current = idx
        setDropIndex(idx)
      }
      const onUp = (): void => {
        document.removeEventListener('mousemove', onMove)
        document.removeEventListener('mouseup', onUp)
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
        if (started) {
          suppressClickRef.current = true
          const di = dropIndexRef.current
          if (di !== null && di !== fromIndex && di !== fromIndex + 1) {
            const to = di > fromIndex ? di - 1 : di
            const current = selectedHome(useHomesStore.getState())
            useHomesStore.getState().moveRow(current.id, fromIndex, to)
          }
        }
        setDragIndex(null)
        setDropIndex(null)
        dropIndexRef.current = null
      }
      document.addEventListener('mousemove', onMove)
      document.addEventListener('mouseup', onUp)
    },
    [],
  )

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

      {/* Beta notice, above the Home picker and the command palette button. */}
      <div
        className="px-3 pt-3 no-drag flex items-center justify-center gap-2.5 text-[11px] text-[var(--color-text-muted)]"
        data-testid="home-beta-notice"
      >
        <span>Home is in</span>
        <span
          className="flex-shrink-0 uppercase"
          style={{
            fontSize: '9px',
            fontWeight: 600,
            letterSpacing: '0.04em',
            lineHeight: 1.2,
            padding: '2px 5px',
            borderRadius: 0,
            color: 'var(--color-on-accent, #ffffff)',
            background: 'var(--color-accent, #3b82f6)',
            border: '1px solid var(--color-accent, #3b82f6)',
          }}
        >
          Beta
        </span>
      </div>

      {/* Header controls — the focus-group row's spot and look. */}
      <div className="px-3 pt-2 pb-2 no-drag flex items-center gap-1.5">
        <div className="flex-1 min-w-0">
          <HomePicker />
        </div>
        <button
          className="flex-shrink-0 flex items-center gap-1 px-1.5 py-1 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] transition-colors cursor-pointer"
          onClick={() => useCommandPaletteStore.getState().toggle()}
          title="Command Palette (⌘K)"
        >
          <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
        </button>
      </div>

      {/* Home roster */}
      <div className="flex-1 overflow-y-auto overflow-x-hidden py-1">
        <div ref={listRef}>
          {rows.map((row, idx) => (
            <div
              key={row.address}
              data-home-row={row.address}
              style={{ opacity: dragIndex === idx ? 0.4 : 1 }}
              onMouseDown={(e) => handleRowMouseDown(e, idx)}
              onClickCapture={(e) => {
                if (suppressClickRef.current) {
                  suppressClickRef.current = false
                  e.stopPropagation()
                  e.preventDefault()
                }
              }}
            >
              {dragIndex !== null && dropIndex === idx && <div className="h-[2px] bg-[var(--color-accent)] mx-3" />}
              <HomeRowView home={home} row={row} index={idx} />
              {idx < rows.length - 1 && !(dragIndex !== null && dropIndex === idx + 1) && (
                <div className="border-b border-[var(--color-border)]" />
              )}
            </div>
          ))}
          {dragIndex !== null && dropIndex === rows.length && <div className="h-[2px] bg-[var(--color-accent)] mx-3" />}
        </div>

        {rows.length === 0 && (
          <div className="px-3 py-6 text-center">
            <p className="text-xs text-[var(--color-text-muted)]">No agents on this Home yet</p>
            <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-60">Add an agent to get started</p>
          </div>
        )}
      </div>

      {/* Add Agent + collapse the nav — the Add Workspace bar. */}
      <div ref={barRef} className="relative p-3 border-t border-[var(--color-border)] flex gap-2">
        {pickerOpen && <AddAgentPicker home={home} />}
        <button
          className="no-drag flex-1 flex items-center justify-center gap-2 px-3 py-2 text-xs bg-white/[0.04] transition-colors text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.08]"
          onClick={() => setPickerOpen(!pickerOpen)}
          aria-expanded={pickerOpen}
          aria-haspopup="menu"
        >
          <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
          </svg>
          Add Agent
        </button>
        <SidebarCollapseButton />
      </div>
    </div>
  )
}
