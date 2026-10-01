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
//     Clicking it switches this window's server (H15) and lands on Home.

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
import type { RowStatus } from '@/lib/home-status'
import { openHomeRow } from '@/lib/home-open'
import { AgentRowButton, SingleProjectItem } from '@/components/Sidebar/Sidebar'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import ResizeHandle from '@/components/Sidebar/ResizeHandle'
import { SidebarCollapseButton } from '@/components/Sidebar/SidebarCollapseButton'
import { PresenceAvatarCluster } from '@/components/Presence/PresenceWorkspaceAvatars'
import HomePicker from './HomePicker'
import { AddAgentPicker } from './HomeAddPanels'
import { useHomeAddPickerStore, useRowStatus } from './home-room'

/** Avatar color for an agent on another server (its color lives there). */
export const OTHER_SERVER_AVATAR_COLOR = 'var(--color-text-muted)'

const STATUS_TEXT: Record<RowStatus['kind'], string> = {
  working: 'text-[var(--color-status-working)]',
  permission: 'text-[var(--color-status-error-soft)]',
  review: 'text-[var(--color-status-ok-soft)]',
  idle: 'text-[var(--color-text-muted)]',
  live: 'text-[var(--color-status-ok-soft)]',
  offline: 'text-[var(--color-text-muted)]',
  'sign-in': 'text-[var(--color-status-warn)]',
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

function asRole(role: string): 'owner' | 'admin' | 'member' | 'viewer' {
  return role === 'owner' || role === 'admin' || role === 'viewer' ? role : 'member'
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
  if (items.length > 0) items.push({ id: 'home-sep', label: '', type: 'separator' })
  items.push({ id: 'home-remove', label: `Remove from ${home.name}` })

  const clicked = await showContextMenu(items)
  if (clicked === 'home-settings' && ws) {
    useSettingsStore.getState().openSettings('projects', ws.id)
  } else if (clicked === 'home-wiki' && ws) {
    usePageViewStore.getState().openWiki(ws.path)
  } else if (clicked === 'home-open') {
    openHomeRow(row)
  } else if (clicked === 'home-remove') {
    useHomesStore.getState().removeRow(home.id, row.address)
  }
}

/** A row that does not paint as a live Agents row. */
function OtherHomeRow({ home, row }: { home: Home; row: HomeRow }): React.JSX.Element {
  const { status, place, onConnected } = useRowStatus(row)
  const remoteOnWeb = isWebClient() && !onConnected
  const canOpen = !remoteOnWeb && status.kind !== 'not-found'
  const title = remoteOnWeb
    ? 'Open this agent from the desktop app — the web page has no server switcher'
    : status.kind === 'sign-in'
      ? `Sign in to ${place} and open ${row.label}`
      : !onConnected
        ? `Switch this window to ${place} and open ${row.label}`
        : status.kind === 'not-found'
          ? `${row.label} is not on this server any more`
          : `Open ${row.label}`

  return (
    <AgentRowButton
      isActive={false}
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
          </div>
          <RowStateText status={status} />
        </>
      }
    />
  )
}

function HomeRowView({ home, row }: { home: Home; row: HomeRow }): React.JSX.Element {
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
      <SingleProjectItem
        project={ws}
        isActive={ws.id === activeProjectId}
        onContextMenu={(e) => void homeRowContextMenu(e, home, row)}
      />
    )
  }
  return (
    <div className="no-drag">
      <OtherHomeRow home={home} row={row} />
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

      {/* Header controls — the focus-group row's spot and look. */}
      <div className="px-3 pt-3 pb-2 no-drag flex items-center gap-1.5">
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
              <HomeRowView home={home} row={row} />
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
