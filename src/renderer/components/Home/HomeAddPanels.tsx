// Home P1 (vs-live H16/H17) — the Add Agent picker.
//
// One button, in the Agents sidebar's Add Workspace spot, opens one
// picker with two choices inside it:
// This server: the CONNECTED server's workspaces (already in the projects
// store — no new fetch) that are not on this Home yet.
// From a server: this client's saved servers (the switcher list, plus
// This computer when the window is on a remote), minus the connected one;
// then that server's workspaces read with ITS OWN saved login
// (host-ops, the Settings → Connections tile path). No switch. A server
// without a login offers the sign-in that caches a token without
// switching (`signInForManagement`).
//
// Adding never creates a workspace folder and never calls Add Workspace
// (H7): it only writes a row into `k2.homes.v1`.

import { useEffect, useMemo, useState } from 'react'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useHomesStore, type Home } from '@/stores/homes'
import { hostOpGet } from '@/lib/host-ops'
import { credsForHomeHost } from '@/lib/home-creds'
import {
  LOCAL_HOME_HOST,
  activeHomeHostKey,
  findWorkspaceForRow,
  homeAddress,
  homeHostKey,
  parseHomeAddress,
  workspaceHandle,
} from '@/lib/home-address'
import { hostDisplayAddress } from '@/components/TopBar/ServerSwitcher'
import { isWebClient } from '@/lib/is-web'

interface ListedWorkspace {
  id: string
  name: string
  handle?: string | null
}

/** Workspaces of one server that can go on `home` (have a handle, not
 *  already there). */
function addable(home: Home, hostKey: string, workspaces: ListedWorkspace[]): ListedWorkspace[] {
  const onHome = new Set(home.rows.map((r) => r.address))
  return workspaces.filter((w) => {
    const h = workspaceHandle(w)
    return h !== null && !onHome.has(homeAddress(h, hostKey))
  })
}

function addTo(home: Home, hostKey: string, w: ListedWorkspace): void {
  const h = workspaceHandle(w)
  if (!h) return
  useHomesStore.getState().addRow(home.id, {
    address: homeAddress(h, hostKey),
    workspaceId: w.id,
    label: w.name,
  })
}

/** Rename repair from a freshly listed server: rows on `hostKey` whose
 *  handle moved get the new address; labels follow the name. */
export function repairFromList(hostKey: string, workspaces: ListedWorkspace[]): void {
  useHomesStore.getState().repairRows((row) => {
    const p = parseHomeAddress(row.address)
    if (!p || p.host !== hostKey) return null
    const ws = findWorkspaceForRow(workspaces, row)
    if (!ws) return null
    const handle = workspaceHandle(ws)
    if (!handle) return null
    return { address: homeAddress(handle, hostKey), workspaceId: ws.id, label: ws.name }
  })
}

// The Agents sidebar's dropdown look (FocusGroupDropdown), opening upward
// from the bottom bar.
const panelClass =
  'absolute bottom-full left-3 right-3 z-50 mb-0.5 max-h-[60vh] overflow-y-auto bg-[var(--color-bg)] border border-[var(--color-border)] shadow-xl py-0.5'
const itemClass =
  'no-drag w-full flex items-center gap-2 px-3 py-1.5 text-left text-[11px] transition-colors cursor-pointer text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
const headClass = 'px-3 pt-1.5 pb-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]'

function WorkspaceList({
  home,
  hostKey,
  workspaces,
  emptyText,
}: {
  home: Home
  hostKey: string
  workspaces: ListedWorkspace[]
  emptyText: string
}): React.JSX.Element {
  const list = addable(home, hostKey, workspaces)
  if (list.length === 0) {
    return <p className="px-3 py-2 text-[11px] text-[var(--color-text-muted)]">{emptyText}</p>
  }
  return (
    <>
      {list.map((w) => (
        <button key={w.id} type="button" className={itemClass} onClick={() => addTo(home, hostKey, w)}>
          <span className="w-3 flex-shrink-0 text-[var(--color-text-muted)]">+</span>
          <span className="truncate flex-1">{w.name}</span>
          <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">{workspaceHandle(w)}</span>
        </button>
      ))}
    </>
  )
}

export function connectedServerLabel(active: 'local' | ConnectHost): string {
  return active === 'local' ? 'This computer' : active.label
}

function BackHead({ label, onBack }: { label: string; onBack: () => void }): React.JSX.Element {
  return (
    <button type="button" className={`${itemClass} ${headClass} normal-case`} onClick={onBack}>
      ‹ {label}
    </button>
  )
}

function ThisServerList({ home, onBack }: { home: Home; onBack: () => void }): React.JSX.Element {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const projects = useProjectsStore((s) => s.projects)
  const hostKey = activeHomeHostKey(activeHost)
  return (
    <>
      <BackHead label={`Agents on ${connectedServerLabel(activeHost)}`} onBack={onBack} />
      <WorkspaceList
        home={home}
        hostKey={hostKey}
        workspaces={projects}
        emptyText="Every agent on this server is already on this Home."
      />
    </>
  )
}

type PickerMode = 'menu' | 'this' | 'server'

