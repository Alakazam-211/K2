// Home P1 (prd-home-v1 H1–H7 + vs-live H11–H18) — the Home page.
//
// A top-level page between the gear and Agents. Same full-page overlay
// idiom as Projects / Tickets: a fixed-inset layer with its own draggable
// top bar carrying the ServerSwitcher + PageTabs, so the server dropdown
// stays in reach. Body: the Home picker (header), the selected Home's rows,
// and a bottom bar with Add Agent + From a server.
//
// Nothing here resets on a server switch. `<App>` remounts on a switch
// (ConnectionGate keys it on the host), so this component's own state
// resets, but the Homes live in `stores/homes` (never subscribed to
// `onActiveHostChange`) and the page stays `home` in `page-view`.
//
// Row status polls other servers every 30s, and only while this page is
// open and the window is visible (lib/home-status).

import { useEffect, useMemo, useRef, useState } from 'react'
import { usePageViewStore } from '@/stores/page-view'
import { useHomesStore, selectedHome, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { usePresenceStore } from '@/stores/presence'
import { isWebClient } from '@/lib/is-web'
import { titleBarDragOnMouseDown, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
import { topBarLeftClusterMinWidth, TRAFFIC_LIGHT_CLUSTER_GAP_PX } from '@/lib/desktop-chrome'
import {
  LOCAL_HOME_HOST,
  activeHomeHostKey,
  findWorkspaceForRow,
  homeAddress,
  parseHomeAddress,
  savedHostForKey,
  workspaceHandle,
} from '@/lib/home-address'
import {
  HOME_POLL_MS,
  personInitials,
  probeHost,
  resolveRowStatus,
  useHomeProbeStore,
  type PresencePerson,
  type RowStatus,
} from '@/lib/home-status'
import { credsForHomeHost } from '@/lib/home-creds'
import { openHomeRow } from '@/lib/home-open'
import ServerSwitcher from '@/components/TopBar/ServerSwitcher'
import PageTabs from '@/components/TopBar/PageTabs'
import DesktopChromeLeft from '@/components/TopBar/DesktopChromeLeft'
import DesktopChromeRight from '@/components/TopBar/DesktopChromeRight'
import K2MarkButton from '@/components/TopBar/K2MarkButton'
import { Surface } from '@/components/ui'
import HomePicker from './HomePicker'
import { AddAgentPanel, FromServerPanel } from './HomeAddPanels'

const TOPBAR_HEIGHT = 38

// ── Row status (one row) ─────────────────────────────────────────────────

function useRowStatus(row: HomeRow): { status: RowStatus; place: string } {
  const parsed = parseHomeAddress(row.address)
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const hosts = useConnectHostStore((s) => s.hosts)
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const projects = useProjectsStore((s) => s.projects)
  const roster = usePresenceStore((s) => s.roster)
  const rosterSupported = usePresenceStore((s) => s.supported)
  const hostKey = parsed?.host ?? ''
  const probe = useHomeProbeStore((s) => s.probes[hostKey])
  const connectedKey = activeHomeHostKey(activeHost)
  const onConnected = hostKey === connectedKey
  const ws = onConnected ? findWorkspaceForRow(projects, row) : null
  const activity = useActiveAgentsStore((s) => (ws ? s.getProjectStatus(ws.id) : 'idle'))

  const saved = hostKey === LOCAL_HOME_HOST ? null : savedHostForKey(hosts, hostKey)
  const place =
    hostKey === LOCAL_HOME_HOST ? 'This computer' : saved ? saved.label : hostKey || 'unknown server'

  if (!parsed) return { status: { kind: 'not-found', label: 'Not found', people: [] }, place }
  if (onConnected) {
    if (connectionStatus !== 'connected') {
      return { status: { kind: 'checking', label: 'Checking…', people: [] }, place }
    }
    const self = activeHost === 'local' ? 'owner' : activeHost.username || 'owner'
    return {
      status: resolveRowStatus({
        where: 'connected',
        row,
        workspace: ws,
        activity,
        roster,
        rosterSupported,
        self,
      }),
      place,
    }
  }
  const isLocal = hostKey === LOCAL_HOME_HOST
  return {
    status: resolveRowStatus({
      where: 'other',
      row,
      saved: isLocal ? !isWebClient() : saved !== null,
      hasLogin: isLocal ? true : (saved?.token.length ?? 0) > 0,
      probe,
      self: isLocal ? 'owner' : saved?.username || 'owner',
    }),
    place,
  }
}

const STATUS_CLASS: Record<RowStatus['kind'], string> = {
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

const DOT_CLASS: Partial<Record<RowStatus['kind'], string>> = {
  working: 'bg-[var(--color-status-working)]',
  permission: 'bg-[var(--color-status-error-soft)]',
  live: 'bg-[var(--color-status-ok-soft)]',
  'sign-in': 'bg-[var(--color-status-warn)]',
}

function PresenceChips({ people }: { people: PresencePerson[] }): React.JSX.Element | null {
  if (people.length === 0) return null
  const shown = people.slice(0, 3)
  const extra = people.length - shown.length
  const names = people.map((p) => p.name).join(', ')
  return (
    <span className="flex items-center -space-x-1" title={`Here now: ${names}`} aria-label={`Here now: ${names}`}>
      {shown.map((p) => (
        <span
          key={p.user}
          className="flex h-4 w-4 items-center justify-center rounded-full border border-[var(--color-bg)] bg-[var(--color-accent)] text-[8px] font-bold text-[var(--color-on-accent)]"
        >
          {personInitials(p.name)}
        </span>
      ))}
      {extra > 0 && (
        <span className="flex h-4 min-w-4 items-center justify-center rounded-full border border-[var(--color-bg)] bg-[var(--color-bg-elevated)] px-0.5 text-[8px] font-bold text-[var(--color-text-secondary)]">
          +{extra}
        </span>
      )}
    </span>
  )
}

function HomeRowItem({
  home,
  row,
  index,
  dragIndex,
  setDragIndex,
}: {
  home: Home
  row: HomeRow
  index: number
  dragIndex: number | null
  setDragIndex: (i: number | null) => void
}): React.JSX.Element {
  const { status, place } = useRowStatus(row)
  const parsed = parseHomeAddress(row.address)
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const remoteOnWeb = isWebClient() && parsed !== null && parsed.host !== connectedKey
  const canOpen = !remoteOnWeb && status.kind !== 'not-found'
  const openTitle = remoteOnWeb
    ? 'Open this agent from the desktop app — the web page has no server switcher'
    : status.kind === 'sign-in'
      ? `Sign in to ${place} and open ${row.label}`
      : parsed && parsed.host !== connectedKey
        ? `Switch this window to ${place} and open ${row.label}`
        : `Open ${row.label}`

  return (
    <li
      draggable
      onDragStart={(e) => {
        setDragIndex(index)
        e.dataTransfer.effectAllowed = 'move'
      }}
      onDragOver={(e) => {
        if (dragIndex === null) return
        e.preventDefault()
        e.dataTransfer.dropEffect = 'move'
      }}
      onDrop={(e) => {
        e.preventDefault()
        if (dragIndex !== null && dragIndex !== index) {
          useHomesStore.getState().moveRow(home.id, dragIndex, index)
        }
        setDragIndex(null)
      }}
      onDragEnd={() => setDragIndex(null)}
      className={`group flex items-center gap-3 border-b border-[var(--color-border)] px-4 py-2.5 ${
        dragIndex === index ? 'opacity-50' : ''
      } ${canOpen ? 'cursor-pointer hover:bg-white/[0.04]' : 'cursor-default'}`}
      onClick={() => {
        if (canOpen) openHomeRow(row)
      }}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget) return
        if ((e.key === 'Enter' || e.key === ' ') && canOpen) {
          e.preventDefault()
          openHomeRow(row)
        }
      }}
      role="button"
      tabIndex={0}
      aria-disabled={!canOpen}
      title={openTitle}
    >
      <span
        className={`h-2 w-2 flex-shrink-0 rounded-full ${DOT_CLASS[status.kind] ?? 'bg-[var(--color-border)]'}`}
        aria-hidden
      />
      <div className="min-w-0 flex-1">
        <div className="truncate text-[12px] font-medium text-[var(--color-text-primary)]">{row.label}</div>
        <div className="truncate text-[10px] text-[var(--color-text-muted)]">
          {place} · {row.address}
        </div>
      </div>
      <PresenceChips people={status.people} />
      <span className={`flex-shrink-0 text-[10px] font-medium ${STATUS_CLASS[status.kind]}`}>{status.label}</span>
      <button
        type="button"
        className="no-drag flex h-5 w-5 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] opacity-0 hover:text-[var(--color-text-primary)] group-hover:opacity-100 focus:opacity-100 cursor-pointer"
        title={`Remove ${row.label} from ${home.name}`}
        aria-label={`Remove ${row.label} from ${home.name}`}
        onClick={(e) => {
          e.stopPropagation()
          useHomesStore.getState().removeRow(home.id, row.address)
        }}
      >
        <svg width="9" height="9" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
          <line x1="2" y1="2" x2="10" y2="10" />
          <line x1="10" y1="2" x2="2" y2="10" />
        </svg>
      </button>
    </li>
  )
}