/** The one Add Agent picker: This server, or From a server (desktop). */
export function AddAgentPicker({ home }: { home: Home }): React.JSX.Element {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const [mode, setMode] = useState<PickerMode>('menu')
  const web = isWebClient()
  return (
    <div className={panelClass} role="menu" aria-label="Add Agent">
      {mode === 'menu' && (
        <>
          <div className={headClass}>Add an agent to {home.name}</div>
          <button type="button" role="menuitem" className={itemClass} onClick={() => setMode('this')}>
            <span className="truncate flex-1">This server</span>
            <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">{connectedServerLabel(activeHost)}</span>
            <span className="flex-shrink-0 text-[var(--color-text-muted)]">›</span>
          </button>
          {!web && (
            <button type="button" role="menuitem" className={itemClass} onClick={() => setMode('server')}>
              <span className="truncate flex-1">From a server</span>
              <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">saved servers</span>
              <span className="flex-shrink-0 text-[var(--color-text-muted)]">›</span>
            </button>
          )}
        </>
      )}
      {mode === 'this' && <ThisServerList home={home} onBack={() => setMode('menu')} />}
      {mode === 'server' && <FromServerList home={home} onBack={() => setMode('menu')} />}
    </div>
  )
}

type Listing =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'signin'; host: ConnectHost }
  | { kind: 'ok'; workspaces: ListedWorkspace[] }
  | { kind: 'error'; message: string }

function FromServerList({ home, onBack }: { home: Home; onBack: () => void }): React.JSX.Element {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const hosts = useConnectHostStore((s) => s.hosts)
  const connectedKey = activeHomeHostKey(activeHost)
  const [picked, setPicked] = useState<string | null>(null)
  const [listing, setListing] = useState<Listing>({ kind: 'idle' })

  // The saved servers, deduped by row host key, minus the connected one.
  const servers = useMemo(() => {
    const out: { key: string; label: string; sub: string }[] = []
    if (connectedKey !== LOCAL_HOME_HOST) out.push({ key: LOCAL_HOME_HOST, label: 'This computer', sub: 'local' })
    const seen = new Set(out.map((s) => s.key))
    const sorted = [...hosts].sort((a, b) => a.label.localeCompare(b.label, undefined, { sensitivity: 'base' }))
    for (const h of sorted) {
      const key = homeHostKey(h)
      if (key === connectedKey || seen.has(key)) continue
      seen.add(key)
      out.push({ key, label: h.label, sub: hostDisplayAddress(h) })
    }
    return out
  }, [hosts, connectedKey])

  // A token for the picked server (a sign-in just landed) re-lists.
  const pickedToken =
    picked && picked !== LOCAL_HOME_HOST
      ? hosts.find((h) => homeHostKey(h) === picked && h.token.length > 0)?.token ?? ''
      : ''

  useEffect(() => {
    if (!picked) return
    let cancelled = false
    setListing({ kind: 'loading' })
    void (async () => {
      const c = await credsForHomeHost(picked, useConnectHostStore.getState().hosts)
      if (cancelled) return
      if (c.kind === 'unsaved') {
        setListing({ kind: 'error', message: 'That server is no longer saved.' })
        return
      }
      if (c.kind === 'unreachable') {
        setListing({ kind: 'error', message: 'This computer’s daemon is not reachable.' })
        return
      }
      if (!c.creds.token) {
        const host = useConnectHostStore.getState().hosts.find((h) => homeHostKey(h) === picked)
        if (host) setListing({ kind: 'signin', host })
        else setListing({ kind: 'error', message: 'That server is no longer saved.' })
        return
      }
      try {
        const raw = await hostOpGet<unknown>(c.creds, 'projects/list')
        if (cancelled) return
        const workspaces = Array.isArray(raw) ? (raw as ListedWorkspace[]) : []
        repairFromList(picked, workspaces)
        setListing({ kind: 'ok', workspaces })
      } catch (err) {
        if (cancelled) return
        const message = err instanceof Error ? err.message : String(err)
        setListing({ kind: 'error', message })
      }
    })()
    return () => {
      cancelled = true
    }
  }, [picked, pickedToken])

  if (!picked) {
    return (
      <>
        <BackHead label="Saved servers" onBack={onBack} />
        {servers.length === 0 ? (
          <p className="px-3 py-2 text-[11px] text-[var(--color-text-muted)]">
            No other saved servers. Add one in Settings → Connections.
          </p>
        ) : (
          servers.map((s) => (
            <button key={s.key} type="button" className={itemClass} onClick={() => setPicked(s.key)}>
              <span className="truncate flex-1">{s.label}</span>
              <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">{s.sub}</span>
            </button>
          ))
        )}
      </>
    )
  }

  const server = servers.find((s) => s.key === picked)
  return (
    <>
      <BackHead label={server?.label ?? picked} onBack={() => setPicked(null)} />
      {listing.kind === 'loading' || listing.kind === 'idle' ? (
        <p className="px-3 py-2 text-[11px] text-[var(--color-text-muted)]">Loading agents…</p>
      ) : listing.kind === 'signin' ? (
        <div className="flex items-center gap-2 px-3 py-2">
          <p className="flex-1 text-[11px] text-[var(--color-text-secondary)]">
            Sign in to {listing.host.label} to list its agents.
          </p>
          <button
            type="button"
            className="no-drag px-2 py-1 text-[11px] bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 cursor-pointer"
            onClick={() => useConnectHostStore.getState().signInForManagement(listing.host)}
          >
            Sign in
          </button>
        </div>
      ) : listing.kind === 'error' ? (
        <p className="px-3 py-2 text-[11px] text-[var(--color-status-error-soft)]">
          Couldn’t list agents: {listing.message}
        </p>
      ) : (
        <WorkspaceList
          home={home}
          hostKey={picked}
          workspaces={listing.workspaces}
          emptyText="Every agent on that server is already on this Home."
        />
      )}
    </>
  )
}