// ── Polling (other servers) ──────────────────────────────────────────────

function useHomeStatusPoll(isOpen: boolean, home: Home): void {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const hosts = useConnectHostStore((s) => s.hosts)
  const otherKeys = useMemo(() => {
    const keys = new Set<string>()
    for (const r of home.rows) {
      const p = parseHomeAddress(r.address)
      if (p && p.host !== connectedKey) keys.add(p.host)
    }
    return [...keys].sort()
  }, [home.rows, connectedKey])
  const otherKeysJoined = otherKeys.join('\n')
  // A login landing (or dropping) for one of them re-polls right away.
  const loginsKey = hosts.map((h) => `${h.id}:${h.token.length > 0 ? 1 : 0}`).join('|')

  useEffect(() => {
    if (!isOpen || otherKeys.length === 0) return
    let cancelled = false
    const run = async (): Promise<void> => {
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return
      const savedHosts = useConnectHostStore.getState().hosts
      await Promise.all(
        otherKeys.map(async (key) => {
          const c = await credsForHomeHost(key, savedHosts)
          if (cancelled) return
          const setProbe = useHomeProbeStore.getState().setProbe
          if (c.kind === 'unsaved') return
          if (c.kind === 'unreachable') {
            setProbe(key, { reach: 'offline', auth: 'ok', presence: null, at: Date.now() })
            return
          }
          // No login: the row says "Sign in" without asking the server.
          if (!c.creds.token) return
          const probe = await probeHost(c.creds)
          if (!cancelled) setProbe(key, probe)
        }),
      )
    }
    void run()
    const timer = setInterval(() => void run(), HOME_POLL_MS)
    const onVisible = (): void => {
      if (document.visibilityState === 'visible') void run()
    }
    document.addEventListener('visibilitychange', onVisible)
    return () => {
      cancelled = true
      clearInterval(timer)
      document.removeEventListener('visibilitychange', onVisible)
    }
    // otherKeysJoined stands in for otherKeys (stable string identity).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen, otherKeysJoined, loginsKey])
}

/** Rows on the connected server follow a workspace rename (handle moved:
 *  the stored id finds it) and pick up its current name. */
function useConnectedRowRepair(isOpen: boolean): void {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const projects = useProjectsStore((s) => s.projects)
  useEffect(() => {
    if (!isOpen || connectionStatus !== 'connected' || projects.length === 0) return
    useHomesStore.getState().repairRows((row) => {
      const p = parseHomeAddress(row.address)
      if (!p || p.host !== connectedKey) return null
      const ws = findWorkspaceForRow(projects, row)
      if (!ws) return null
      const handle = workspaceHandle(ws)
      if (!handle) return null
      return { address: homeAddress(handle, connectedKey), workspaceId: ws.id, label: ws.name }
    })
  }, [isOpen, projects, connectedKey, connectionStatus])
}

// ── Page ─────────────────────────────────────────────────────────────────

type Panel = 'add' | 'server' | null

export default function HomePage(): React.JSX.Element | null {
  const isOpen = usePageViewStore((s) => s.page === 'home')
  const home = useHomesStore(selectedHome)
  const [panel, setPanel] = useState<Panel>(null)
  const [dragIndex, setDragIndex] = useState<number | null>(null)
  const barRef = useRef<HTMLDivElement | null>(null)

  useHomeStatusPoll(isOpen, home)
  useConnectedRowRepair(isOpen)

  useEffect(() => {
    if (!isOpen) setPanel(null)
  }, [isOpen])

  // Close an open add panel on outside click / Esc.
  useEffect(() => {
    if (!panel) return
    const onDown = (e: MouseEvent): void => {
      if (barRef.current && e.target instanceof Node && !barRef.current.contains(e.target)) setPanel(null)
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') setPanel(null)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [panel])

  if (!isOpen) return null

  const web = isWebClient()
  const bottomButton =
    'no-drag flex-1 flex items-center justify-center gap-2 px-3 py-2 text-xs bg-white/[0.04] transition-colors text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.08] cursor-pointer'

  return (
    <div className="fixed inset-[var(--inset-window)] z-50 flex flex-col bg-[var(--color-bg)]">
      {/* Top bar — same left cluster as every page: wordmark + SERVER
          DROPDOWN + the page switcher, draggable. */}
      <Surface
        role2="surface"
        bordered={false}
        className="flex items-center border-b border-[var(--color-border)] px-3 select-none flex-shrink-0"
        onMouseDown={titleBarDragOnMouseDown}
        onDoubleClick={titleBarOnDoubleClick}
        style={{ height: TOPBAR_HEIGHT, minHeight: TOPBAR_HEIGHT }}
      >
        <div
          className="flex items-center flex-1 [&>*]:shrink-0"
          style={{ minWidth: topBarLeftClusterMinWidth(), gap: TRAFFIC_LIGHT_CLUSTER_GAP_PX }}
        >
          <DesktopChromeLeft />
          <K2MarkButton />
          <ServerSwitcher />
          <PageTabs />
        </div>
        <DesktopChromeRight />
      </Surface>

      <div className="flex-1 min-h-0 flex justify-center">
        <div className="flex w-full max-w-2xl min-h-0 flex-col border-x border-[var(--color-border)]">
          {/* Header: which Home. */}
          <div className="flex items-center gap-2 border-b border-[var(--color-border)] px-2 py-1.5 flex-shrink-0">
            <HomePicker />
            <span className="ml-auto pr-2 text-[10px] text-[var(--color-text-muted)]">
              {home.rows.length === 1 ? '1 agent' : `${home.rows.length} agents`}
            </span>
          </div>

          {/* Rows. */}
          <div className="flex-1 min-h-0 overflow-y-auto">
            {home.rows.length === 0 ? (
              <div className="flex h-full items-center justify-center px-8 text-center">
                <p className="text-[11px] text-[var(--color-text-muted)]">
                  This Home is empty. Add agents from this server with Add Agent
                  {web ? '.' : ', or from any saved server with From a server.'}
                </p>
              </div>
            ) : (
              <ul>
                {home.rows.map((row, i) => (
                  <HomeRowItem
                    key={row.address}
                    home={home}
                    row={row}
                    index={i}
                    dragIndex={dragIndex}
                    setDragIndex={setDragIndex}
                  />
                ))}
              </ul>
            )}
          </div>

          {/* Bottom bar — Add Agent + From a server (H5/H6). */}
          <div ref={barRef} className="relative p-3 border-t border-[var(--color-border)] flex gap-2 flex-shrink-0">
            {panel === 'add' && <AddAgentPanel home={home} />}
            {panel === 'server' && <FromServerPanel home={home} />}
            <button
              type="button"
              className={bottomButton}
              aria-expanded={panel === 'add'}
              onClick={() => setPanel((p) => (p === 'add' ? null : 'add'))}
            >
              <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
              </svg>
              Add Agent
            </button>
            {!web && (
              <button
                type="button"
                className={bottomButton}
                aria-expanded={panel === 'server'}
                onClick={() => setPanel((p) => (p === 'server' ? null : 'server'))}
              >
                From a server
              </button>
            )}
          </div>
        </div>
      </div>

      <div
        data-toast-host="home"
        className="pointer-events-none overflow-hidden"
        style={{ position: 'absolute', top: TOPBAR_HEIGHT, right: 0, bottom: 0, left: 0, zIndex: 40 }}
      />
    </div>
  )
}
